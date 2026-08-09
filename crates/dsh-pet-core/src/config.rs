//! 配置：持久化的用户偏好（缩放、气泡可见性、提示音、窗口位置）。
//!
//! 对应 main.js L15-46 的全局配置 + main.js L404-410 的 setPetScale。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 鲸鱼缩放系数，0.5 – 1.1，默认 0.67
    #[serde(default = "Config::default_scale")]
    pub scale: f32,
    /// 气泡是否可见
    #[serde(default = "Config::default_true")]
    pub bubble_visible: bool,
    /// 状态提示音开关
    #[serde(default = "Config::default_true")]
    pub sound_on: bool,
    /// 窗口位置（保存上次位置，None 时定位到右下角）
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
}

impl Config {
    pub const SCALE_MIN: f32 = 0.5;
    pub const SCALE_MAX: f32 = 1.1;
    pub const SCALE_STEP: f32 = 0.1;
    pub const SCALE_DEFAULT: f32 = 0.67;

    fn default_scale() -> f32 {
        Self::SCALE_DEFAULT
    }
    fn default_true() -> bool {
        true
    }

    /// 配置文件路径：平台 config_dir / dsh-pet / config.json
    pub fn path() -> PathBuf {
        let proj = directories::ProjectDirs::from("com", "dsh", "pet").expect("无法获取配置目录");
        proj.config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                tracing::warn!("配置解析失败 ({e})，使用默认值");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::warn!("配置写入失败: {e}");
                }
            }
            Err(e) => tracing::warn!("配置序列化失败: {e}"),
        }
    }

    /// 把 scale 钳制到合法区间并保留两位小数
    pub fn clamp_scale(scale: f32) -> f32 {
        let clamped = scale.clamp(Self::SCALE_MIN, Self::SCALE_MAX);
        (clamped * 100.0).round() / 100.0
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scale: Self::SCALE_DEFAULT,
            bubble_visible: true,
            sound_on: true,
            window_x: None,
            window_y: None,
        }
    }
}
