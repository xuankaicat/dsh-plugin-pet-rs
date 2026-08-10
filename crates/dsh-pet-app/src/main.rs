//! DSH 桌宠主入口。
//!
//! 对应 main.js 的主流程：单实例 → 配置 → 资源 → tokio 任务 → winit 事件循环。
//
// winit 0.30 的 EventLoop::run / create_window 已标记 deprecated（建议用 run_app），
// v1 仍用 run + 闭包形式（迁移到 ApplicationHandler trait 是 v2 待办）。
#![allow(deprecated)]

mod audio;
mod platform;
mod shot_mode;
mod tasks;
mod tray;

use std::sync::Arc;
use std::time::{Duration, Instant};

use dsh_pet_core::{Config, Mode, PetState, Snapshot, StateEvent};
use dsh_pet_ui::{
    create_window, ClickDecision, ClickTarget, ClickTracker, InputAction, InputState, Renderer,
    SettingsHit,
};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};

use crate::audio::AudioPlayer;
use crate::tasks::{is_valid_endpoint, spawn_network_tasks, tick_task};

/// 从异步任务发往事件循环的用户事件
#[derive(Debug, Clone)]
enum UserEvent {
    /// 状态快照更新
    Snapshot(Snapshot),
    /// 托盘菜单动作
    TrayAction(tray::TrayAction),
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dsh_pet=info,warn".into()),
        )
        .init();

    // --shot 模式：渲染 5 状态截图后退出
    let args: Vec<String> = std::env::args().collect();
    if let Some(idx) = args.iter().position(|a| a == "--shot") {
        let out_dir = args
            .get(idx + 1)
            .cloned()
            .unwrap_or_else(|| ".shots".into());
        tracing::info!("截图自检模式，输出目录: {out_dir}");
        return shot_mode::run_shot_mode(std::path::Path::new(&out_dir));
    }

    // 单实例锁
    let instance = single_instance::SingleInstance::new("com.dsh.pet")?;
    if !instance.is_single() {
        eprintln!("DSH 桌宠已在运行");
        std::process::exit(0);
    }

    // 配置
    let mut config = Config::load();

    // DSH URL：环境变量优先，其次配置文件，最后默认值
    let mut dsh_url = std::env::var("DSH_PET_URL")
        .unwrap_or_else(|_| config.normalized_endpoint())
        .trim_end_matches('/')
        .to_string();

    // 资源
    let custom_dir = platform::custom_dir();
    let sprites = Arc::new(dsh_pet_core::SpritePack::load(&custom_dir)?);
    let font = platform::load_font();
    let scale = config.scale;

    // tokio 运行时
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let app_cancel = CancellationToken::new();

    // 事件通道
    let (event_tx, event_rx) = mpsc::channel::<StateEvent>(256);
    let (snapshot_tx, snapshot_rx) = watch::channel(Snapshot::starting());

    // HTTP 客户端（RPC 用，SSE 走 WebSocket 不需要 HTTP 客户端）
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    // 启动网络任务（RPC + 双 SSE）— endpoint 变更时可通过 spawn_network_tasks 重建
    let mut net_cancel = spawn_network_tasks(&rt, &dsh_url, &http, event_tx.clone());

    // 定时心跳（独立于网络任务，endpoint 变更不影响）
    rt.spawn(tick_task(event_tx.clone(), app_cancel.clone()));

    // 状态聚合任务
    rt.spawn(async move {
        let mut state = PetState::new();
        let mut event_rx = event_rx;
        while let Some(event) = event_rx.recv().await {
            state.apply(event);
            let snapshot = state.build_snapshot();
            let _ = snapshot_tx.send(snapshot);
        }
    });

    // winit 事件循环
    let mut event_loop_builder = EventLoop::<UserEvent>::with_user_event();
    let event_loop = event_loop_builder.build()?;
    let proxy = event_loop.create_proxy();

    // snapshot 变化 → 发到事件循环
    {
        let proxy = proxy.clone();
        rt.spawn(async move {
            let mut snapshot_rx = snapshot_rx;
            while snapshot_rx.changed().await.is_ok() {
                let snap = snapshot_rx.borrow().clone();
                let _ = proxy.send_event(UserEvent::Snapshot(snap));
            }
        });
    }

    // 创建窗口
    let window = Arc::new(create_window(&event_loop, &config)?);

    // 创建 softbuffer surface — 用 Arc<Window> 避免 window 被 display_handle/window_handle 借用
    // （Arc<Window> 实现了 HasDisplayHandle + HasWindowHandle，且为 'static 无生命周期依赖）
    let context =
        softbuffer::Context::new(window.clone()).map_err(|e| anyhow::anyhow!("context: {e}"))?;
    let mut surface = softbuffer::Surface::new(&context, window.clone())
        .map_err(|e| anyhow::anyhow!("surface: {e}"))?;

    // 渲染器：窗口/气泡使用 dpi，鲸鱼舞台额外乘用户 pet_scale。
    let dpi = window.scale_factor() as f32;
    let mut renderer = Renderer::new(sprites.clone(), font, scale, dpi);
    renderer.set_bubble_visible(config.bubble_visible);
    renderer.set_sound_on(config.sound_on);
    let mut input = InputState::new();
    let mut click_tracker = ClickTracker::default();
    let audio = AudioPlayer::new();

    // 平台初始化
    platform::init();
    platform::hide_dock();

    // 托盘
    let tray_ui = tray::create(config.sound_on, config.bubble_visible, config.scale);

    // 运行前初始化状态
    let mut last_snapshot = Snapshot::starting();
    let mut last_mode = Mode::Starting;
    let mut modifiers = winit::keyboard::ModifiersState::empty();
    let start_time = Instant::now();

    // 主事件循环
    event_loop.run(move |event, elwt| {
        let time_ms = start_time.elapsed().as_millis() as u64;

        match event {
            Event::WindowEvent {
                event: ref win_event,
                ..
            } => match win_event {
                WindowEvent::RedrawRequested => {
                    // 先把 pixmap 尺寸对齐到窗口物理尺寸（避免 stride 不匹配导致扫描线）
                    let inner = window.inner_size();
                    renderer.resize_pixmap(inner.width, inner.height);
                    let buf = renderer.render(&last_snapshot, time_ms);
                    let w = std::num::NonZeroU32::new(inner.width)
                        .unwrap_or(std::num::NonZeroU32::new(1).unwrap());
                    let h = std::num::NonZeroU32::new(inner.height)
                        .unwrap_or(std::num::NonZeroU32::new(1).unwrap());
                    let _ = surface.resize(w, h);
                    if let Ok(mut buffer) = surface.buffer_mut() {
                        // tiny-skia: [R,G,B,A] → u32 LE = 0xAABBGGRR
                        // softbuffer Win32: 0xAARRGGBB (DWM 用 alpha 合成)
                        // 需交换 R(bits 0-7) 和 B(bits 16-23)
                        for (dst, &src) in buffer.iter_mut().zip(buf.iter()) {
                            let r = src & 0xFF;
                            let g = (src >> 8) & 0xFF;
                            let b = (src >> 16) & 0xFF;
                            let a = (src >> 24) & 0xFF;
                            *dst = (a << 24) | (r << 16) | (g << 8) | b;
                        }
                        let _ = buffer.present();
                    }
                    if Renderer::is_animating(last_snapshot.mode) {
                        window.request_redraw();
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    input.update_cursor(position.x, position.y);
                    let in_bubble = renderer.is_bubble_hit(position.x, position.y);
                    let in_whale = renderer.is_whale_region(position.x, position.y);
                    input.set_hit_regions(in_bubble, in_whale);
                    // 拖拽：用平台 API 获取鼠标屏幕绝对坐标，完全绕过 winit 相对坐标
                    if input.is_down() {
                        if let Some(cur) = platform::cursor_screen_position() {
                            input.check_moved(cur.0 as f64, cur.1 as f64);
                            if input.is_moved() {
                                if let Some((start, origin)) = input.drag_full_state() {
                                    let dx = cur.0 - start.0 as i32;
                                    let dy = cur.1 - start.1 as i32;
                                    window.set_outer_position(winit::dpi::PhysicalPosition::new(
                                        origin.0 + dx,
                                        origin.1 + dy,
                                    ));
                                }
                            }
                        }
                    }
                }
                WindowEvent::MouseInput { state, .. } => {
                    let renderer_ref = &renderer;
                    let action = input.handle(win_event, |x, y| renderer_ref.is_whale_hit(x, y));
                    // 按下时记录窗口屏幕坐标和鼠标屏幕绝对坐标
                    if *state == winit::event::ElementState::Pressed
                        && input.is_down()
                        && input.drag_origin().is_none()
                    {
                        if let Ok(win_pos) = window.outer_position() {
                            input.set_drag_origin((win_pos.x, win_pos.y));
                            if let Some(cur) = platform::cursor_screen_position() {
                                input.set_start_mouse_screen(cur.0 as f64, cur.1 as f64);
                            }
                        }
                    }
                    if *state == winit::event::ElementState::Released {
                        input.clear_drag_origin();
                    }
                    match action {
                        InputAction::Click(ClickTarget::Whale) => {
                            // 点击鲸鱼时提交 endpoint 编辑
                            commit_endpoint_if_focused(
                                &mut renderer,
                                &mut config,
                                &mut dsh_url,
                                &mut net_cancel,
                                &rt,
                                &http,
                                &event_tx,
                            );
                            if !renderer.endpoint_focused() {
                                window.set_ime_allowed(false);
                            }
                            let decision = click_tracker.register(Instant::now());
                            if decision == ClickDecision::DoubleClick {
                                // 打开设置前，若有未提交的 endpoint 编辑则丢弃
                                renderer.set_endpoint_focused(false);
                                window.set_ime_allowed(false);
                                renderer.set_endpoint_text(dsh_url.clone());
                                renderer.set_settings_visible(true);
                                window.request_redraw();
                            }
                        }
                        InputAction::Click(ClickTarget::Bubble) => {
                            if renderer.settings_visible() {
                                let (x, y) = input.cursor_position();
                                match renderer.settings_hit_test(x, y) {
                                    SettingsHit::ToggleSound => {
                                        // 点击开关前先提交 endpoint 编辑
                                        commit_endpoint_if_focused(
                                            &mut renderer,
                                            &mut config,
                                            &mut dsh_url,
                                            &mut net_cancel,
                                            &rt,
                                            &http,
                                            &event_tx,
                                        );
                                        if !renderer.endpoint_focused() {
                                            window.set_ime_allowed(false);
                                        }
                                        config.sound_on = !config.sound_on;
                                        renderer.set_sound_on(config.sound_on);
                                        if let Some(tray) = tray_ui.as_ref() {
                                            tray.set_sound_on(config.sound_on);
                                        }
                                        config.save();
                                    }
                                    SettingsHit::EndpointInput => {
                                        renderer.set_endpoint_focused(true);
                                        window.set_ime_allowed(true);
                                        if let Some((x, y, w, h)) = renderer.endpoint_ime_area() {
                                            window.set_ime_cursor_area(
                                                winit::dpi::PhysicalPosition::new(
                                                    x as i32, y as i32,
                                                ),
                                                winit::dpi::PhysicalSize::new(w as u32, h as u32),
                                            );
                                        }
                                    }
                                    SettingsHit::Close => {
                                        // 关闭面板前丢弃 endpoint 编辑
                                        renderer.set_endpoint_focused(false);
                                        window.set_ime_allowed(false);
                                        renderer.set_endpoint_text(dsh_url.clone());
                                        renderer.set_settings_visible(false);
                                    }
                                    SettingsHit::None => {
                                        // 点击面板空白区域 → 提交 endpoint 编辑并失焦
                                        commit_endpoint_if_focused(
                                            &mut renderer,
                                            &mut config,
                                            &mut dsh_url,
                                            &mut net_cancel,
                                            &rt,
                                            &http,
                                            &event_tx,
                                        );
                                        if !renderer.endpoint_focused() {
                                            window.set_ime_allowed(false);
                                        }
                                    }
                                }
                            } else {
                                let _ = open::that(&dsh_url);
                            }
                        }
                        InputAction::ContextMenu => {
                            // 右键菜单：v1 暂不弹出，用户通过托盘访问
                        }
                        _ => {}
                    }
                    if *state == winit::event::ElementState::Released {
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseWheel { .. } => {
                    let renderer_ref = &renderer;
                    let action = input.handle(win_event, |x, y| renderer_ref.is_whale_hit(x, y));
                    if let InputAction::Scroll { delta } = action {
                        renderer.scroll_offset = (renderer.scroll_offset + delta).max(0.0);
                        window.request_redraw();
                    }
                }
                WindowEvent::CloseRequested => {
                    elwt.exit();
                }
                WindowEvent::ModifiersChanged(m) => {
                    modifiers = m.state();
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == winit::event::ElementState::Pressed
                        && renderer.settings_visible() =>
                {
                    let has_preedit = renderer.endpoint_preedit().is_some();
                    let ctrl = modifiers.control_key();
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            if renderer.endpoint_focused() {
                                renderer.set_endpoint_focused(false);
                                window.set_ime_allowed(false);
                                renderer.set_endpoint_text(dsh_url.clone());
                            } else {
                                renderer.set_settings_visible(false);
                            }
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::Backspace)
                            if renderer.endpoint_focused() && !has_preedit =>
                        {
                            renderer.backspace_at_cursor();
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::Enter) if renderer.endpoint_focused() => {
                            commit_endpoint_if_focused(
                                &mut renderer,
                                &mut config,
                                &mut dsh_url,
                                &mut net_cancel,
                                &rt,
                                &http,
                                &event_tx,
                            );
                            if !renderer.endpoint_focused() {
                                window.set_ime_allowed(false);
                            }
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::ArrowLeft)
                            if renderer.endpoint_focused() && !has_preedit =>
                        {
                            renderer.move_cursor_left();
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::ArrowRight)
                            if renderer.endpoint_focused() && !has_preedit =>
                        {
                            renderer.move_cursor_right();
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::Home)
                            if renderer.endpoint_focused() && !has_preedit =>
                        {
                            renderer.move_cursor_home();
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::End)
                            if renderer.endpoint_focused() && !has_preedit =>
                        {
                            renderer.move_cursor_end();
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::Delete)
                            if renderer.endpoint_focused() && !has_preedit =>
                        {
                            renderer.delete_at_cursor();
                            window.request_redraw();
                        }
                        Key::Character(s)
                            if renderer.endpoint_focused() && !has_preedit && ctrl =>
                        {
                            let changed = match s.as_str().to_lowercase().as_str() {
                                "a" => {
                                    renderer.select_all_endpoint();
                                    true
                                }
                                "c" => {
                                    if let Some(text) = renderer.selected_text() {
                                        let _ = arboard::Clipboard::new()
                                            .and_then(|mut cb| cb.set_text(text));
                                    }
                                    false
                                }
                                "x" => {
                                    if let Some(text) = renderer.selected_text() {
                                        let _ = arboard::Clipboard::new()
                                            .and_then(|mut cb| cb.set_text(text));
                                        renderer.delete_at_cursor();
                                        true
                                    } else {
                                        false
                                    }
                                }
                                "v" => {
                                    let text = arboard::Clipboard::new()
                                        .and_then(|mut cb| cb.get_text())
                                        .unwrap_or_default();
                                    if !text.is_empty()
                                        && renderer.endpoint_text().len() + text.len() <= 256
                                    {
                                        renderer.insert_text_at_cursor(&text);
                                        true
                                    } else {
                                        false
                                    }
                                }
                                "z" => {
                                    renderer.undo();
                                    true
                                }
                                _ => false,
                            };
                            if changed {
                                window.request_redraw();
                            }
                        }
                        Key::Character(s)
                            if renderer.endpoint_focused() && !has_preedit && !ctrl =>
                        {
                            let mut inserted = false;
                            for ch in s.chars() {
                                if !ch.is_control()
                                    && renderer.endpoint_text().len() + ch.len_utf8() <= 256
                                {
                                    renderer.insert_text_at_cursor(&ch.to_string());
                                    inserted = true;
                                }
                            }
                            if inserted {
                                window.request_redraw();
                            }
                        }
                        _ => {}
                    }
                }
                WindowEvent::Ime(ime)
                    if renderer.settings_visible() && renderer.endpoint_focused() =>
                {
                    match ime {
                        winit::event::Ime::Preedit(text, cursor) => {
                            renderer.set_endpoint_preedit(Some((text.clone(), *cursor)));
                            window.request_redraw();
                        }
                        winit::event::Ime::Commit(text) => {
                            renderer.insert_text_at_cursor(text);
                            window.request_redraw();
                        }
                        winit::event::Ime::Disabled => {
                            renderer.set_endpoint_preedit(None);
                            window.request_redraw();
                        }
                        winit::event::Ime::Enabled => {}
                    }
                }
                _ => {}
            },
            Event::UserEvent(UserEvent::Snapshot(snap)) => {
                if snap.mode != last_mode {
                    renderer.set_mode(last_mode, snap.mode);
                    if config.sound_on {
                        audio.play_for_mode(snap.mode);
                    }
                    last_mode = snap.mode;
                }
                last_snapshot = snap;
                window.request_redraw();
            }
            Event::UserEvent(UserEvent::TrayAction(action)) => {
                handle_tray_action(
                    action,
                    &mut config,
                    &mut renderer,
                    &audio,
                    tray_ui.as_ref(),
                    &dsh_url,
                );
                window.request_redraw();
            }
            Event::AboutToWait => {
                if let Some(action) = tray::poll_action() {
                    let _ = proxy.send_event(UserEvent::TrayAction(action));
                }
                let now = Instant::now();
                if click_tracker.poll_single_click(now) {
                    let _ = open::that(&dsh_url);
                }
                // 设置面板展开时 60fps 持续重绘（光标闪烁、preedit 动画）
                let settings_deadline = if renderer.settings_visible() {
                    Some(now + Duration::from_millis(16))
                } else {
                    None
                };
                let click_deadline = click_tracker.next_deadline();
                match (settings_deadline, click_deadline) {
                    (Some(a), Some(b)) => {
                        elwt.set_control_flow(ControlFlow::WaitUntil(a.min(b)));
                        if a <= b {
                            window.request_redraw();
                        }
                    }
                    (Some(a), None) => {
                        elwt.set_control_flow(ControlFlow::WaitUntil(a));
                        window.request_redraw();
                    }
                    (None, Some(b)) => {
                        elwt.set_control_flow(ControlFlow::WaitUntil(b));
                    }
                    (None, None) => {
                        elwt.set_control_flow(ControlFlow::Wait);
                    }
                }
            }
            Event::LoopExiting => {
                if let Ok(pos) = window.outer_position() {
                    config.window_x = Some(pos.x);
                    config.window_y = Some(pos.y);
                    config.save();
                }
                net_cancel.cancel();
                app_cancel.cancel();
            }
            _ => {}
        }
    })?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_tray_action(
    action: tray::TrayAction,
    config: &mut Config,
    renderer: &mut Renderer,
    audio: &AudioPlayer,
    tray_ui: Option<&tray::TrayUi>,
    dsh_url: &str,
) {
    use tray::TrayAction;
    match action {
        TrayAction::OpenGui => {
            let _ = open::that(dsh_url);
        }
        TrayAction::ToggleBubble => {
            config.bubble_visible = !config.bubble_visible;
            renderer.set_bubble_visible(config.bubble_visible);
            config.save();
        }
        TrayAction::ToggleSound => {
            config.sound_on = !config.sound_on;
            renderer.set_sound_on(config.sound_on);
            if let Some(tray) = tray_ui {
                tray.set_sound_on(config.sound_on);
            }
            config.save();
        }
        TrayAction::TestSound => {
            audio.play_done();
        }
        TrayAction::ScaleUp => {
            set_scale(config, renderer, config.scale + Config::SCALE_STEP);
        }
        TrayAction::ScaleDown => {
            set_scale(config, renderer, config.scale - Config::SCALE_STEP);
        }
        TrayAction::ScaleReset => {
            set_scale(config, renderer, Config::SCALE_DEFAULT);
        }
        TrayAction::Quit => {
            // 注：事件循环退出需要通过 elwt.exit()，此处仅保存配置
            config.save();
            std::process::exit(0);
        }
    }
}

fn set_scale(config: &mut Config, renderer: &mut Renderer, scale: f32) {
    let scale = Config::clamp_scale(scale);
    config.scale = scale;
    renderer.set_pet_scale(scale);
    config.save();
}

/// 若 endpoint 输入框有焦点，提交编辑：校验 → 热切换 → 失焦。
/// 无效输入恢复原值。无焦点时空操作。
#[allow(clippy::too_many_arguments)]
fn commit_endpoint_if_focused(
    renderer: &mut Renderer,
    config: &mut Config,
    dsh_url: &mut String,
    net_cancel: &mut CancellationToken,
    rt: &tokio::runtime::Runtime,
    http: &reqwest::Client,
    event_tx: &mpsc::Sender<StateEvent>,
) {
    if !renderer.endpoint_focused() {
        return;
    }
    let new_endpoint = renderer.endpoint_text().to_string();
    let trimmed = new_endpoint.trim().trim_end_matches('/').to_string();
    if is_valid_endpoint(&trimmed) && trimmed != *dsh_url {
        config.endpoint = trimmed.clone();
        *dsh_url = trimmed;
        config.save();
        net_cancel.cancel();
        *net_cancel = spawn_network_tasks(rt, dsh_url, http, event_tx.clone());
    } else {
        renderer.set_endpoint_text(dsh_url.clone());
    }
    renderer.set_endpoint_focused(false);
}
