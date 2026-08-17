//! DSH 桌宠主入口。
//!
//! 对应 main.js 的主流程：单实例 → 配置 → 资源 → tokio 任务 → winit 事件循环。
//
// winit 0.30 的 EventLoop::run / create_window 已标记 deprecated（建议用 run_app），
// v1 仍用 run + 闭包形式（迁移到 ApplicationHandler trait 是 v2 待办）。
//
// release 版在 Windows 上以 GUI 子系统链接：双击 exe 纯 GUI 启动，不弹控制台窗口（CLI）。
// debug 构建保持控制台子系统，便于开发时在终端查看日志。
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![allow(deprecated)]

mod audio;
mod child_dsh;
mod platform;
mod shot_mode;
mod tasks;
mod tray;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsh_pet_core::{Config, Mode, PetState, Snapshot, StateEvent};
use dsh_pet_ui::{
    create_window, ClickTarget, ContextMenuAction, InputAction, InputState, Renderer, SettingsHit,
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
    /// DSH 子进程就绪，携带实际地址
    DshChildUrl(String),
    /// DSH 子进程启动失败
    DshChildFailed(String),
}

/// 透明区域点击穿透的轮询间隔（毫秒）
const CLICK_THROUGH_POLL_MS: u64 = 100;

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
    // 无论如何先保证桌宠显示在可见屏幕上（winit 在本机监视器尺寸/DPI 有误，可能把窗口放到屏幕外）
    platform::ensure_window_on_screen(&window);

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
    renderer.set_spawn_dsh(config.spawn_dsh);
    let mut input = InputState::new();
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
    // 当前窗口是否为点击穿透状态（避免重复切换窗口样式）
    let mut click_through = false;
    // 由桌宠启动的 DSH 子进程（共享槽；关闭开关/退出时回收）
    let dsh_child: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
    // 启动任务代际号：关闭/重启时 +1，使在途的旧启动任务失效
    let dsh_gen = Arc::new(AtomicU64::new(0));
    // 启动时检测默认端口是否已有 DSH：有则关闭子进程模式，直接连接现有实例
    if config.spawn_dsh && dsh_server_running(&rt, &http) {
        tracing::info!(
            "检测到 {} 已有 DSH 服务，关闭「由桌宠启动 DSH」并直接连接",
            Config::ENDPOINT_DEFAULT
        );
        config.spawn_dsh = false;
        renderer.set_spawn_dsh(false);
        config.save();
    }
    if config.spawn_dsh {
        spawn_dsh_task(&rt, &proxy, &dsh_child, &dsh_gen, dsh_gen.load(Ordering::SeqCst));
    }

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
                    if Renderer::is_animating(last_snapshot.mode) || renderer.bubble_animating()
                    {
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
                    // 面板内右键菜单可见时：左键优先命中菜单项，点击外部关闭菜单
                    let menu_handled = if renderer.menu_visible()
                        && *state == winit::event::ElementState::Released
                    {
                        let (x, y) = input.cursor_position();
                        let menu_action = renderer.menu_action_at(x, y);
                        renderer.set_menu_visible(false);
                        if let Some(ma) = menu_action {
                            handle_menu_action(
                                ma,
                                &mut config,
                                &mut renderer,
                                &audio,
                                tray_ui.as_ref(),
                                &dsh_url,
                                &window,
                                elwt,
                            );
                        }
                        window.request_redraw();
                        true
                    } else {
                        false
                    };
                    if !menu_handled {
                        match action {
                        InputAction::Click(ClickTarget::Whale) => {
                            // 单击鲸鱼：先提交 endpoint 编辑
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
                            if renderer.settings_visible() {
                                // 设置面板打开时，单击鲸鱼 → 关闭面板（丢弃未提交编辑）
                                renderer.set_endpoint_focused(false);
                                window.set_ime_allowed(false);
                                renderer.set_endpoint_text(dsh_url.clone());
                                renderer.set_settings_visible(false);
                            } else {
                                // 否则切换气泡显示/隐藏（无延迟）
                                toggle_bubble(&mut config, &mut renderer, tray_ui.as_ref());
                            }
                            window.request_redraw();
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
                                    SettingsHit::SpawnDsh => {
                                        if config.spawn_dsh {
                                            // 关闭子进程模式：先弹面板内确认（不再用原生 MessageBox）
                                            renderer.set_confirm_stop_dsh(true);
                                            window.request_redraw();
                                        } else {
                                            // 开启子进程模式：地址框显示「启动中…」，后台拉起
                                            config.spawn_dsh = true;
                                            renderer.set_spawn_dsh(true);
                                            renderer.set_endpoint_text("启动中…".into());
                                            spawn_dsh_task(
                                                &rt,
                                                &proxy,
                                                &dsh_child,
                                                &dsh_gen,
                                                dsh_gen.load(Ordering::SeqCst),
                                            );
                                            config.save();
                                            window.request_redraw();
                                        }
                                    }
                                    SettingsHit::ConfirmStopDshYes => {
                                        // 确认关闭：停止子进程，恢复手动地址
                                        renderer.set_confirm_stop_dsh(false);
                                        config.spawn_dsh = false;
                                        renderer.set_spawn_dsh(false);
                                        stop_dsh_child(&rt, &dsh_child, &dsh_gen);
                                        let manual = config.normalized_endpoint();
                                        if dsh_url != manual {
                                            dsh_url = manual.clone();
                                            renderer.set_endpoint_text(manual.clone());
                                            net_cancel.cancel();
                                            net_cancel = spawn_network_tasks(
                                                &rt, &dsh_url, &http, event_tx.clone(),
                                            );
                                        }
                                        config.save();
                                        window.request_redraw();
                                    }
                                    SettingsHit::ConfirmStopDshNo => {
                                        // 取消：保持开启
                                        renderer.set_confirm_stop_dsh(false);
                                        window.request_redraw();
                                    }
                                    SettingsHit::RestartDsh => {
                                        // 重启桌宠托管的 DSH 子进程（仅 spawn_dsh 模式显示按钮）
                                        if config.spawn_dsh {
                                            stop_dsh_child(&rt, &dsh_child, &dsh_gen);
                                            renderer.set_endpoint_text("重启中…".into());
                                            spawn_dsh_task(
                                                &rt,
                                                &proxy,
                                                &dsh_child,
                                                &dsh_gen,
                                                dsh_gen.load(Ordering::SeqCst),
                                            );
                                            window.request_redraw();
                                        }
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
                            // 右键：托盘创建失败（如 Linux GNOME 无扩展）时打开面板内菜单；
                            // 否则切换设置面板开/关（丢弃未提交的 endpoint 编辑）
                            if renderer.menu_visible() {
                                renderer.set_menu_visible(false);
                            } else if tray_ui.is_none() {
                                let (x, y) = input.cursor_position();
                                renderer.open_menu(x as f32, y as f32);
                            } else {
                                renderer.set_endpoint_focused(false);
                                window.set_ime_allowed(false);
                                renderer.set_endpoint_text(dsh_url.clone());
                                renderer.set_settings_visible(!renderer.settings_visible());
                            }
                            window.request_redraw();
                        }
                            _ => {}
                        }
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
                        && (renderer.settings_visible() || renderer.menu_visible()) =>
                {
                    let has_preedit = renderer.endpoint_preedit().is_some();
                    let ctrl = modifiers.control_key();
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            if renderer.menu_visible() {
                                renderer.set_menu_visible(false);
                            } else if renderer.endpoint_focused() {
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
                    elwt,
                );
                window.request_redraw();
            }
            Event::UserEvent(UserEvent::DshChildUrl(url)) => {
                // DSH 子进程就绪 → 热切换连接
                if !url.is_empty() && url != dsh_url {
                    tracing::info!("DSH 子进程就绪: {url}");
                    dsh_url = url.clone();
                    renderer.set_endpoint_text(url.clone());
                    net_cancel.cancel();
                    net_cancel = spawn_network_tasks(&rt, &dsh_url, &http, event_tx.clone());
                }
                window.request_redraw();
            }
            Event::UserEvent(UserEvent::DshChildFailed(msg)) => {
                tracing::error!("DSH 子进程启动失败: {msg}");
                renderer.set_endpoint_text("启动失败".into());
                window.request_redraw();
            }
            Event::AboutToWait => {
                if let Some(action) = tray::poll_action() {
                    let _ = proxy.send_event(UserEvent::TrayAction(action));
                }
                let now = Instant::now();
                // 透明区域点击穿透：轮询光标下像素，动态切换窗口命中测试
                update_click_through(&window, &renderer, &input, &mut click_through);
                // 气泡浮入/浮出动画期间 60fps 持续重绘
                let bubble_deadline = if renderer.bubble_animating() {
                    Some(now + Duration::from_millis(16))
                } else {
                    None
                };
                // 设置面板展开时 60fps 持续重绘（光标闪烁、preedit 动画）
                let settings_deadline = if renderer.settings_visible() {
                    Some(now + Duration::from_millis(16))
                } else {
                    None
                };
                let needs_redraw = bubble_deadline.is_some() || settings_deadline.is_some();
                let deadline = bubble_deadline.into_iter().chain(settings_deadline).min();
                // 至少每 100ms 唤醒一次：点击穿透时窗口收不到鼠标事件，需靠轮询恢复命中
                let poll_deadline = now + Duration::from_millis(CLICK_THROUGH_POLL_MS);
                match deadline {
                    Some(d) => {
                        elwt.set_control_flow(ControlFlow::WaitUntil(d.min(poll_deadline)));
                        if needs_redraw {
                            window.request_redraw();
                        }
                    }
                    None => {
                        elwt.set_control_flow(ControlFlow::WaitUntil(poll_deadline));
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
                // 回收由桌宠启动的 DSH 子进程
                stop_dsh_child_on_exit(&rt, &dsh_child, &dsh_gen);
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
    elwt: &winit::event_loop::ActiveEventLoop,
) {
    use tray::TrayAction;
    match action {
        TrayAction::OpenGui => {
            let _ = open::that(dsh_url);
        }
        TrayAction::OpenSettings => {
            renderer.set_endpoint_focused(false);
            renderer.set_menu_visible(false);
            renderer.set_endpoint_text(dsh_url.to_string());
            renderer.set_settings_visible(true);
        }
        TrayAction::ToggleBubble => {
            toggle_bubble(config, renderer, tray_ui);
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
            // 通过 elwt.exit() 正常退出，让 LoopExiting 回收 DSH 子进程并保存配置
            config.save();
            elwt.exit();
        }
    }
}

/// 处理面板内右键菜单动作（托盘创建失败时的兜底菜单）。
#[allow(clippy::too_many_arguments)]
fn handle_menu_action(
    action: ContextMenuAction,
    config: &mut Config,
    renderer: &mut Renderer,
    audio: &AudioPlayer,
    tray_ui: Option<&tray::TrayUi>,
    dsh_url: &str,
    window: &winit::window::Window,
    elwt: &winit::event_loop::ActiveEventLoop,
) {
    match action {
        ContextMenuAction::OpenSettings => {
            renderer.set_endpoint_focused(false);
            window.set_ime_allowed(false);
            renderer.set_endpoint_text(dsh_url.to_string());
            renderer.set_settings_visible(true);
        }
        other => {
            use tray::TrayAction;
            let ta = match other {
                ContextMenuAction::OpenGui => TrayAction::OpenGui,
                ContextMenuAction::ToggleBubble => TrayAction::ToggleBubble,
                ContextMenuAction::ToggleSound => TrayAction::ToggleSound,
                ContextMenuAction::TestSound => TrayAction::TestSound,
                ContextMenuAction::ScaleUp => TrayAction::ScaleUp,
                ContextMenuAction::ScaleDown => TrayAction::ScaleDown,
                ContextMenuAction::ScaleReset => TrayAction::ScaleReset,
                ContextMenuAction::Quit => TrayAction::Quit,
                ContextMenuAction::OpenSettings => unreachable!(),
            };
            handle_tray_action(ta, config, renderer, audio, tray_ui, dsh_url, elwt);
        }
    }
}

fn set_scale(config: &mut Config, renderer: &mut Renderer, scale: f32) {
    let scale = Config::clamp_scale(scale);
    config.scale = scale;
    renderer.set_pet_scale(scale);
    config.save();
}

/// 切换气泡显示/隐藏（单击鲸鱼、托盘菜单共用），并同步托盘菜单文案。
fn toggle_bubble(config: &mut Config, renderer: &mut Renderer, tray_ui: Option<&tray::TrayUi>) {
    config.bubble_visible = !config.bubble_visible;
    renderer.set_bubble_visible(config.bubble_visible);
    if let Some(tray) = tray_ui {
        tray.set_bubble_visible(config.bubble_visible);
    }
    config.save();
}

/// 透明区域点击穿透：定期检查光标下像素是否完全透明，
/// 动态切换窗口是否接收鼠标事件（Windows 上通过 WS_EX_TRANSPARENT 实现）。
fn update_click_through(
    window: &winit::window::Window,
    renderer: &Renderer,
    input: &InputState,
    current: &mut bool,
) {
    let transparent = if input.is_down() {
        // 拖拽/按下期间保持可交互，避免穿透打断拖拽
        false
    } else {
        match (platform::cursor_screen_position(), window.inner_position()) {
            (Some((sx, sy)), Ok(inner)) => {
                let cx = sx - inner.x;
                let cy = sy - inner.y;
                renderer.is_transparent_at(cx as f32, cy as f32)
            }
            _ => false,
        }
    };
    if *current != transparent {
        *current = transparent;
        // hittest = true → 接收鼠标事件（不穿透）
        let _ = window.set_cursor_hittest(!transparent);
    }
}

/// 后台启动 DSH 子进程：内部级联尝试「dsh」→「npx @deepseek-ai/dsh」，
/// 成功后把子进程放入共享槽并通知事件循环热切换连接。
fn spawn_dsh_task(
    rt: &tokio::runtime::Runtime,
    proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
    slot: &Arc<Mutex<Option<tokio::process::Child>>>,
    gen: &Arc<AtomicU64>,
    generation: u64,
) {
    let proxy = proxy.clone();
    let slot = slot.clone();
    let gen = gen.clone();
    rt.spawn(async move {
        match child_dsh::start().await {
            Ok(spawned) => {
                if gen.load(Ordering::SeqCst) == generation {
                    *slot.lock().unwrap() = Some(spawned.child);
                    let _ = proxy.send_event(UserEvent::DshChildUrl(spawned.url));
                } else {
                    // 已被关闭/重启取代：回收这个过期子进程
                    let mut child = spawned.child;
                    child_dsh::kill(&mut child).await;
                }
            }
            Err(msg) => {
                if gen.load(Ordering::SeqCst) == generation {
                    tracing::error!("DSH 子进程启动失败: {msg}");
                    let _ = proxy.send_event(UserEvent::DshChildFailed(msg));
                }
            }
        }
    });
}

/// 停止 DSH 子进程（若有），并让在途的启动任务失效（代际 +1）。
/// 回收在后台异步进行，不阻塞事件循环（避免 taskkill 卡住界面）。
fn stop_dsh_child(
    rt: &tokio::runtime::Runtime,
    slot: &Arc<Mutex<Option<tokio::process::Child>>>,
    gen: &Arc<AtomicU64>,
) {
    gen.fetch_add(1, Ordering::SeqCst);
    let child = slot.lock().unwrap().take();
    if let Some(mut c) = child {
        rt.spawn(async move {
            child_dsh::kill(&mut c).await;
        });
    }
}

/// 桌宠退出时同步回收 DSH 子进程（进程即将结束，阻塞可接受，确保子进程被回收）。
fn stop_dsh_child_on_exit(
    rt: &tokio::runtime::Runtime,
    slot: &Arc<Mutex<Option<tokio::process::Child>>>,
    gen: &Arc<AtomicU64>,
) {
    gen.fetch_add(1, Ordering::SeqCst);
    let child = slot.lock().unwrap().take();
    if let Some(mut c) = child {
        rt.block_on(child_dsh::kill(&mut c));
    }
}

/// 快速探测默认端口是否已有 DSH 服务（任何 <500 的 HTTP 响应都视为已占用）。
fn dsh_server_running(rt: &tokio::runtime::Runtime, http: &reqwest::Client) -> bool {
    let url = format!("{}/", Config::ENDPOINT_DEFAULT);
    rt.block_on(async {
        match tokio::time::timeout(Duration::from_millis(1500), http.get(&url).send()).await {
            Ok(Ok(resp)) => {
                tracing::info!("启动探测 {} → HTTP {}", url, resp.status());
                resp.status().as_u16() < 500
            }
            Ok(Err(e)) => {
                tracing::info!("启动探测 {} 失败: {e}", url);
                false
            }
            Err(_) => {
                tracing::info!("启动探测 {} 超时", url);
                false
            }
        }
    })
}

// 关闭「由桌宠启动 DSH」的确认已改为面板内确认框（renderer 绘制），
// 不再使用原生 MessageBox：避免被置顶窗口挡住、显示延迟及风格不一致问题。

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
