//! 平台适配层：字体加载、custom 目录、Dock 隐藏、Wayland 检测。

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
pub use windows::*;

use std::path::PathBuf;

/// custom/ 目录路径：与可执行文件同目录
pub fn custom_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            return parent.join("custom");
        }
    }
    PathBuf::from("custom")
}

/// 加载字体：优先系统 CJK 字体，回退到内嵌子集。
///
/// v1 偏离 spec：不内嵌 Noto Sans CJK 子集（需 HarfBuzz subsetting 工具链），
/// 仅依赖系统字体。三端主流系统均有内置 CJK 字体，影响可控。
pub fn load_font() -> ab_glyph::FontArc {
    let candidates = platform_font_names();
    for name in candidates {
        if let Some(path) = find_system_font(name) {
            match std::fs::read(&path) {
                Ok(data) => {
                    if let Ok(font) = ab_glyph::FontArc::try_from_vec(data) {
                        tracing::info!("使用系统字体: {name} ({})", path.display());
                        return font;
                    }
                }
                Err(e) => tracing::warn!("读取字体 {name} 失败: {e}"),
            }
        }
    }
    tracing::warn!("未找到系统 CJK 字体，CJK 字符可能无法渲染");
    // v1 偏离 spec：不内嵌 Noto Sans CJK 子集（需 HarfBuzz subsetting 工具链）。
    // 如系统无 CJK 字体，直接 panic 并给出安装指引。
    panic!(
        "无可用的 CJK 字体。请安装以下任一字体：\n  \
         Windows: Microsoft YaHei / SimHei\n  \
         macOS: PingFang SC / Heiti SC\n  \
         Linux: Noto Sans CJK SC / WenQuanYi Micro Hei"
    );
}

fn platform_font_names() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &["PingFang SC", "Heiti SC", "STHeiti", "Hiragino Sans GB"]
    }
    #[cfg(target_os = "windows")]
    {
        &["Microsoft YaHei", "SimHei", "Microsoft JhengHei"]
    }
    #[cfg(target_os = "linux")]
    {
        &[
            "Noto Sans CJK SC",
            "Source Han Sans SC",
            "WenQuanYi Micro Hei",
            "Droid Sans Fallback",
        ]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        &[]
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn init() {}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn hide_dock() {}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn find_system_font(_name: &str) -> Option<PathBuf> {
    None
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
/// 获取鼠标屏幕绝对坐标。不支持的平台返回 None，回退到 winit 相对坐标。
pub fn cursor_screen_position() -> Option<(i32, i32)> {
    None
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn ensure_window_on_screen(_window: &winit::window::Window) {}
