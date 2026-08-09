//! 极简文本布局器：按 \n 分行，每行超出 max_width 时按字符贪心换行。
//!
//! 对应 spec 风险 3：不引入 cosmic-text，自写 ~150 行覆盖气泡场景。

use ab_glyph::{Font, FontArc, ScaleFont};

/// 一行文本（含实际文本和测量宽度）
#[derive(Debug, Clone)]
pub struct TextLine {
    pub text: String,
    pub width: f32,
}

pub struct TextLayout<'a> {
    font: &'a FontArc,
    font_size: f32,
}

impl<'a> TextLayout<'a> {
    pub fn new(font: &'a FontArc, font_size: f32) -> Self {
        Self { font, font_size }
    }

    /// 字符前进宽度（像素）
    fn char_advance(&self, ch: char) -> f32 {
        let glyph_id = self.font.glyph_id(ch);
        self.font.as_scaled(self.font_size).h_advance(glyph_id)
    }

    /// 按 \n 分行，每行超出 max_width 时按字符贪心换行（CJK 每字可断）
    pub fn layout(&self, text: &str, max_width: f32) -> Vec<TextLine> {
        let mut lines = Vec::new();
        for raw_line in text.split('\n') {
            let mut current = String::new();
            let mut current_width = 0.0f32;
            for ch in raw_line.chars() {
                let advance = self.char_advance(ch);
                if current_width + advance > max_width && !current.is_empty() {
                    lines.push(TextLine {
                        text: current.clone(),
                        width: current_width,
                    });
                    current.clear();
                    current_width = 0.0;
                }
                current.push(ch);
                current_width += advance;
            }
            if !current.is_empty() {
                lines.push(TextLine {
                    text: std::mem::take(&mut current),
                    width: current_width,
                });
            }
            // 空行也保留
            if raw_line.is_empty() {
                lines.push(TextLine {
                    text: String::new(),
                    width: 0.0,
                });
            }
        }
        lines
    }
}
