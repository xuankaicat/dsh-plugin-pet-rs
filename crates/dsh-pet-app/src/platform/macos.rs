//! macOS 平台适配。

use std::path::PathBuf;

pub fn init() {
    // macOS 无特殊初始化
}

/// 隐藏 Dock 图标（等价于 app.dock.hide()）
pub fn hide_dock() {
    // 使用 objc 调用 NSApplication.setActivationPolicy(.accessory)
    // 这会隐藏 Dock 图标但不影响菜单栏
    unsafe {
        let app: *mut objc::runtime::Object =
            objc::msg_send![objc::class!(NSApplication), sharedApplication];
        let _: () = objc::msg_send![app, setActivationPolicy: 0]; // NSApplicationActivationPolicyAccessory = 0
    }
}

/// 获取鼠标的屏幕绝对坐标（物理像素）。macOS 上返回 None，回退到 winit 相对坐标。
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    None
}

/// 查找系统字体
pub fn find_system_font(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let candidates = [
        format!("/System/Library/Fonts/{name}.ttf"),
        format!("/System/Library/Fonts/{name}.otf"),
        format!("/System/Library/Fonts/Supplemental/{name}.ttf"),
        format!("/Library/Fonts/{name}.ttf"),
        format!("{home}/Library/Fonts/{name}.ttf"),
    ];
    candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(PathBuf::from)
}
