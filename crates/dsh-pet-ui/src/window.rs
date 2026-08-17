//! 窗口创建：透明无边框置顶悬浮窗。
//!
//! 对应 main.js L412-448 的 createWindow()。
//
// winit 0.30 的 EventLoop::create_window 已标记 deprecated（建议用 ActiveEventLoop::create_window），
// 但为保持与 main.rs 中 EventLoop::run 闭包形式的一致性，v1 仍用此 API。
#![allow(deprecated)]

use dsh_pet_core::Config;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::EventLoop;
use winit::window::{Window, WindowLevel};

/// 创建桌宠窗口（透明、无边框、置顶、不抢焦点）。
///
/// 泛型 `T` 为用户事件类型，由 app crate 定义。
pub fn create_window<T>(el: &EventLoop<T>, config: &Config) -> anyhow::Result<Window> {
    let mut builder = Window::default_attributes()
        .with_title("DSH 桌宠")
        // 窗口 280×372：加高以容纳设置面板三行开关（声音/帧率/由桌宠启动 DSH）；
        // pet_scale 只缩放鲸鱼舞台，不缩放窗口/气泡。
        .with_inner_size(LogicalSize::new(280u32, 372u32))
        .with_transparent(true)
        .with_decorations(false)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_active(false); // 启动不抢焦点

    // 平台特定属性
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        builder = builder.with_skip_taskbar(true);
    }

    let window = el.create_window(builder)?;

    // 定位：优先用保存的位置，否则右下角
    if let (Some(x), Some(y)) = (config.window_x, config.window_y) {
        window.set_outer_position(LogicalPosition::new(x, y));
    } else {
        position_bottom_right(&window);
    }

    Ok(window)
}

/// 把窗口定位到主显示器右下角（留 18px 右边距、24px 下边距）
fn position_bottom_right(window: &Window) {
    if let Some(monitor) = window.current_monitor() {
        let monitor_size = monitor.size();
        let monitor_pos = monitor.position();
        let win_size = window.outer_size();
        // monitor.size() 和 outer_size() 都是物理像素，直接计算物理坐标
        let x = monitor_pos.x + monitor_size.width as i32 - win_size.width as i32 - 18;
        let y = monitor_pos.y + monitor_size.height as i32 - win_size.height as i32 - 24;
        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    }
}
