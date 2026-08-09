//! 鼠标输入状态机：区分单击 / 右键 / 拖拽 / 滚动。
//!
//! 对应 renderer.js L113-177 的拖拽 + 点击逻辑。

use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

/// 输入事件产生的动作
#[derive(Debug, Clone)]
pub enum InputAction {
    None,
    /// 单击鲸鱼/气泡 → 打开 GUI
    Click,
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
            in_bubble: false,
            in_whale: false,
            drag_threshold: 3.0,
        }
    }

    /// 更新鼠标位置（由 CursorMoved 事件驱动），更新 in_bubble / in_whale 状态
    pub fn update_cursor(&mut self, x: f64, y: f64) {
        self.last_screen_x = x;
        self.last_screen_y = y;
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
            WindowEvent::MouseInput { state, button, .. } => {
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if !self.in_whale && !self.in_bubble {
                            return InputAction::None;
                        }
                        // 鲸鱼区域需像素级命中
                        if self.in_whale && !is_whale_hit(self.last_screen_x, self.last_screen_y) {
                            return InputAction::None;
                        }
                        self.down = true;
                        self.moved = false;
                        self.start_screen_x = self.last_screen_x;
                        self.start_screen_y = self.last_screen_y;
                        InputAction::None
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        let action = if self.down && !self.moved {
                            InputAction::Click
                        } else if self.down {
                            InputAction::DragEnd
                        } else {
                            InputAction::None
                        };
                        self.down = false;
                        self.moved = false;
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
                }
            }
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

/// 处理拖拽增量的辅助函数：从 winit CursorMoved 事件计算增量
pub fn drag_delta(
    state: &mut InputState,
    position: &winit::dpi::PhysicalPosition<f64>,
) -> Option<(i32, i32)> {
    if !state.down {
        state.last_screen_x = position.x;
        state.last_screen_y = position.y;
        return None;
    }
    let dx = position.x - state.last_screen_x;
    let dy = position.y - state.last_screen_y;
    let total_dx = (position.x - state.start_screen_x).abs();
    let total_dy = (position.y - state.start_screen_y).abs();
    if !state.moved && (total_dx > state.drag_threshold || total_dy > state.drag_threshold) {
        state.moved = true;
    }
    state.last_screen_x = position.x;
    state.last_screen_y = position.y;
    if state.moved {
        Some((dx as i32, dy as i32))
    } else {
        None
    }
}
