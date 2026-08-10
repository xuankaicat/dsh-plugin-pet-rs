//! Windows 平台适配。

use std::path::PathBuf;

pub fn init() {
    // Windows 无特殊初始化
}

pub fn hide_dock() {
    // Windows 无 Dock 概念；skip_taskbar 已在窗口属性中设置
}

/// 获取鼠标的屏幕绝对坐标（物理像素）。
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point).ok()?;
    }
    Some((point.x, point.y))
}

/// 查找系统字体
pub fn find_system_font(name: &str) -> Option<PathBuf> {
    let win_dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
    // 文件名别名映射（字体名 → 实际文件名）
    let aliases: &[(&str, &str)] = &[
        ("Microsoft YaHei", "msyh.ttc"),
        ("SimHei", "simhei.ttf"),
        ("Microsoft JhengHei", "msjh.ttc"),
    ];
    // 先查别名
    for (n, file) in aliases {
        if name.eq_ignore_ascii_case(n) {
            let p = format!("{win_dir}\\Fonts\\{file}");
            if std::path::Path::new(&p).exists() {
                return Some(PathBuf::from(p));
            }
        }
    }
    // 回退：直接用字体名拼路径
    let candidates = [
        format!("{win_dir}\\Fonts\\{}.ttf", name.replace(' ', "")),
        format!("{win_dir}\\Fonts\\{}.ttc", name.replace(' ', "")),
        format!("{win_dir}\\Fonts\\{}.otf", name.replace(' ', "")),
    ];
    candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(PathBuf::from)
}
