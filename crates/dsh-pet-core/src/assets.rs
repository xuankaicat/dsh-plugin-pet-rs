//! 像素素材包：加载/合并 whale-sprites-hd.json，校验网格，解析颜色。
//!
//! 对应 main.js L383-395 的 loadCustomSprites + pixel.js 的渲染逻辑。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{ensure, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SpritePack {
    #[serde(default)]
    pub canvas: Option<CanvasMeta>,
    #[serde(default)]
    pub palette: HashMap<String, String>,
    #[serde(default)]
    pub sprites: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub source_derivation: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CanvasMeta {
    pub width: usize,  // 80
    pub height: usize, // 58
}

/// 喷水柱定位（working/done sprite 中的 'R' 像素区域）
pub struct Spout {
    pub x0: usize,
    pub x1: usize,
    pub y0: usize,
}

impl SpritePack {
    /// 网格宽高（HD 版固定 80×58）
    pub const W: usize = 80;
    pub const H: usize = 58;

    /// 加载：内置素材 → 合并 custom/sprites.json（palette 逐 key 覆盖，sprites 整体替换）
    pub fn load(custom_dir: &Path) -> Result<Self> {
        let mut pack: SpritePack = serde_json::from_str(include_str!(
            "../../../assets/sprites/whale-sprites-hd.json"
        ))?;

        let custom_path = custom_dir.join("sprites.json");
        if custom_path.exists() {
            match std::fs::read_to_string(&custom_path) {
                Ok(json) => match serde_json::from_str::<SpritePack>(&json) {
                    Ok(custom) => {
                        for (k, v) in custom.palette {
                            pack.palette.insert(k, v);
                        }
                        if !custom.sprites.is_empty() {
                            pack.sprites = custom.sprites;
                        }
                        tracing::info!("已加载自定义素材包 {}", custom_path.display());
                    }
                    Err(e) => tracing::warn!("自定义素材包解析失败: {e}"),
                },
                Err(e) => tracing::warn!("自定义素材包读取失败: {e}"),
            }
        }
        pack.validate()?;
        Ok(pack)
    }

    /// 校验：每个 sprite 必须 58 行 × 80 字符
    pub fn validate(&self) -> Result<()> {
        for (name, sprite) in &self.sprites {
            ensure!(
                sprite.len() == Self::H,
                "sprite '{name}' 有 {} 行，期望 {}",
                sprite.len(),
                Self::H
            );
            for (i, row) in sprite.iter().enumerate() {
                let chars = row.chars().count();
                ensure!(
                    chars == Self::W,
                    "sprite '{name}' 第 {i} 行有 {chars} 字符，期望 {}",
                    Self::W
                );
            }
        }
        Ok(())
    }

    /// 颜色解析： "#RRGGBB" → [R,G,B,255], "#RRGGBBAA" → [R,G,B,A], "#00000000" → 透明
    pub fn parse_color(s: &str) -> Option<[u8; 4]> {
        let hex = s.strip_prefix('#')?;
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some([r, g, b, 255])
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some([r, g, b, a])
            }
            _ => None,
        }
    }

    /// 定位喷水柱（R 像素区域），用于水滴动画起点
    pub fn find_spout(sprite: &[String]) -> Option<Spout> {
        let mut x0 = usize::MAX;
        let mut x1 = 0usize;
        let mut y0 = usize::MAX;
        for (y, row) in sprite.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                if ch == 'R' {
                    if x < x0 {
                        x0 = x;
                    }
                    if x > x1 {
                        x1 = x;
                    }
                    if y < y0 {
                        y0 = y;
                    }
                }
            }
        }
        if x1 == 0 && x0 == usize::MAX {
            None
        } else {
            Some(Spout { x0, x1, y0 })
        }
    }

    /// 取某状态的 sprite，回退到 default
    pub fn sprite_for(&self, mode: &str) -> Option<&Vec<String>> {
        self.sprites
            .get(mode)
            .or_else(|| self.sprites.get("default"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_rgb() {
        assert_eq!(
            SpritePack::parse_color("#546AF5"),
            Some([0x54, 0x6A, 0xF5, 255])
        );
    }

    #[test]
    fn parse_color_rgba_transparent() {
        assert_eq!(SpritePack::parse_color("#00000000"), Some([0, 0, 0, 0]));
    }

    #[test]
    fn parse_color_invalid() {
        assert_eq!(SpritePack::parse_color("not-a-color"), None);
        assert_eq!(SpritePack::parse_color("#123"), None);
    }

    #[test]
    fn bundled_pack_loads_and_validates() {
        let pack = SpritePack::load(std::path::Path::new("/nonexistent/custom"))
            .expect("内置素材应可加载");
        assert_eq!(
            pack.canvas.as_ref().map(|c| (c.width, c.height)),
            Some((80, 58))
        );
        assert!(pack.palette.contains_key("b"));
        // 6 状态精灵 + default（starting 回退到 default）
        for m in ["default", "offline", "attention", "working", "done", "idle"] {
            assert!(pack.sprites.contains_key(m), "missing sprite {m}");
        }
        // starting 模式无独立 sprite，sprite_for 应回退到 default
        assert!(pack.sprite_for("starting").is_some());
    }
}
