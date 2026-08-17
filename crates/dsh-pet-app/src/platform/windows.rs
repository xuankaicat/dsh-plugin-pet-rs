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

/// 确保窗口位于某个可见显示器的工作区内。
///
/// winit 在本机的监视器物理尺寸/DPI 读取可能与系统不一致（会把 2560×1440 的屏幕
/// 报成 3840×2160），导致右下角定位到屏幕外、桌宠“不显示”。这里用原生 API 拿
/// 真实的屏幕几何：若窗口与所有显示器工作区的可见面积不足 1/4，就移回主显示器
/// 工作区右下角——无论如何都保证桌宠先显示出来。
pub fn ensure_window_on_screen(window: &winit::window::Window) {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SetWindowPos, HWND_TOP, MONITORINFOF_PRIMARY, SWP_NOACTIVATE, SWP_NOSIZE,
        SWP_NOZORDER,
    };

    unsafe {
        // winit 0.30 通过 raw-window-handle 暴露 HWND
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        let hwnd = HWND(win32.hwnd.get() as *mut _);
        let mut win_rect = RECT::default();
        if GetWindowRect(hwnd, &mut win_rect).is_err() {
            return;
        }
        let win_w = win_rect.right - win_rect.left;
        let win_h = win_rect.bottom - win_rect.top;
        let win_area = (win_w as i64) * (win_h as i64);

        #[derive(Default)]
        struct Check {
            win_rect: RECT,
            max_overlap: i64,
            primary_work: Option<RECT>,
        }

        unsafe extern "system" fn enum_proc(
            hmon: HMONITOR,
            _hdc: HDC,
            _lprc: *mut RECT,
            lparam: LPARAM,
        ) -> BOOL {
            unsafe {
                let check = &mut *(lparam.0 as *mut Check);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(hmon, &mut info).as_bool() {
                    let a = check.win_rect;
                    let b = info.rcWork;
                    let ix = (a.right.min(b.right) - a.left.max(b.left)).max(0) as i64;
                    let iy = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0) as i64;
                    check.max_overlap = check.max_overlap.max(ix * iy);
                    if info.dwFlags & MONITORINFOF_PRIMARY != 0 {
                        check.primary_work = Some(info.rcWork);
                    }
                }
                BOOL(1)
            }
        }

        let mut check = Check {
            win_rect,
            max_overlap: 0,
            primary_work: None,
        };
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_proc),
            LPARAM(&mut check as *mut Check as isize),
        );

        // 窗口与任一工作区的可见面积不足 1/4 → 判定在屏幕外，移回主显示器工作区右下角
        if win_area > 0 && check.max_overlap * 4 < win_area {
            if let Some(work) = check.primary_work {
                let x = work.right - win_w - 18;
                let y = work.bottom - win_h - 24;
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    x,
                    y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                tracing::info!("窗口位置在屏幕外，已移回主显示器右下角: ({x}, {y})");
            }
        }
    }
}
