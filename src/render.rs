use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};
use tiny_skia::{Paint, Pixmap, Rect, Transform};

use crate::config::Config;

pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl TextRenderer {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        }
    }

    pub fn measure(&mut self, config: &Config) -> (f32, f32) {
        let family = family(&config.font);
        let (mut width, mut height) = (0.0f32, 0.0f32);
        for line in &config.lines {
            let mut buffer = line_buffer(&line.text, family, line.size);
            buffer.shape_until_scroll(&mut self.font_system, false);
            let line_w = buffer
                .layout_runs()
                .map(|run| run.line_w)
                .fold(0.0, f32::max);
            width = width.max(line.x + line_w);
            height = height.max(line.y + line_height(line.size));
        }
        (width, height)
    }

    pub fn render(&mut self, width: u32, height: u32, scale: f32, config: &Config) -> Pixmap {
        let mut pixmap = Pixmap::new(width, height).expect("нулевой размер");
        let family = family(&config.font);
        for line in &config.lines {
            self.draw_line(
                &mut pixmap,
                &line.text,
                family,
                config.color.0,
                line.size * scale,
                line.x * scale,
                line.y * scale,
            );
        }
        pixmap
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_line(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        family: Family,
        color: cosmic_text::Color,
        size: f32,
        x: f32,
        y: f32,
    ) {
        let (x, y) = (x.round() as i32, y.round() as i32);
        let mut buffer = line_buffer(text, family, size);

        let mut paint = Paint::default();
        let text_alpha = u16::from(color.a());
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            color,
            |gx, gy, w, h, color| {
                let alpha = (u16::from(color.a()) * text_alpha + 127) / 255;
                paint.set_color_rgba8(color.r(), color.g(), color.b(), alpha as u8);
                let Some(rect) =
                    Rect::from_xywh((x + gx) as f32, (y + gy) as f32, w as f32, h as f32)
                else {
                    return;
                };
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            },
        );
    }
}

fn line_height(size: f32) -> f32 {
    size * 1.2
}

fn line_buffer(text: &str, family: Family, size: f32) -> Buffer {
    let mut buffer = Buffer::new_empty(Metrics::new(size, line_height(size)));
    buffer.set_size(None, None);
    buffer.set_text(text, &Attrs::new().family(family), Shaping::Advanced, None);
    buffer
}

fn family(name: &str) -> Family<'_> {
    match name {
        "sans-serif" | "sans" => Family::SansSerif,
        "serif" => Family::Serif,
        "monospace" | "mono" => Family::Monospace,
        name => Family::Name(name),
    }
}

pub fn copy_to_argb8888(pixmap: &Pixmap, canvas: &mut [u8]) {
    let (src, _) = pixmap.data().as_chunks::<4>();
    let (dst, _) = canvas.as_chunks_mut::<4>();
    for (&[r, g, b, a], dst) in src.iter().zip(dst) {
        *dst = [b, g, r, a];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_alpha_is_applied() {
        let mut renderer = TextRenderer::new();
        for alpha in [0x40, 0x80, 0xff] {
            let mut pixmap = Pixmap::new(200, 40).unwrap();
            let color = cosmic_text::Color::rgba(255, 255, 255, alpha);
            renderer.draw_line(
                &mut pixmap,
                "Activate",
                Family::SansSerif,
                color,
                24.0,
                0.0,
                0.0,
            );
            let max = pixmap.pixels().iter().map(|p| p.alpha()).max().unwrap();
            assert_eq!(max, alpha, "альфа цвета {alpha:#x}");
        }
    }
}
