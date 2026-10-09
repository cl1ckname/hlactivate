use std::{path::PathBuf, process::Command};

use cosmic_text::{
    Attrs, Buffer, Family, FamilyOwned, FontSystem, Metrics, Shaping, SwashCache, fontdb,
};

use crate::config::{Config, Lang};

pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    family: FamilyOwned,
}

impl TextRenderer {
    pub fn new(font: &str, lang: Lang) -> Self {
        let (font_system, family) = match load_single_font(font, lang) {
            Some(loaded) => loaded,
            None => {
                eprintln!("не удалось найти шрифт через fc-match, загружаю все системные");
                (FontSystem::new(), FamilyOwned::new(generic_family(font)))
            }
        };
        Self {
            font_system,
            swash_cache: SwashCache::new(),
            family,
        }
    }

    pub fn measure(&mut self, config: &Config) -> (f32, f32) {
        let (mut width, mut height) = (0.0f32, 0.0f32);
        for line in &config.lines {
            let (text_w, text_h) = self.measure_text(&line.text, line.size);
            width = width.max(line.x + text_w);
            height = height.max(line.y + text_h);
        }
        (width, height)
    }

    fn measure_text(&mut self, text: &str, size: f32) -> (f32, f32) {
        let mut buffer = self.line_buffer(text, size);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
            .layout_runs()
            .fold((0.0f32, 0.0f32), |(width, height), run| {
                (
                    width.max(run.line_w),
                    height.max(run.line_top + run.line_height),
                )
            })
    }

    pub fn render(
        &mut self,
        data: &mut [u8],
        width: u32,
        height: u32,
        scale: f32,
        config: &Config,
    ) {
        let mut canvas = Canvas::new(data, width, height);
        for line in &config.lines {
            self.draw_line(
                &mut canvas,
                &line.text,
                config.color.0,
                line.size * scale,
                line.x * scale,
                line.y * scale,
            );
        }
    }

    fn draw_line(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        color: cosmic_text::Color,
        size: f32,
        x: f32,
        y: f32,
    ) {
        let (x, y) = (x.round() as i32, y.round() as i32);
        let mut buffer = self.line_buffer(text, size);

        let text_alpha = color.a();
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            color,
            |gx, gy, w, h, color| {
                let alpha = mul(color.a(), text_alpha);
                canvas.fill_rect(
                    x.saturating_add(gx),
                    y.saturating_add(gy),
                    w,
                    h,
                    color.r(),
                    color.g(),
                    color.b(),
                    alpha,
                );
            },
        );
    }

    fn line_buffer(&self, text: &str, size: f32) -> Buffer {
        let mut buffer = Buffer::new_empty(Metrics::new(size, line_height(size)));
        buffer.set_size(None, None);
        let attrs = Attrs::new().family(self.family.as_family());
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer
    }
}

fn line_height(size: f32) -> f32 {
    size * 1.2
}

fn load_single_font(font: &str, lang: Lang) -> Option<(FontSystem, FamilyOwned)> {
    let (path, family) = fc_match(font, lang)?;
    let mut db = fontdb::Database::new();
    if db
        .load_font_source(fontdb::Source::File(path.clone()))
        .is_empty()
    {
        return None;
    }
    let font_system = FontSystem::new_with_locale_and_db(lang.code().into(), db);
    Some((font_system, FamilyOwned::new(Family::Name(&family))))
}

fn fc_match(font: &str, lang: Lang) -> Option<(PathBuf, String)> {
    let mut pattern = String::new();
    for c in font.chars() {
        if matches!(c, '\\' | '-' | ':' | ',') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push_str(":lang=");
    pattern.push_str(lang.code());

    let output = Command::new("fc-match")
        .args(["--format", "%{file}\n%{family[0]}", &pattern])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let (file, family) = stdout.split_once('\n')?;
    Some((file.into(), family.into()))
}

fn generic_family(name: &str) -> Family<'_> {
    match name {
        "sans-serif" | "sans" => Family::SansSerif,
        "serif" => Family::Serif,
        "monospace" | "mono" => Family::Monospace,
        name => Family::Name(name),
    }
}

struct Canvas<'a> {
    pixels: &'a mut [[u8; 4]],
    width: i32,
    height: i32,
}

impl<'a> Canvas<'a> {
    fn new(data: &'a mut [u8], width: u32, height: u32) -> Self {
        let (pixels, _) = data.as_chunks_mut::<4>();
        let pixels = &mut pixels[..width as usize * height as usize];
        pixels.fill([0; 4]);
        Self {
            pixels,
            width: width as i32,
            height: height as i32,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, r: u8, g: u8, b: u8, a: u8) {
        let (x0, x1) = clip(x, w, self.width);
        let (y0, y1) = clip(y, h, self.height);
        if a == 0 || x0 >= x1 || y0 >= y1 {
            return;
        }
        let src = [mul(b, a), mul(g, a), mul(r, a), a];
        let inv = 255 - a;
        let width = self.width as usize;
        for py in y0..y1 {
            let row = py * width;
            for dst in &mut self.pixels[row + x0..row + x1] {
                for c in 0..4 {
                    dst[c] = src[c].saturating_add(mul(dst[c], inv));
                }
            }
        }
    }
}

fn clip(start: i32, len: u32, limit: i32) -> (usize, usize) {
    let limit = i64::from(limit);
    let start = i64::from(start);
    let end = start + i64::from(len);
    (start.clamp(0, limit) as usize, end.clamp(0, limit) as usize)
}

fn mul(a: u8, b: u8) -> u8 {
    ((u16::from(a) * u16::from(b) + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_outside_canvas_are_clipped() {
        let mut data = vec![0u8; 10 * 4 * 4];
        let mut canvas = Canvas::new(&mut data, 10, 4);
        for (x, y) in [
            (12, 3),
            (10, 3),
            (-5, 3),
            (3, 4),
            (3, -1),
            (i32::MAX, 0),
            (i32::MIN, 0),
        ] {
            canvas.fill_rect(x, y, 2, 1, 255, 255, 255, 255);
        }
        canvas.fill_rect(9, 3, 5, 5, 255, 255, 255, 255);
        canvas.fill_rect(-1, -1, 2, 2, 255, 255, 255, 255);
        let opaque: Vec<usize> = data
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, p)| p[3] != 0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(opaque, [0, 39]);
    }

    #[test]
    fn text_past_right_edge_does_not_panic() {
        let mut renderer = TextRenderer::new("sans-serif", Lang::Eng);
        let mut data = vec![0u8; 20 * 30 * 4];
        let mut canvas = Canvas::new(&mut data, 20, 30);
        let color = cosmic_text::Color::rgba(255, 255, 255, 255);
        renderer.draw_line(&mut canvas, "Activate Linux", color, 24.0, 0.0, 0.0);
        renderer.draw_line(&mut canvas, "Activate Linux", color, 24.0, 500.0, 0.0);
    }

    #[test]
    fn multiline_text_height() {
        let mut renderer = TextRenderer::new("sans-serif", Lang::Eng);
        let (_, one) = renderer.measure_text("Hello", 24.0);
        let (_, two) = renderer.measure_text("Hello\nWorld", 24.0);
        let (_, three) = renderer.measure_text("a\nb\nc", 24.0);
        for (got, want) in [(one, 28.8), (two, 57.6), (three, 86.4)] {
            assert!((got - want).abs() < 0.01, "высота {got}, ожидалось {want}");
        }
    }

    #[test]
    fn text_alpha_is_applied() {
        let mut renderer = TextRenderer::new("sans-serif", Lang::Eng);
        for alpha in [0x40, 0x80, 0xff] {
            let mut data = vec![0u8; 200 * 40 * 4];
            let mut canvas = Canvas::new(&mut data, 200, 40);
            let color = cosmic_text::Color::rgba(255, 255, 255, alpha);
            renderer.draw_line(&mut canvas, "Activate", color, 24.0, 0.0, 0.0);
            let max = data.as_chunks::<4>().0.iter().map(|p| p[3]).max().unwrap();
            assert_eq!(max, alpha, "альфа цвета {alpha:#x}");
        }
    }
}
