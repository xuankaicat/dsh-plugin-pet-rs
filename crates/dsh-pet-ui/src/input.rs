//! 鼠标输入状态机：区分单击 / 右键 / 拖拽 / 滚动。
//!
//! 对应 renderer.js L113-177 的拖拽 + 点击逻辑。

use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickTarget {
    Whale,
    Bubble,
}

/// 输入事件产生的动作
#[derive(Debug, Clone)]
pub enum InputAction {
    None,
    /// 单击鲸鱼 → 切换气泡显示/隐藏；单击气泡 → 打开 GUI
    Click(ClickTarget),
    /// 右键 → 上下文菜单
    ContextMenu,
    /// 拖拽中（增量 dx, dy）
    Dragging {
        dx: i32,
        dy: i32,
    },
    /// 拖拽结束
    DragEnd,
    /// 气泡内滚动
    Scroll {
        delta: f32,
    },
}

pub struct InputState {
    down: bool,
    moved: bool,
    start_screen_x: f64,
    start_screen_y: f64,
    last_screen_x: f64,
    last_screen_y: f64,
    down_target: Option<ClickTarget>,
    /// 按下时的窗口屏幕坐标（由 main 在 MouseInput Pressed 时设置）
    drag_origin: Option<(i32, i32)>,
    /// 鼠标是否在气泡区域内
    pub in_bubble: bool,
    /// 鼠标是否在鲸鱼舞台区域内
    pub in_whale: bool,
    /// 拖拽触发阈值（像素）
    drag_threshold: f64,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl InputState {
    pub fn new() -> Self {
        Self {
            down: false,
            moved: false,
            start_screen_x: 0.0,
            start_screen_y: 0.0,
            last_screen_x: 0.0,
            last_screen_y: 0.0,
            down_target: None,
            drag_origin: None,
            in_bubble: false,
            in_whale: false,
            drag_threshold: 8.0,
        }
    }

    /// 更新鼠标位置（由 CursorMoved 事件驱动），更新 in_bubble / in_whale 状态
    pub fn update_cursor(&mut self, x: f64, y: f64) {
        self.last_screen_x = x;
        self.last_screen_y = y;
    }

    pub fn set_hit_regions(&mut self, in_bubble: bool, in_whale: bool) {
        self.in_bubble = in_bubble;
        self.in_whale = in_whale;
    }

    pub fn cursor_position(&self) -> (f64, f64) {
        (self.last_screen_x, self.last_screen_y)
    }

    /// 设置拖拽起始时的窗口屏幕坐标（由 main 在按下时调用）。
    pub fn set_drag_origin(&mut self, origin: (i32, i32)) {
        self.drag_origin = Some(origin);
    }

    pub fn drag_origin(&self) -> Option<(i32, i32)> {
        self.drag_origin
    }

    pub fn clear_drag_origin(&mut self) {
        self.drag_origin = None;
    }

    pub fn is_down(&self) -> bool {
        self.down
    }

    pub fn is_moved(&self) -> bool {
        self.moved
    }

    /// 设置按下时的鼠标屏幕绝对坐标（由 main 在按下时调用）。
    pub fn set_start_mouse_screen(&mut self, x: f64, y: f64) {
        self.start_screen_x = x;
        self.start_screen_y = y;
    }

    /// 返回 (start_mouse_screen, drag_origin)，供 main 计算窗口新位置。
    pub fn drag_full_state(&self) -> Option<((f64, f64), (i32, i32))> {
        self.drag_origin
            .map(|origin| ((self.start_screen_x, self.start_screen_y), origin))
    }

    /// 检查鼠标是否移动超过阈值，设置 moved 标志。
    pub fn check_moved(&mut self, cur_x: f64, cur_y: f64) {
        if self.moved {
            return;
        }
        let dx = (cur_x - self.start_screen_x).abs();
        let dy = (cur_y - self.start_screen_y).abs();
        if dx > self.drag_threshold || dy > self.drag_threshold {
            self.moved = true;
        }
    }

    /// 处理 winit 窗口事件，返回产生的动作。
    ///
    /// 注：CursorMoved 的拖拽增量请用 `drag_delta()` 辅助函数计算；
    /// 此方法仅处理 MouseInput（点击/右键）和 MouseWheel（滚动）。
    pub fn handle(
        &mut self,
        event: &WindowEvent,
        is_whale_hit: impl Fn(f64, f64) -> bool,
    ) -> InputAction {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.last_screen_x = position.x;
                self.last_screen_y = position.y;
                InputAction::None
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    // 按下时用区域级命中（in_whale / in_bubble），允许在鲸鱼透明区域也开始拖拽。
                    // 点击判定（Click）在 Released 时用像素级命中。
                    // 注：start_screen_x/y 由 main 在按下后通过 set_start_mouse_screen 设置
                    // （需要窗口屏幕坐标 + 相对坐标 = 屏幕绝对坐标）。
                    let target = if self.in_whale {
                        Some(ClickTarget::Whale)
                    } else if self.in_bubble {
                        Some(ClickTarget::Bubble)
                    } else {
                        None
                    };
                    if target.is_none() {
                        return InputAction::None;
                    }
                    self.down = true;
                    self.moved = false;
                    self.down_target = target;
                    InputAction::None
                }
                (MouseButton::Left, ElementState::Released) => {
                    let action = if self.down && !self.moved {
                        // 未移动 → 判定为点击。鲸鱼需要像素级命中，气泡只需区域命中。
                        match self.down_target {
                            Some(ClickTarget::Whale) => {
                                if is_whale_hit(self.last_screen_x, self.last_screen_y) {
                                    InputAction::Click(ClickTarget::Whale)
                                } else {
                                    InputAction::None
                                }
                            }
                            Some(ClickTarget::Bubble) => InputAction::Click(ClickTarget::Bubble),
                            None => InputAction::None,
                        }
                    } else if self.down {
                        InputAction::DragEnd
                    } else {
                        InputAction::None
                    };
                    self.down = false;
                    self.moved = false;
                    self.down_target = None;
                    action
                }
                (MouseButton::Right, ElementState::Released) => {
                    if self.in_whale && !is_whale_hit(self.last_screen_x, self.last_screen_y) {
                        return InputAction::None;
                    }
                    if !self.in_whale && !self.in_bubble {
                        return InputAction::None;
                    }
                    InputAction::ContextMenu
                }
                _ => InputAction::None,
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if self.in_bubble {
                    let y = match delta {
                        MouseScrollDelta::LineDelta(_, y) => *y * 20.0,
                        MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
                    };
                    InputAction::Scroll { delta: y }
                } else {
                    InputAction::None
                }
            }
            _ => InputAction::None,
        }
    }
}

/// 处理拖拽增量的辅助函数：从 winit CursorMoved 事件计算增量。
///
/// 注：此函数仅在不支持 `cursor_screen_position` 的平台上作为回退使用。
/// winit 的 CursorMoved 提供窗口相对坐标，拖拽窗口时鼠标屏幕位置不变但相对坐标会变，
/// 因此会导致抖动。主平台（Windows）已改用 `GetCursorPos` 获取屏幕绝对坐标。
#[allow(dead_code)]
pub fn drag_delta(
    state: &mut InputState,
    position: &winit::dpi::PhysicalPosition<f64>,
    win_pos: (i32, i32),
) -> Option<(i32, i32)> {
    let screen_x = win_pos.0 as f64 + position.x;
    let screen_y = win_pos.1 as f64 + position.y;
    if !state.down {
        state.last_screen_x = screen_x;
        state.last_screen_y = screen_y;
        return None;
    }
    let total_dx = screen_x - state.start_screen_x;
    let total_dy = screen_y - state.start_screen_y;
    state.check_moved(screen_x, screen_y);
    state.last_screen_x = screen_x;
    state.last_screen_y = screen_y;
    if state.moved {
        Some((total_dx as i32, total_dy as i32))
    } else {
        None
    }
}

