//! 系统托盘：tray-icon crate（三端兼容）。
//!
//! 对应 main.js L314-374 的 trayIcon() + buildMenu()。

use std::sync::Mutex;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// 托盘菜单动作
#[derive(Debug, Clone)]
pub enum TrayAction {
    OpenGui,
    ToggleBubble,
    ToggleSound(bool),
    TestSound,
    ScaleUp,
    ScaleDown,
    ScaleReset,
    Quit,
}

/// 托盘菜单项 ID（用于事件匹配）
struct MenuIds {
    open_gui: MenuId,
    toggle_bubble: MenuId,
    sound: MenuId,
    test_sound: MenuId,
    scale_up: MenuId,
    scale_down: MenuId,
    scale_reset: MenuId,
    quit: MenuId,
}

/// 全局存储菜单项 ID（tray-icon 的事件回调是全局静态的）
static MENU_IDS: Mutex<Option<MenuIds>> = Mutex::new(None);

/// 创建托盘图标和菜单。
///
/// 返回 `(TrayIcon, Vec<TrayAction>)` — TrayIcon 必须存活，Vec 用于收集初始化时的动作（通常为空）。
/// 后续菜单事件通过 `tray_icon::menu::MenuEvent::receiver()` 全局通道获取。
pub fn create(sound_on: bool, bubble_visible: bool, scale: f32) -> Option<TrayIcon> {
    let icon = load_tray_icon();

    let menu = Menu::new();
    let open_gui = MenuItem::new("打开 DSH GUI", true, None);
    let toggle_bubble = MenuItem::new(
        if bubble_visible {
            "隐藏气泡"
        } else {
            "显示气泡"
        },
        true,
        None,
    );
    let sound = MenuItem::new(
        if sound_on {
            "✓ 状态提示音"
        } else {
            "  状态提示音"
        },
        true,
        None,
    );
    let test_sound = MenuItem::new("测试提示音", true, None);
    let sep1 = PredefinedMenuItem::separator();
    let scale_label = MenuItem::new(format!("鲸鱼大小 {}%", (scale * 100.0) as u32), false, None);
    let scale_up = MenuItem::new("  放大", true, None);
    let scale_down = MenuItem::new("  缩小", true, None);
    let scale_reset = MenuItem::new("  重置（默认 67%）", true, None);
    let sep2 = PredefinedMenuItem::separator();
    let quit = MenuItem::new("退出桌宠", true, None);

    let _ = menu.append(&open_gui);
    let _ = menu.append(&toggle_bubble);
    let _ = menu.append(&sound);
    let _ = menu.append(&test_sound);
    let _ = menu.append(&sep1);
    let _ = menu.append(&scale_label);
    let _ = menu.append(&scale_up);
    let _ = menu.append(&scale_down);
    let _ = menu.append(&scale_reset);
    let _ = menu.append(&sep2);
    let _ = menu.append(&quit);

    // 存储菜单项 ID 供事件匹配
    {
        let mut ids = MENU_IDS.lock().unwrap();
        *ids = Some(MenuIds {
            open_gui: open_gui.id().clone(),
            toggle_bubble: toggle_bubble.id().clone(),
            sound: sound.id().clone(),
            test_sound: test_sound.id().clone(),
            scale_up: scale_up.id().clone(),
            scale_down: scale_down.id().clone(),
            scale_reset: scale_reset.id().clone(),
            quit: quit.id().clone(),
        });
    }

    let tray = TrayIconBuilder::new()
        .with_tooltip("DSH 桌宠")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()
        .ok()?;

    Some(tray)
}

/// 尝试从全局 MenuEvent 通道轮询一个托盘动作
pub fn poll_action() -> Option<TrayAction> {
    let event = MenuEvent::receiver().try_recv().ok()?;
    let ids = MENU_IDS.lock().unwrap();
    let ids = ids.as_ref()?;
    let action = if event.id == ids.open_gui {
        TrayAction::OpenGui
    } else if event.id == ids.toggle_bubble {
        TrayAction::ToggleBubble
    } else if event.id == ids.sound {
        // 切换提示音状态（具体 on/off 由调用方维护）
        TrayAction::ToggleSound(true) // 占位，实际值由调用方决定
    } else if event.id == ids.test_sound {
        TrayAction::TestSound
    } else if event.id == ids.scale_up {
        TrayAction::ScaleUp
    } else if event.id == ids.scale_down {
        TrayAction::ScaleDown
    } else if event.id == ids.scale_reset {
        TrayAction::ScaleReset
    } else if event.id == ids.quit {
        TrayAction::Quit
    } else {
        return None;
    };
    Some(action)
}

/// 加载托盘图标（从内嵌 PNG）
fn load_tray_icon() -> Icon {
    let png = include_bytes!("../../../assets/tray-icon.png");
    let img = image::load_from_memory(png)
        .expect("托盘图标 PNG 损坏")
        .to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).expect("托盘图标 RGBA 转换失败")
}
