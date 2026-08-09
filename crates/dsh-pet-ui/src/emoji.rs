//! Emoji 位图图集：内嵌预栅格化 PNG（😢🎉💤✦）。
//!
//! 决策 4b：不引入彩色 emoji 字体（~10MB），改为内嵌小位图。

use std::collections::HashMap;

use tiny_skia::{Color, Pixmap, PixmapPaint, Transform};

/// 内嵌 emoji 位图图集
pub struct EmojiAtlas {
    /// Unicode 码点 → 解码后的 RGBA 位图
    emojis: HashMap<u32, image::RgbaImage>,
    /// ✦ 符号位图
    spark: image::RgbaImage,
}

impl EmojiAtlas {
    pub fn embedded() -> Self {
        let mut emojis = HashMap::new();
        emojis.insert(
            0x1F622u32,
            decode_png(include_bytes!("../../../assets/emoji/crying.png")),
        ); // 😢
        emojis.insert(
            0x1F389u32,
            decode_png(include_bytes!("../../../assets/emoji/party.png")),
        ); // 🎉
        emojis.insert(
            0x1F4A4u32,
            decode_png(include_bytes!("../../../assets/emoji/sleep.png")),
        ); // 💤
        let spark = decode_png(include_bytes!("../../../assets/emoji/spark.png")); // ✦
        Self { emojis, spark }
    }

    /// 查询某字符是否有内嵌位图
    pub fn get(&self, ch: char) -> Option<&image::RgbaImage> {
        self.emojis.get(&(ch as u32))
    }

    /// 在 pixmap 上绘制 ✦ 符号（缩放到 size，带 opacity）
    pub fn draw_spark(&self, pixmap: &mut Pixmap, x: f32, y: f32, size: f32, opacity: f32) {
        blit_image(pixmap, &self.spark, x, y, size, size, opacity);
    }

    /// 在 pixmap 上绘制指定 emoji 位图
    pub fn blit(
        &self,
        pixmap: &mut Pixmap,
        x: f32,
        y: f32,
        size: f32,
        data: &image::RgbaImage,
        opacity: f32,
    ) {
        blit_image(pixmap, data, x, y, size, size, opacity);
    }
}

fn decode_png(bytes: &[u8]) -> image::RgbaImage {
    image::load_from_memory(bytes)
        .expect("内嵌 emoji PNG 损坏")
        .to_rgba8()
}

/// 把 RGBA 图像缩放绘制到 tiny-skia Pixmap 上（带 opacity）
fn blit_image(
    dst: &mut Pixmap,
    img: &image::RgbaImage,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    opacity: f32,
) {
    let target_w = w.round().max(1.0) as u32;
    let target_h = h.round().max(1.0) as u32;
    if img.width() == 0 || img.height() == 0 {
        return;
    }

    // 先实际缩放像素，再按目标坐标原位 blit。
    // 不用 Transform::pre_translate：它会让平移量也参与缩放，导致 emoji 掉出文字行。
    let resized = image::imageops::resize(
        img,
        target_w,
        target_h,
        image::imageops::FilterType::Nearest,
    );
    let mut src = match Pixmap::new(target_w, target_h) {
        Some(p) => p,
        None => return,
    };
    let src_pixels = src.pixels_mut();
    let img_pixels = resized.as_raw();
    for (i, src_px) in src_pixels.iter_mut().enumerate() {
        let r = img_pixels[i * 4];
        let g = img_pixels[i * 4 + 1];
        let b = img_pixels[i * 4 + 2];
        let a = (img_pixels[i * 4 + 3] as f32 * opacity) as u8;
        *src_px = Color::from_rgba8(r, g, b, a).premultiply().to_color_u8();
    }

    let paint = PixmapPaint::default();
    dst.draw_pixmap(
        x.round() as i32,
        y.round() as i32,
        src.as_ref(),
        &paint,
        Transform::identity(),
        None,
    );
}
