//! 桌宠渲染器：tiny-skia Pixmap 合成 → softbuffer 呈现。
//!
//! 对应 app/renderer.js + app/pixel.js + app/styles.css 的全部渲染逻辑。

use std::sync::Arc;
use std::time::Instant;

use ab_glyph::{point, Font, FontArc, ScaleFont};
use dsh_pet_core::{Mode, Snapshot, SpritePack};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};

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
const BUBBLE_RADIUS: f32 = 14.0;
const SETTINGS_H: f32 = 132.0;
/// 气泡背景色 rgba(14,26,78,0.92) ≈ alpha 235
const BUBBLE_BG: [u8; 4] = [14, 26, 78, 235];
/// 气泡与鲸鱼间距
const GAP_NORMAL: f32 = 6.0;
/// working/done 时水柱顶起的间距
const GAP_SPRAY: f32 = 26.0;
/// 气泡浮入/浮出动画时长（毫秒）
const BUBBLE_ANIM_IN_MS: f32 = 300.0;
const BUBBLE_ANIM_OUT_MS: f32 = 260.0;

/// 气泡显示/隐藏动画状态：上浮出现、下浮消失
#[derive(Clone, Copy)]
enum BubbleAnim {
    Idle,
    Appearing { start: Instant },
    Disappearing { start: Instant },
}

#[derive(Clone, Copy, Default)]
struct HitRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl HitRect {
    fn contains(self, x: f64, y: f64) -> bool {
        let x = x as f32;
        let y = y as f32;
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsHit {
    None,
    ToggleSound,
    Close,
    EndpointInput,
}

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
    bubble_anim: BubbleAnim,
    settings_visible: bool,
    sound_on: bool,
    endpoint_text: String,
    endpoint_cursor: usize,
    endpoint_selection: Option<(usize, usize)>,
    endpoint_undo: Option<(String, usize)>,
    endpoint_preedit: Option<(String, Option<(usize, usize)>)>,
    endpoint_focused: bool,
    mode_changed_at: Instant,
    pub scroll_offset: f32,
    bubble_rect: Option<HitRect>,
    stage_rect: Option<HitRect>,
    settings_toggle_rect: Option<HitRect>,
    settings_close_rect: Option<HitRect>,
    settings_endpoint_rect: Option<HitRect>,
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
            bubble_anim: BubbleAnim::Idle,
            settings_visible: false,
            sound_on: true,
            endpoint_text: String::new(),
            endpoint_cursor: 0,
            endpoint_selection: None,
            endpoint_undo: None,
            endpoint_preedit: None,
            endpoint_focused: false,
            mode_changed_at: Instant::now(),
            scroll_offset: 0.0,
            bubble_rect: None,
            stage_rect: None,
            settings_toggle_rect: None,
            settings_close_rect: None,
            settings_endpoint_rect: None,
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
        let bubble_h = if self.settings_visible {
            SETTINGS_H * self.dpi_scale
        } else {
            self.measure_bubble_h(snapshot)
        };
        let bubble_y = (whale_y - gap - bubble_h).max(0.0);

        let stage_w = STAGE_FULL_W * stage_scale;
        self.stage_rect = Some(HitRect {
            x: (self.pixmap.width() as f32 - stage_w) / 2.0,
            y: whale_y,
            w: stage_w,
            h: whale_h,
        });

        // 气泡浮入/浮出动画：上浮出现（自下而上浮入并淡入）、下浮消失（向下沉出并淡出）
        let (bubble_offset, bubble_alpha) = self.bubble_anim_progress(bubble_h);
        if self.settings_visible {
            // 设置面板始终绘制（与气泡显隐无关），占据气泡区域
            let bubble_w = BUBBLE_W * self.dpi_scale;
            self.bubble_rect = Some(HitRect {
                x: (self.pixmap.width() as f32 - bubble_w) / 2.0,
                y: bubble_y,
                w: bubble_w,
                h: bubble_h + 9.0 * self.dpi_scale,
            });
            self.settings_toggle_rect = None;
            self.settings_close_rect = None;
            self.settings_endpoint_rect = None;
            self.draw_settings(bubble_y, time_ms);
        } else if self.bubble_visible
            || matches!(self.bubble_anim, BubbleAnim::Disappearing { .. })
        {
            let bubble_w = BUBBLE_W * self.dpi_scale;
            let anim_y = bubble_y + bubble_offset;
            self.bubble_rect = Some(HitRect {
                x: (self.pixmap.width() as f32 - bubble_w) / 2.0,
                y: anim_y,
                w: bubble_w,
                h: bubble_h + 9.0 * self.dpi_scale,
            });
            self.settings_toggle_rect = None;
            self.settings_close_rect = None;
            self.settings_endpoint_rect = None;
            self.draw_bubble(snapshot, anim_y, bubble_alpha, time_ms);
        } else {
            self.bubble_rect = None;
            self.settings_toggle_rect = None;
            self.settings_close_rect = None;
            self.settings_endpoint_rect = None;
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
        let title_h = 16.0 * self.dpi_scale + 4.0 * self.dpi_scale;
        let body_line_h = 13.0 * self.dpi_scale * 1.5;
        let layout = TextLayout::new(&self.font, 13.0 * self.dpi_scale);
        let body_lines = layout
            .layout(&snapshot.bubble.body, (BUBBLE_W - 28.0) * self.dpi_scale)
            .len()
            .clamp(1, 6) as f32;
        let padding = (10.0 + 12.0) * self.dpi_scale;
        title_h + body_line_h * body_lines + padding
    }

    fn draw_bubble(&mut self, snapshot: &Snapshot, y: f32, anim_alpha: f32, time_ms: u64) {
        let _ = time_ms;
        let w = BUBBLE_W * self.dpi_scale;
        let h = self.measure_bubble_h(snapshot);
        let x = ((self.pixmap.width() as f32) - w) / 2.0;

        // popIn 动画：模式变化后 400ms 内，scale 0.7 → 1.0
        let elapsed = self.mode_changed_at.elapsed().as_millis() as f32;
        let pop_progress = (elapsed / 400.0).min(1.0);
        // anim_alpha：气泡上浮出现 / 下浮消失的整体透明度
        let pop_alpha = pop_progress * anim_alpha;

        self.draw_card_background(x, y, w, h, pop_alpha);

        let mut paint = Paint::default();
        paint.set_color_rgba8(
            BUBBLE_BG[0],
            BUBBLE_BG[1],
            BUBBLE_BG[2],
            (BUBBLE_BG[3] as f32 * pop_alpha) as u8,
        );
        paint.anti_alias = true;

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

        // 标题
        self.draw_text(
            &snapshot.bubble.title,
            x + 14.0 * self.dpi_scale,
            y + 10.0 * self.dpi_scale,
            16.0 * self.dpi_scale,
            Color::from_rgba8(255, 255, 255, 255),
            w - 28.0 * self.dpi_scale,
            pop_alpha,
        );

        // 正文（13px，多行）
        let body_y = y + (10.0 + 4.0 + 16.0) * self.dpi_scale;
        let line_h = 13.0 * self.dpi_scale * 1.5;
        let layout = TextLayout::new(&self.font, 13.0 * self.dpi_scale);
        let lines = layout.layout(&snapshot.bubble.body, w - 28.0 * self.dpi_scale);

        let max_body_h = h - (body_y - y) - 12.0 * self.dpi_scale;
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
                13.0 * self.dpi_scale,
                Color::from_rgba8(255, 255, 255, 209), // rgba(255,255,255,0.82)
                w - 28.0 * self.dpi_scale,
                pop_alpha,
            );
        }
    }

    fn draw_settings(&mut self, y: f32, time_ms: u64) {
        let scale = self.dpi_scale;
        let w = BUBBLE_W * scale;
        let h = SETTINGS_H * scale;
        let x = (self.pixmap.width() as f32 - w) / 2.0;
        self.draw_card_background(x, y, w, h, 1.0);

        // 标题 "设置"
        self.draw_text(
            "设置",
            x + 14.0 * scale,
            y + 10.0 * scale,
            16.0 * scale,
            Color::WHITE,
            w - 56.0 * scale,
            1.0,
        );
        // "×" 关闭按钮
        self.draw_text(
            "×",
            x + w - 29.0 * scale,
            y + 7.0 * scale,
            18.0 * scale,
            Color::from_rgba8(255, 255, 255, 210),
            20.0 * scale,
            1.0,
        );
        // "声音提醒" 标签
        self.draw_text(
            "声音提醒",
            x + 14.0 * scale,
            y + 48.0 * scale,
            13.0 * scale,
            Color::from_rgba8(255, 255, 255, 230),
            130.0 * scale,
            1.0,
        );

        // 声音开关 toggle
        let toggle = HitRect {
            x: x + w - 58.0 * scale,
            y: y + 46.0 * scale,
            w: 44.0 * scale,
            h: 24.0 * scale,
        };
        let toggle_color = if self.sound_on {
            Color::from_rgba8(92, 215, 170, 255)
        } else {
            Color::from_rgba8(91, 104, 150, 255)
        };
        self.fill_rounded_rect(toggle, 12.0 * scale, toggle_color);
        let knob_x = if self.sound_on {
            toggle.x + 32.0 * scale
        } else {
            toggle.x + 12.0 * scale
        };
        let mut knob = PathBuilder::new();
        knob.push_circle(knob_x, toggle.y + 12.0 * scale, 8.0 * scale);
        if let Some(path) = knob.finish() {
            let mut paint = Paint::default();
            paint.set_color(Color::WHITE);
            paint.anti_alias = true;
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }

        // "DSH 地址" 标签
        self.draw_text(
            "DSH 地址",
            x + 14.0 * scale,
            y + 74.0 * scale,
            13.0 * scale,
            Color::from_rgba8(255, 255, 255, 230),
            130.0 * scale,
            1.0,
        );

        // endpoint 输入框
        let input_rect = HitRect {
            x: x + 10.0 * scale,
            y: y + 94.0 * scale,
            w: w - 20.0 * scale,
            h: 28.0 * scale,
        };
        // 输入框背景
        let bg_color = if self.endpoint_focused {
            Color::from_rgba8(30, 50, 110, 235)
        } else {
            Color::from_rgba8(22, 38, 90, 200)
        };
        self.fill_rounded_rect(input_rect, 6.0 * scale, bg_color);
        // 焦点边框
        if self.endpoint_focused {
            let border = HitRect {
                x: input_rect.x,
                y: input_rect.y,
                w: input_rect.w,
                h: input_rect.h,
            };
            let border_color = Color::from_rgba8(92, 160, 255, 200);
            self.fill_rounded_rect(border, 6.0 * scale, border_color);
            let inner = HitRect {
                x: input_rect.x + 1.0 * scale,
                y: input_rect.y + 1.0 * scale,
                w: input_rect.w - 2.0 * scale,
                h: input_rect.h - 2.0 * scale,
            };
            self.fill_rounded_rect(inner, 5.0 * scale, bg_color);
        }

        // endpoint 文本
        let text_x = input_rect.x + 8.0 * scale;
        let text_y = input_rect.y + 8.0 * scale;
        let text_size = 13.0 * scale;
        let text_max_w = input_rect.w - 16.0 * scale;

        let has_preedit = self.endpoint_preedit.is_some();
        let is_empty = self.endpoint_text.is_empty() && !has_preedit;

        if is_empty && !self.endpoint_focused {
            self.draw_text(
                "点击输入…",
                text_x,
                text_y,
                text_size,
                Color::from_rgba8(255, 255, 255, 100),
                text_max_w,
                1.0,
            );
        } else {
            let text_color = Color::from_rgba8(255, 255, 255, 235);
            // 已确认文本（clone 避免与 draw_text 的 &mut self 冲突）
            let confirmed = self.endpoint_text.clone();
            let preedit_info = self.endpoint_preedit.clone();
            let selection = self.endpoint_selection;
            let confirmed_w = self.measure_text_width(&confirmed, text_size);

            // 选区高亮背景
            if let Some((sel_start, sel_end)) = selection {
                let chars: Vec<char> = confirmed.chars().collect();
                let before: String = chars[..sel_start].iter().collect();
                let selected: String = chars[sel_start..sel_end].iter().collect();
                let sel_x = text_x + self.measure_text_width(&before, text_size).min(text_max_w);
                let sel_w = self.measure_text_width(&selected, text_size);
                let sel_rect = HitRect {
                    x: sel_x,
                    y: text_y,
                    w: sel_w,
                    h: text_size + 2.0 * scale,
                };
                self.fill_rounded_rect(sel_rect, 2.0 * scale, Color::from_rgba8(92, 160, 255, 120));
            }

            self.draw_text(
                &confirmed, text_x, text_y, text_size, text_color, text_max_w, 1.0,
            );
            // preedit 文本（紧接在已确认文本后）
            if let Some((preedit, _)) = &preedit_info {
                let preedit_x = text_x + confirmed_w.min(text_max_w);
                let remaining = (text_max_w - confirmed_w.min(text_max_w)).max(0.0);
                self.draw_text(
                    preedit, preedit_x, text_y, text_size, text_color, remaining, 1.0,
                );
                // preedit 下划线
                let preedit_w = self.measure_text_width(preedit, text_size).min(remaining);
                let underline = HitRect {
                    x: preedit_x,
                    y: text_y + text_size + 1.0 * scale,
                    w: preedit_w,
                    h: 1.0 * scale,
                };
                self.fill_rounded_rect(underline, 0.0, Color::from_rgba8(150, 180, 255, 200));
            }
        }

        // 光标（焦点时 500ms 闪烁）
        if self.endpoint_focused && (time_ms / 500).is_multiple_of(2) {
            if let Some(cursor_x) = self.endpoint_cursor_x(text_x, text_size, text_max_w) {
                let cursor_rect = HitRect {
                    x: cursor_x.round(),
                    y: input_rect.y + 6.0 * scale,
                    w: 1.0 * scale,
                    h: 16.0 * scale,
                };
                self.fill_rounded_rect(cursor_rect, 0.0, Color::from_rgba8(255, 255, 255, 220));
            }
        }

        self.settings_toggle_rect = Some(toggle);
        self.settings_close_rect = Some(HitRect {
            x: x + w - 42.0 * scale,
            y,
            w: 42.0 * scale,
            h: 38.0 * scale,
        });
        self.settings_endpoint_rect = Some(input_rect);
    }

    fn draw_card_background(&mut self, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        let scale = self.dpi_scale;
        let layers = [
            (12.0, 6.0, 12),
            (8.0, 6.0, 18),
            (4.0, 6.0, 28),
            (1.0, 6.0, 32),
        ];
        for (spread, offset_y, opacity) in layers {
            let rect = HitRect {
                x: x - spread * scale,
                y: y + offset_y * scale - spread * scale,
                w: w + spread * 2.0 * scale,
                h: h + spread * 2.0 * scale,
            };
            self.fill_rounded_rect(
                rect,
                (BUBBLE_RADIUS + spread) * scale,
                Color::from_rgba8(10, 20, 60, (opacity as f32 * alpha) as u8),
            );
        }
        self.fill_rounded_rect(
            HitRect { x, y, w, h },
            BUBBLE_RADIUS * scale,
            Color::from_rgba8(
                BUBBLE_BG[0],
                BUBBLE_BG[1],
                BUBBLE_BG[2],
                (BUBBLE_BG[3] as f32 * alpha) as u8,
            ),
        );
    }

    fn fill_rounded_rect(&mut self, rect: HitRect, radius: f32, color: Color) {
        let path = rounded_rect_path(rect.x, rect.y, rect.w, rect.h, radius);
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
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
                            let cov_a = coverage * alpha * color.alpha();
                            let c =
                                Color::from_rgba(color.red(), color.green(), color.blue(), cov_a)
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

    /// 测量文本渲染宽度（像素），用于光标定位。
    fn measure_text_width(&self, text: &str, size: f32) -> f32 {
        let font = &self.font;
        let scaled = font.as_scaled(size);
        text.chars()
            .map(|ch| scaled.h_advance(font.glyph_id(ch)))
            .sum()
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

    /// 切换气泡显示/隐藏，并启动上浮出现 / 下浮消失动画。
    pub fn set_bubble_visible(&mut self, v: bool) {
        if v == self.bubble_visible {
            return;
        }
        self.bubble_visible = v;
        self.bubble_anim = if v {
            BubbleAnim::Appearing {
                start: Instant::now(),
            }
        } else {
            BubbleAnim::Disappearing {
                start: Instant::now(),
            }
        };
    }

    /// 气泡浮入/浮出动画进度：返回 (y 方向偏移, 整体透明度)。
    /// 动画结束自动回到 Idle，返回 (0.0, 1.0)。
    fn bubble_anim_progress(&mut self, slide: f32) -> (f32, f32) {
        let (progress, dir) = match self.bubble_anim {
            BubbleAnim::Idle => return (0.0, 1.0),
            BubbleAnim::Appearing { start } => {
                let t = (start.elapsed().as_millis() as f32 / BUBBLE_ANIM_IN_MS).min(1.0);
                if t >= 1.0 {
                    self.bubble_anim = BubbleAnim::Idle;
                    return (0.0, 1.0);
                }
                (t, 1.0)
            }
            BubbleAnim::Disappearing { start } => {
                let t = (start.elapsed().as_millis() as f32 / BUBBLE_ANIM_OUT_MS).min(1.0);
                if t >= 1.0 {
                    self.bubble_anim = BubbleAnim::Idle;
                    return (0.0, 1.0);
                }
                (t, -1.0)
            }
        };
        if dir > 0.0 {
            // 上浮出现：自下而上浮入并淡入
            let e = ease_out_cubic(progress);
            (slide * (1.0 - e), e)
        } else {
            // 下浮消失：向下沉出并淡出
            let e = ease_in_cubic(progress);
            (slide * e, 1.0 - e)
        }
    }

    /// 气泡浮入/浮出动画是否进行中（用于决定是否 request_redraw）
    pub fn bubble_animating(&self) -> bool {
        matches!(
            self.bubble_anim,
            BubbleAnim::Appearing { .. } | BubbleAnim::Disappearing { .. }
        )
    }

    pub fn set_settings_visible(&mut self, visible: bool) {
        self.settings_visible = visible;
    }

    pub fn settings_visible(&self) -> bool {
        self.settings_visible
    }

    pub fn set_sound_on(&mut self, sound_on: bool) {
        self.sound_on = sound_on;
    }

    pub fn set_endpoint_text(&mut self, text: String) {
        let len = text.chars().count();
        self.endpoint_text = text;
        self.endpoint_cursor = len;
        self.endpoint_selection = None;
        self.endpoint_undo = None;
        self.endpoint_preedit = None;
    }

    pub fn endpoint_text(&self) -> &str {
        &self.endpoint_text
    }

    pub fn endpoint_cursor(&self) -> usize {
        self.endpoint_cursor
    }

    pub fn set_endpoint_cursor(&mut self, pos: usize) {
        let len = self.endpoint_text.chars().count();
        self.endpoint_cursor = pos.min(len);
    }

    pub fn endpoint_preedit(&self) -> &Option<(String, Option<(usize, usize)>)> {
        &self.endpoint_preedit
    }

    pub fn set_endpoint_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.endpoint_preedit = preedit;
    }

    pub fn move_cursor_left(&mut self) {
        if self.endpoint_cursor > 0 {
            self.endpoint_cursor -= 1;
        }
        self.endpoint_selection = None;
    }

    pub fn move_cursor_right(&mut self) {
        let len = self.endpoint_text.chars().count();
        if self.endpoint_cursor < len {
            self.endpoint_cursor += 1;
        }
        self.endpoint_selection = None;
    }

    pub fn move_cursor_home(&mut self) {
        self.endpoint_cursor = 0;
        self.endpoint_selection = None;
    }

    pub fn move_cursor_end(&mut self) {
        let len = self.endpoint_text.chars().count();
        self.endpoint_cursor = len;
        self.endpoint_selection = None;
    }

    // ============================ 选择 / 剪贴板 ============================

    pub fn select_all_endpoint(&mut self) {
        let len = self.endpoint_text.chars().count();
        self.endpoint_selection = Some((0, len));
        self.endpoint_cursor = len;
    }

    pub fn endpoint_selection(&self) -> Option<(usize, usize)> {
        self.endpoint_selection
    }

    pub fn selected_text(&self) -> Option<String> {
        self.endpoint_selection.map(|(s, e)| {
            let chars: Vec<char> = self.endpoint_text.chars().collect();
            chars[s..e].iter().collect()
        })
    }

    /// 删除当前选中的文本，返回是否确实删除了内容。
    pub fn delete_selection(&mut self) -> bool {
        let (s, e) = match self.endpoint_selection {
            Some(v) => v,
            None => return false,
        };
        let chars: Vec<char> = self.endpoint_text.chars().collect();
        let mut new_chars = chars[..s].to_vec();
        new_chars.extend_from_slice(&chars[e..]);
        self.endpoint_text = new_chars.into_iter().collect();
        self.endpoint_cursor = s;
        self.endpoint_selection = None;
        true
    }

    /// 在光标处插入文本（若有选区则替换）。保存 undo 快照。
    pub fn insert_text_at_cursor(&mut self, text: &str) {
        self.save_undo();
        if self.delete_selection() {
            // selection 已删除，继续在 cursor 处插入
        }
        self.endpoint_preedit = None;
        let byte_pos = self
            .endpoint_text
            .char_indices()
            .nth(self.endpoint_cursor)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.endpoint_text.len());
        self.endpoint_text.insert_str(byte_pos, text);
        self.endpoint_cursor += text.chars().count();
    }

    /// 删除光标前一个字符（Backspace）。保存 undo。
    pub fn backspace_at_cursor(&mut self) {
        if self.endpoint_selection.is_some() {
            self.save_undo();
            self.delete_selection();
            return;
        }
        if self.endpoint_cursor == 0 || self.endpoint_text.is_empty() {
            return;
        }
        self.save_undo();
        let char_pos = self.endpoint_cursor - 1;
        let byte_pos = self
            .endpoint_text
            .char_indices()
            .nth(char_pos)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.endpoint_text.len());
        let next_byte = self.endpoint_text[byte_pos..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| byte_pos + i)
            .unwrap_or_else(|| self.endpoint_text.len());
        self.endpoint_text.replace_range(byte_pos..next_byte, "");
        self.endpoint_cursor -= 1;
    }

    /// 删除光标后一个字符（Delete）。保存 undo。
    pub fn delete_at_cursor(&mut self) {
        if self.endpoint_selection.is_some() {
            self.save_undo();
            self.delete_selection();
            return;
        }
        let len = self.endpoint_text.chars().count();
        if self.endpoint_cursor >= len {
            return;
        }
        self.save_undo();
        let byte_pos = self
            .endpoint_text
            .char_indices()
            .nth(self.endpoint_cursor)
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.endpoint_text.len());
        let next_byte = self.endpoint_text[byte_pos..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| byte_pos + i)
            .unwrap_or_else(|| self.endpoint_text.len());
        self.endpoint_text.replace_range(byte_pos..next_byte, "");
    }

    // ============================ Undo ============================

    fn save_undo(&mut self) {
        self.endpoint_undo = Some((self.endpoint_text.clone(), self.endpoint_cursor));
    }

    pub fn undo(&mut self) {
        if let Some((text, cursor)) = self.endpoint_undo.take() {
            self.endpoint_text = text;
            self.endpoint_cursor = cursor;
            self.endpoint_selection = None;
            self.endpoint_preedit = None;
        }
    }

    pub fn set_endpoint_focused(&mut self, focused: bool) {
        self.endpoint_focused = focused;
        if !focused {
            self.endpoint_preedit = None;
            self.endpoint_selection = None;
        }
    }

    pub fn endpoint_focused(&self) -> bool {
        self.endpoint_focused
    }

    /// 输入框区域（物理像素），用于设置 IME 光标区域。
    pub fn endpoint_ime_area(&self) -> Option<(f32, f32, f32, f32)> {
        self.settings_endpoint_rect.map(|r| (r.x, r.y, r.w, r.h))
    }

    /// 计算光标 x 坐标（物理像素）。preedit cursor 为 None 时返回 None。
    fn endpoint_cursor_x(&self, text_x: f32, text_size: f32, text_max_w: f32) -> Option<f32> {
        if let Some((preedit, preedit_cursor)) = &self.endpoint_preedit {
            match preedit_cursor {
                Some((start, _)) => {
                    let start = (*start).min(preedit.len());
                    let safe = preedit
                        .char_indices()
                        .take_while(|(i, _)| *i <= start)
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    let mut s = self.endpoint_text.clone();
                    s.push_str(&preedit[..safe]);
                    Some(text_x + self.measure_text_width(&s, text_size).min(text_max_w))
                }
                None => None,
            }
        } else {
            let chars_before: String = self
                .endpoint_text
                .chars()
                .take(self.endpoint_cursor)
                .collect();
            Some(
                text_x
                    + self
                        .measure_text_width(&chars_before, text_size)
                        .min(text_max_w),
            )
        }
    }

    pub fn is_bubble_hit(&self, x: f64, y: f64) -> bool {
        self.bubble_rect.is_some_and(|rect| rect.contains(x, y))
    }

    pub fn is_whale_region(&self, x: f64, y: f64) -> bool {
        self.stage_rect.is_some_and(|rect| rect.contains(x, y))
    }

    pub fn settings_hit_test(&self, x: f64, y: f64) -> SettingsHit {
        if self
            .settings_close_rect
            .is_some_and(|rect| rect.contains(x, y))
        {
            SettingsHit::Close
        } else if self
            .settings_endpoint_rect
            .is_some_and(|rect| rect.contains(x, y))
        {
            SettingsHit::EndpointInput
        } else if self
            .settings_toggle_rect
            .is_some_and(|rect| rect.contains(x, y))
        {
            SettingsHit::ToggleSound
        } else {
            SettingsHit::None
        }
    }

    /// 是否需要持续动画（用于决定是否 request_redraw）
    pub fn is_animating(mode: Mode) -> bool {
        mode.is_animating()
    }

    /// 像素级命中检测：窗口坐标 (x, y) 是否落在非透明像素上
    pub fn is_whale_hit(&self, x: f64, y: f64) -> bool {
        if !self.is_whale_region(x, y) {
            return false;
        }
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

    /// 判断窗口坐标（物理像素）处是否完全透明，用于透明区域点击穿透。
    pub fn is_transparent_at(&self, x: f32, y: f32) -> bool {
        if x < 0.0 || y < 0.0 {
            return true;
        }
        let px = x as u32;
        let py = y as u32;
        if px >= self.pixmap.width() || py >= self.pixmap.height() {
            return true;
        }
        self.pixmap
            .pixel(px, py)
            .map(|c| c.alpha() == 0)
            .unwrap_or(true)
    }
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> tiny_skia::Path {
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = r * 0.552_284_8;
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    path.close();
    path.finish().expect("rounded rectangle path")
}

// ============================ 缓动 / 动画函数 ============================

/// popIn 缓动：cubic-bezier(0.34, 1.4, 0.64, 1) 近似
fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// 上浮出现缓动：快起缓落（decelerate）
fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// 下浮消失缓动：缓起快落（accelerate）
fn ease_in_cubic(t: f32) -> f32 {
    t.powi(3)
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
