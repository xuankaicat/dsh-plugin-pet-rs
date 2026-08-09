//! 桌宠渲染器：tiny-skia Pixmap 合成 → softbuffer 呈现。
//!
//! 对应 app/renderer.js + app/pixel.js + app/styles.css 的全部渲染逻辑。

use std::sync::Arc;
use std::time::Instant;

use ab_glyph::{point, Font, FontArc, ScaleFont};
use dsh_pet_core::{Mode, Snapshot, SpritePack};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};

use crate::emoji::EmojiAtlas;
use crate::text::TextLayout;

/// 像素画每格的逻辑像素数：CANVAS_FULL_W(240) / W(80) = 3
/// （Electron 版 PIXEL_SCALE=6 是 canvas 物理像素基准，含 2x 超采样，由浏览器缩放；
///  Rust 版无浏览器缩放，直接用逻辑像素 3）
const PIXEL_SCALE: f32 = 3.0;
/// 舞台全尺寸（styles.css #stage）
const STAGE_FULL_W: f32 = 262.0;
const STAGE_FULL_H: f32 = 174.0;
/// Canvas 全尺寸（renderer.js CANVAS_FULL）
const CANVAS_FULL_W: f32 = 240.0;
const _CANVAS_FULL_H: f32 = 174.0;
/// 气泡尺寸（styles.css #bubble）
const BUBBLE_W: f32 = 252.0;
/// 气泡圆角半径（预留，v1 用直角矩形简化）
#[allow(dead_code)]
const BUBBLE_RADIUS: f32 = 14.0;
/// 气泡背景色 rgba(14,26,78,0.92) ≈ alpha 235
const BUBBLE_BG: [u8; 4] = [14, 26, 78, 235];
/// 气泡与鲸鱼间距
const GAP_NORMAL: f32 = 6.0;
/// working/done 时水柱顶起的间距
const GAP_SPRAY: f32 = 26.0;

pub struct Renderer {
    pixmap: Pixmap,
    assets: Arc<SpritePack>,
    font: FontArc,
    emoji_atlas: EmojiAtlas,
    /// 系统 DPI 比例（窗口逻辑像素 → 物理像素）
    dpi_scale: f32,
    /// 用户配置的鲸鱼舞台比例（0.5–1.1），只影响 stage，不影响气泡。
    pet_scale: f32,
    bubble_visible: bool,
    mode_changed_at: Instant,
    pub scroll_offset: f32,
}

impl Renderer {
    pub fn new(assets: Arc<SpritePack>, font: FontArc, pet_scale: f32, dpi_scale: f32) -> Self {
        let w = ((280.0 * dpi_scale).round() as u32).max(1);
        let h = ((340.0 * dpi_scale).round() as u32).max(1);
        Self {
            pixmap: Pixmap::new(w, h).expect("无法创建 pixmap"),
            assets,
            font,
            emoji_atlas: EmojiAtlas::embedded(),
            dpi_scale,
            pet_scale,
            bubble_visible: true,
            mode_changed_at: Instant::now(),
            scroll_offset: 0.0,
        }
    }

    /// 借用内部 pixmap（shot 模式用）
    pub fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }

    /// 把 pixmap 尺寸调整为精确的物理像素尺寸（与窗口 inner_size 对齐，避免 stride 不匹配）
    pub fn resize_pixmap(&mut self, phys_w: u32, phys_h: u32) {
        let w = phys_w.max(1);
        let h = phys_h.max(1);
        if self.pixmap.width() != w || self.pixmap.height() != h {
            self.pixmap = Pixmap::new(w, h).expect("无法重建 pixmap");
        }
    }

    /// 主渲染入口：返回呈现给 softbuffer 的 &[u32]
    pub fn render(&mut self, snapshot: &Snapshot, time_ms: u64) -> Vec<u32> {
        self.pixmap.fill(Color::TRANSPARENT);

        let gap = match snapshot.mode {
            Mode::Working | Mode::Done => GAP_SPRAY,
            _ => GAP_NORMAL,
        } * self.dpi_scale;

        // 布局：从下往上 — 鲸鱼在底部，气泡在上方
        let stage_scale = self.stage_scale();
        let whale_h = STAGE_FULL_H * stage_scale;
        let whale_y = self.pixmap.height() as f32 - whale_h - 6.0 * self.dpi_scale;
        let bubble_h = self.measure_bubble_h(snapshot);
        let bubble_y = (whale_y - gap - bubble_h).max(0.0);

        if self.bubble_visible {
            self.draw_bubble(snapshot, bubble_y, time_ms);
        }
        self.draw_whale(snapshot, whale_y, time_ms);

        // tiny-skia Pixmap::data() 返回 &[u8]，按 u32 重解释
        bytemuck::cast_slice(self.pixmap.data()).to_vec()
    }

    // ============================ 鲸鱼渲染 ============================

    fn draw_whale(&mut self, snapshot: &Snapshot, base_y: f32, time_ms: u64) {
        let mode = snapshot.mode;
        let sprite_name = mode.as_str();
        let sprite = match self.assets.sprite_for(sprite_name) {
            Some(s) => s.clone(),
            None => return,
        };

        // bob 动画（上下浮动）：styles.css L156-161
        let bob = match mode {
            Mode::Idle | Mode::Done => {
                ((time_ms as f32 / 3200.0 * std::f32::consts::TAU).sin()) * -5.0
            }
            Mode::Working => ((time_ms as f32 / 2200.0 * std::f32::consts::TAU).sin()) * -5.0,
            _ => 0.0,
        };

        // 像素画每格的物理像素数 — 必须是整数，否则子像素渲染产生扫描线
        let stage_scale = self.stage_scale();
        let pixel_scale = (PIXEL_SCALE * stage_scale).round().max(1.0) as i32;
        let pixel_scale_f = pixel_scale as f32;
        // stage 在 280px 窗口内居中，canvas 再在 262px stage 内居中。
        let stage_w = STAGE_FULL_W * stage_scale;
        let stage_x = (self.pixmap.width() as f32 - stage_w) / 2.0;
        let offset_x =
            (stage_x + ((STAGE_FULL_W - CANVAS_FULL_W) / 2.0) * stage_scale).round() as i32;
        let offset_y = (base_y + bob * stage_scale).round() as i32;

        // 绘制 sprite 网格 — 直接写像素，不用 fill_rect（避免浮点子像素不均匀）
        let pm_w = self.pixmap.width() as i32;
        let pm_h = self.pixmap.height() as i32;
        for (y, row) in sprite.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                if ch == '.' {
                    continue;
                }
                let color_str = match self.assets.palette.get(&ch.to_string()) {
                    Some(c) => c.as_str(),
                    None => continue,
                };
                let [r, g, b, a] = SpritePack::parse_color(color_str).unwrap_or([0, 0, 0, 0]);
                if a == 0 {
                    continue;
                }
                let px = offset_x + (x as i32) * pixel_scale;
                let py = offset_y + (y as i32) * pixel_scale;
                self.fill_pixel_block(px, py, pixel_scale, pm_w, pm_h, [r, g, b, a]);
            }
        }

        // 水滴动画（working / done）
        if matches!(mode, Mode::Working | Mode::Done) {
            self.draw_water_drops(
                &sprite,
                offset_x,
                offset_y,
                pixel_scale_f,
                pixel_scale,
                time_ms,
            );
        }

        // zzz 叠层（idle）
        if mode == Mode::Idle {
            self.draw_zzz(stage_x, base_y, time_ms);
        }

        // spark 叠层（done）
        if mode == Mode::Done {
            self.draw_sparks(stage_x, base_y, time_ms);
        }
    }

    /// 直接写入像素块（整数坐标，无子像素渲染问题）
    fn fill_pixel_block(
        &mut self,
        x: i32,
        y: i32,
        scale: i32,
        pm_w: i32,
        pm_h: i32,
        color: [u8; 4],
    ) {
        let premul = Color::from_rgba8(color[0], color[1], color[2], color[3])
            .premultiply()
            .to_color_u8();
        let pixels = self.pixmap.pixels_mut();
        for dy in 0..scale {
            for dx in 0..scale {
                let px = x + dx;
                let py = y + dy;
                if px >= 0 && py >= 0 && px < pm_w && py < pm_h {
                    let idx = (py as usize) * (pm_w as usize) + (px as usize);
                    pixels[idx] = premul;
                }
            }
        }
    }

    /// 水滴动画：pixel.js L49-60
    fn draw_water_drops(
        &mut self,
        sprite: &[String],
        ox: i32,
        oy: i32,
        _scale_f: f32,
        scale: i32,
        time_ms: u64,
    ) {
        let sp = match SpritePack::find_spout(sprite) {
            Some(sp) => sp,
            None => return,
        };
        let cx = (sp.x0 + sp.x1) / 2;
        let r_color = self
            .assets
            .palette
            .get("R")
            .and_then(|s| SpritePack::parse_color(s))
            .unwrap_or([143, 176, 255, 255]);

        let pm_w = self.pixmap.width() as i32;
        let pm_h = self.pixmap.height() as i32;
        for i in 0i32..3 {
            let phase = (((time_ms as f64 / 450.0 + i as f64 / 3.0) % 1.0) + 1.0) % 1.0;
            let y = sp.y0 as i32 - 1 + (phase * 7.0) as i32;
            let x = cx as i32 - 1 + i;
            let px = ox + x * scale;
            let py = oy + y.max(0) * scale;
            self.fill_pixel_block(px, py, scale, pm_w, pm_h, r_color);
        }
    }

    /// zzz 叠层：styles.css L124-137
    fn draw_zzz(&mut self, stage_x: f32, base_y: f32, time_ms: u64) {
        // 3 个 z，不同位置/大小/延迟
        let zzz_config: [(f32, f32, f32, u64); 3] = [
            (74.0, 34.0, 15.0, 0),
            (96.0, 18.0, 12.0, 200),
            (120.0, 6.0, 17.0, 700),
        ];
        let color = Color::from_rgba8(59, 82, 232, 255); // #3B52E8

        for &(left, top, font_size, delay) in &zzz_config {
            let (ty, scale_factor, opacity) = zzz_transform(time_ms, delay);
            let stage_scale = self.stage_scale();
            let x = stage_x + left * stage_scale;
            let y = (base_y + (top + ty) * stage_scale).max(0.0);
            let size = font_size * stage_scale * scale_factor;
            self.draw_text_char('z', x, y, size, color, opacity);
        }
    }

    /// spark 叠层：styles.css L139-152
    fn draw_sparks(&mut self, stage_x: f32, base_y: f32, time_ms: u64) {
        let spark_config: [(f32, f32, f32, u64); 4] = [
            (30.0, 52.0, 15.0, 0),
            (210.0, 40.0, 12.0, 300),
            (196.0, 96.0, 11.0, 600),
            (40.0, 130.0, 12.0, 900),
        ];

        for &(left, top, font_size, delay) in &spark_config {
            let (scale_factor, _rotate, opacity) = spark_transform(time_ms, delay);
            let stage_scale = self.stage_scale();
            let x = stage_x + left * stage_scale;
            let y = base_y + top * stage_scale;
            let size = font_size * stage_scale * scale_factor;
            self.emoji_atlas
                .draw_spark(&mut self.pixmap, x, y, size, opacity);
        }
    }

    // ============================ 气泡渲染 ============================

    fn measure_bubble_h(&self, snapshot: &Snapshot) -> f32 {
        let title_h = 13.0 * self.dpi_scale + 3.0 * self.dpi_scale;
        let body_line_h = 11.0 * self.dpi_scale * 1.5;
        let body_lines = snapshot.bubble.body.lines().count().clamp(1, 6) as f32;
        let padding = (10.0 + 12.0) * self.dpi_scale;
        title_h + body_line_h * body_lines + padding
    }

    fn draw_bubble(&mut self, snapshot: &Snapshot, y: f32, time_ms: u64) {
        let _ = time_ms;
        let w = BUBBLE_W * self.dpi_scale;
        let h = self.measure_bubble_h(snapshot);
        let x = ((self.pixmap.width() as f32) - w) / 2.0;

        // popIn 动画：模式变化后 400ms 内，scale 0.7 → 1.0
        let elapsed = self.mode_changed_at.elapsed().as_millis() as f32;
        let pop_progress = (elapsed / 400.0).min(1.0);
        let pop_alpha = pop_progress;

        // 圆角矩形背景
        let rect = match Rect::from_xywh(x, y, w, h) {
            Some(r) => r,
            None => return,
        };
        let mut path = PathBuilder::new();
        path.push_rect(rect);
        let path = match path.finish() {
            Some(p) => p,
            None => return,
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(
            BUBBLE_BG[0],
            BUBBLE_BG[1],
            BUBBLE_BG[2],
            (BUBBLE_BG[3] as f32 * pop_alpha) as u8,
        );
        paint.anti_alias = true;
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        // 三角尾巴（底部居中）
        let tail_x = x + w / 2.0;
        let mut tail = PathBuilder::new();
        tail.move_to(tail_x - 10.0, y + h);
        tail.line_to(tail_x, y + h + 9.0);
        tail.line_to(tail_x + 10.0, y + h);
        tail.close();
        let tail = match tail.finish() {
            Some(p) => p,
            None => return,
        };
        self.pixmap.fill_path(
            &tail,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        // 标题（bold 13px）
        self.draw_text(
            &snapshot.bubble.title,
            x + 14.0 * self.dpi_scale,
            y + 10.0 * self.dpi_scale,
            13.0 * self.dpi_scale,
            Color::from_rgba8(255, 255, 255, 255),
            w - 28.0 * self.dpi_scale,
            pop_alpha,
        );

        // 正文（11px，多行）
        let body_y = y + (10.0 + 3.0 + 13.0) * self.dpi_scale;
        let line_h = 11.0 * self.dpi_scale * 1.5;
        let layout = TextLayout::new(&self.font, 11.0 * self.dpi_scale);
        let lines = layout.layout(&snapshot.bubble.body, w - 28.0 * self.dpi_scale);

        let max_body_h = 108.0 * self.dpi_scale;
        let visible_lines = ((max_body_h / line_h).floor() as usize)
            .max(1)
            .min(lines.len());

        for (i, line) in lines.iter().enumerate() {
            if i >= visible_lines {
                break;
            }
            let ly = body_y + i as f32 * line_h - self.scroll_offset;
            if ly + line_h < body_y || ly > body_y + max_body_h {
                continue;
            }
            self.draw_text(
                &line.text,
                x + 14.0 * self.dpi_scale,
                ly,
                11.0 * self.dpi_scale,
                Color::from_rgba8(255, 255, 255, 209), // rgba(255,255,255,0.82)
                w - 28.0 * self.dpi_scale,
                pop_alpha,
            );
        }
    }

    // ============================ 文本渲染 ============================

    /// 绘制一行文本（支持 emoji 内嵌位图替换）
    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
        max_width: f32,
        alpha: f32,
    ) {
        let font = &self.font;
        let mut px = x;
        let py = y + size; // baseline 近似（top → baseline）

        for ch in text.chars() {
            // Emoji 检测：有内嵌位图则 blit
            if let Some(emoji_img) = self.emoji_atlas.get(ch) {
                self.emoji_atlas
                    .blit(&mut self.pixmap, px, py - size, size, emoji_img, alpha);
                px += size; // emoji 占 1 个 CJK 字符宽
                if px > x + max_width {
                    break;
                }
                continue;
            }

            let glyph_id = font.glyph_id(ch);
            let advance = font.as_scaled(size).h_advance(glyph_id);

            let glyph = glyph_id.with_scale_and_position(size, point(px, py));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                let gw = bounds.width().ceil() as u32;
                let gh = bounds.height().ceil() as u32;
                if gw > 0 && gh > 0 {
                    if let Some(mut glyph_pixmap) = Pixmap::new(gw, gh) {
                        let pixels = glyph_pixmap.pixels_mut();
                        outlined.draw(|gx, gy, coverage| {
                            let idx = (gy as usize) * (gw as usize) + (gx as usize);
                            if idx >= pixels.len() {
                                return;
                            }
                            let cov_a = coverage * alpha;
                            let c = Color::from_rgba(
                                color.red() * cov_a,
                                color.green() * cov_a,
                                color.blue() * cov_a,
                                cov_a,
                            )
                            .unwrap_or(Color::TRANSPARENT);
                            pixels[idx] = c.premultiply().to_color_u8();
                        });
                        let paint = PixmapPaint::default();
                        self.pixmap.draw_pixmap(
                            bounds.min.x as i32,
                            bounds.min.y as i32,
                            glyph_pixmap.as_ref(),
                            &paint,
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
            px += advance;
            if px > x + max_width {
                break;
            }
        }
    }

    /// 绘制单个字符（用于 zzz）
    fn draw_text_char(&mut self, ch: char, x: f32, y: f32, size: f32, color: Color, alpha: f32) {
        self.draw_text(&ch.to_string(), x, y, size, color, f32::MAX, alpha);
    }

    // ============================ 状态变更接口 ============================

    /// 模式变化时重置 popIn 动画时钟
    pub fn set_mode(&mut self, _old: Mode, _new: Mode) {
        self.mode_changed_at = Instant::now();
    }

    /// 调整用户鲸鱼比例；窗口和气泡尺寸保持不变。
    pub fn set_pet_scale(&mut self, scale: f32) {
        self.pet_scale = scale;
    }

    fn stage_scale(&self) -> f32 {
        self.pet_scale * self.dpi_scale
    }

    pub fn set_bubble_visible(&mut self, v: bool) {
        self.bubble_visible = v;
    }

    /// 是否需要持续动画（用于决定是否 request_redraw）
    pub fn is_animating(mode: Mode) -> bool {
        mode.is_animating()
    }

    /// 像素级命中检测：窗口坐标 (x, y) 是否落在非透明像素上
    pub fn is_whale_hit(&self, x: f64, y: f64) -> bool {
        let px = x as u32;
        let py = y as u32;
        if px >= self.pixmap.width() || py >= self.pixmap.height() {
            return false;
        }
        self.pixmap
            .pixel(px, py)
            .map(|c| c.alpha() > 0)
            .unwrap_or(false)
    }
}

// ============================ 缓动 / 动画函数 ============================

/// popIn 缓动：cubic-bezier(0.34, 1.4, 0.64, 1) 近似
fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// zzz 动画变换：styles.css L133-137
/// 0% { translateY(6px) scale(0.8) opacity:0 } 30% { opacity:1 }
/// 100% { translateY(-12px) scale(1.15) opacity:0 }
/// 周期 2.6s，各 z 延迟 0/0.2/0.7s
fn zzz_transform(time_ms: u64, delay_ms: u64) -> (f32, f32, f32) {
    let t = ((time_ms + delay_ms) as f32 / 2600.0) % 1.0;
    let progress = t;
    let ty = 6.0 + (-18.0) * progress; // 6 → -12
    let scale = 0.8 + 0.35 * progress; // 0.8 → 1.15
    let opacity = if progress < 0.3 {
        progress / 0.3
    } else {
        1.0 - (progress - 0.3) / 0.7
    };
    (ty, scale, opacity)
}

/// spark 动画变换：styles.css L149-152
/// 0%,100% { scale(0.6) rotate(0) opacity:0.4 } 50% { scale(1.2) rotate(20deg) opacity:1 }
/// 周期 1.6s，各 spark 延迟 0/0.3/0.6/0.9s
fn spark_transform(time_ms: u64, delay_ms: u64) -> (f32, f32, f32) {
    let t = ((time_ms + delay_ms) as f32 / 1600.0) % 1.0;
    let wave = (t * std::f32::consts::PI).sin().abs();
    let scale = 0.6 + 0.6 * wave; // 0.6 → 1.2 → 0.6
    let rotate = 20.0 * wave; // 0 → 20 → 0
    let opacity = 0.4 + 0.6 * wave; // 0.4 → 1.0 → 0.4
    (scale, rotate, opacity)
}

// 抑制未使用警告（ease_out_back 预留给气泡 popIn 缩放变换）
#[allow(dead_code)]
fn _use_ease_out_back() {
    let _ = ease_out_back(0.5);
}

// 抑制 PixmapRef 未使用导入警告
#[allow(unused_imports)]
use tiny_skia::PixmapRef as _PixmapRef;
