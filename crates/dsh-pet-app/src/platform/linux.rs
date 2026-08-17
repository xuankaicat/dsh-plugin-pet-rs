//! Linux 平台适配。

use std::path::PathBuf;

pub fn init() {
    // 检测 Wayland vs X11
    if std::env::var("WAYLAND_DISPLAY").is_ok() && std::env::var("DISPLAY").is_err() {
        tracing::warn!(
            "检测到 Wayland 原生模式，桌宠将不会置顶。建议通过 XWayland 运行以获得完整体验。"
        );
    }
}

pub fn hide_dock() {
    // Linux 无统一 Dock API，空操作
}

/// 获取鼠标的屏幕绝对坐标（物理像素）。Linux 上返回 None，回退到 winit 相对坐标。
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    None
}

/// 查找系统字体：优先用 fc-match 命令
pub fn find_system_font(name: &str) -> Option<PathBuf> {
    // 先尝试常见路径
    let candidates = [
        format!("/usr/share/fonts/{name}"),
        format!("/usr/local/share/fonts/{name}"),
        format!("/usr/share/fonts/opentype/{name}"),
        format!("/usr/share/fonts/truetype/{name}"),
    ];
    for p in &candidates {
        if std::path::Path::new(p).exists() {
            return Some(PathBuf::from(p));
        }
    }
    // 用 fc-match 查找
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", name])
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).to_string();
        if !path.is_empty() && std::path::Path::new(&path).exists() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// 确保窗口位于可见显示器上（该平台暂不干预，系统自行管理窗口位置）。
pub fn ensure_window_on_screen(_window: &winit::window::Window) {}
