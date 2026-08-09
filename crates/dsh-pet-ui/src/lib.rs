//! DSH 桌宠 UI 层：窗口、渲染、输入、emoji 图集。
//!
//! 纯 Rust 渲染：winit（窗口）+ tiny-skia（2D 合成）+ softbuffer（呈现）。

pub mod emoji;
pub mod input;
pub mod renderer;
pub mod text;
pub mod window;

pub use emoji::EmojiAtlas;
pub use input::{drag_delta, InputAction, InputState};
pub use renderer::Renderer;
pub use window::create_window;
