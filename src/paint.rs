//! Pixels for the bar and its menus. Everything the desktop shows of itself
//! is drawn here into an RGBA buffer and sent to the server whole, so the
//! text is antialiased, the pills have corners, and the X core font never
//! comes back.

use std::sync::Arc;

/// An RGBA image, straight alpha, rows top to bottom.
#[derive(Clone)]
pub struct Image {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Box-filter an ARGB icon (as `_NET_WM_ICON` hands it out) down to `size`.
    pub fn from_argb_scaled(width: u32, height: u32, argb: &[u32], size: u16) -> Option<Self> {
        if width == 0 || height == 0 || argb.len() < (width * height) as usize {
            return None;
        }
        let size_u = u32::from(size);
        let mut rgba = Vec::with_capacity(usize::from(size) * usize::from(size) * 4);
        for y in 0..size_u {
            let (y0, y1) = (
                y * height / size_u,
                ((y + 1) * height / size_u).max(y * height / size_u + 1),
            );
            for x in 0..size_u {
                let (x0, x1) = (
                    x * width / size_u,
                    ((x + 1) * width / size_u).max(x * width / size_u + 1),
                );
                let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
                for sy in y0..y1.min(height) {
                    for sx in x0..x1.min(width) {
                        let p = argb[(sy * width + sx) as usize];
                        let alpha = u64::from(p >> 24);
                        // Premultiply so transparent neighbours do not darken the edge.
                        r += u64::from((p >> 16) & 0xff) * alpha;
                        g += u64::from((p >> 8) & 0xff) * alpha;
                        b += u64::from(p & 0xff) * alpha;
                        a += alpha;
                        n += 1;
                    }
                }
                // A fully transparent cell has no colour to average; it stays clear.
                let channel = |sum: u64| sum.checked_div(a).unwrap_or(0) as u8;
                let alpha = (a / n.max(1)) as u8;
                rgba.extend_from_slice(&[channel(r), channel(g), channel(b), alpha]);
            }
        }
        Some(Self {
            width: size,
            height: size,
            rgba,
        })
    }
}

/// A typeface at one pixel size.
#[derive(Clone)]
pub struct Face {
    font: Arc<fontdue::Font>,
    px: f32,
}

impl Face {
    pub fn new(font: &Arc<fontdue::Font>, px: f32) -> Self {
        Self {
            font: Arc::clone(font),
            px,
        }
    }

    /// Where the baseline sits to centre a line of this face in a box.
    pub fn baseline_in(&self, top: i32, height: i32) -> i32 {
        let metrics = self.font.horizontal_line_metrics(self.px);
        let ascent = metrics.map_or(self.px * 0.93, |m| m.ascent);
        // Centre the cap height rather than the full ascent/descent box, which
        // is what the eye reads as centred for short labels.
        let caps = ascent * 0.73;
        top + ((height as f32 + caps) / 2.0).round() as i32
    }

    pub fn width(&self, text: &str) -> i32 {
        let mut x = 0.0;
        let mut previous = None;
        for c in text.chars() {
            if let Some(p) = previous
                && let Some(kern) = self.font.horizontal_kern(p, c, self.px)
            {
                x += kern;
            }
            x += self.font.metrics(c, self.px).advance_width;
            previous = Some(c);
        }
        x.round() as i32
    }

    /// The longest prefix that fits, with an ellipsis when it had to be cut.
    pub fn fit(&self, text: &str, max_width: i32) -> String {
        if self.width(text) <= max_width {
            return text.to_owned();
        }
        let chars: Vec<char> = text.chars().collect();
        for keep in (1..chars.len()).rev() {
            let candidate: String = chars[..keep]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_owned()
                + "…";
            if self.width(&candidate) <= max_width {
                return candidate;
            }
        }
        "…".to_owned()
    }
}

/// An opaque RGBA buffer the desktop paints into.
pub struct Canvas {
    pub image: Image,
}

fn channels(color: u32) -> [u8; 3] {
    let [_, r, g, b] = color.to_be_bytes();
    [r, g, b]
}

/// Mix an accent over a surface, including the bar's 16% status tints.
pub fn blend(fg: u32, bg: u32, alpha: f32) -> u32 {
    let alpha = alpha.clamp(0.0, 1.0);
    let mut result = 0;
    for (f, b) in channels(fg).into_iter().zip(channels(bg)) {
        result =
            (result << 8) | (f32::from(f) * alpha + f32::from(b) * (1.0 - alpha)).round() as u32;
    }
    result
}

impl Canvas {
    pub fn new(width: u16, height: u16, fill: u32) -> Self {
        let [r, g, b] = channels(fill);
        let rgba = [r, g, b, 255].repeat(usize::from(width) * usize::from(height));
        Self {
            image: Image {
                width,
                height,
                rgba,
            },
        }
    }

    fn width(&self) -> i32 {
        i32::from(self.image.width)
    }
    fn height(&self) -> i32 {
        i32::from(self.image.height)
    }

    /// Blend `color` over the pixel with `coverage` in 0..=255.
    fn plot(&mut self, x: i32, y: i32, color: [u8; 3], coverage: u32) {
        if x < 0 || y < 0 || x >= self.width() || y >= self.height() || coverage == 0 {
            return;
        }
        let at = ((y * self.width() + x) * 4) as usize;
        let pixel = &mut self.image.rgba[at..at + 4];
        for (dst, src) in pixel.iter_mut().zip(color) {
            *dst = ((u32::from(*dst) * (255 - coverage) + u32::from(src) * coverage) / 255) as u8;
        }
        pixel[3] = 255;
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: u32) {
        let color = channels(color);
        for py in y.max(0)..(y + height).min(self.height()) {
            for px in x.max(0)..(x + width).min(self.width()) {
                self.plot(px, py, color, 255);
            }
        }
    }

    /// A rectangle with rounded corners, antialiased by distance to the edge.
    pub fn round_rect(&mut self, x: i32, y: i32, width: i32, height: i32, radius: f32, color: u32) {
        let color = channels(color);
        let radius = radius
            .min(width as f32 / 2.0)
            .min(height as f32 / 2.0)
            .max(0.0);
        for py in y..y + height {
            for px in x..x + width {
                // Distance from the pixel centre to the rounded rectangle.
                let cx = (px - x) as f32 + 0.5;
                let cy = (py - y) as f32 + 0.5;
                let dx = (radius - cx).max(cx - (width as f32 - radius)).max(0.0);
                let dy = (radius - cy).max(cy - (height as f32 - radius)).max(0.0);
                let distance = (dx * dx + dy * dy).sqrt() - radius;
                let coverage = (0.5 - distance).clamp(0.0, 1.0);
                self.plot(px, py, color, (coverage * 255.0) as u32);
            }
        }
    }

    /// Three radius-eight box passes give a 24px soft edge. The alpha mask
    /// is clipped to the screen, so an offscreen client cannot allocate an
    /// unbounded image. Only its shadow, offset down ten pixels, is blended.
    pub fn shadow(&mut self, x: i32, y: i32, width: i32, height: i32) {
        let (left, top) = ((x - 24).max(0), (y + 10 - 24).max(0));
        let (right, bottom) = (
            (x + width + 24).min(self.width()),
            (y + 10 + height + 24).min(self.height()),
        );
        if right <= left || bottom <= top {
            return;
        }
        let (w, h) = ((right - left) as usize, (bottom - top) as usize);
        let mut mask = vec![0.0f32; w * h];
        for py in (y + 10).max(top)..(y + 10 + height).min(bottom) {
            for px in x.max(left)..(x + width).min(right) {
                mask[(py - top) as usize * w + (px - left) as usize] = 1.0;
            }
        }
        let mut scratch = vec![0.0f32; mask.len()];
        for _ in 0..3 {
            box_pass(&mask, &mut scratch, w, h, true);
            box_pass(&scratch, &mut mask, w, h, false);
        }
        for (index, alpha) in mask.into_iter().enumerate() {
            self.plot(
                left + (index % w) as i32,
                top + (index / w) as i32,
                [0, 0, 0],
                (alpha * 0.45 * 255.0).round() as u32,
            );
        }
    }

    pub fn dot(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        let color = channels(color);
        let (x0, y0) = (
            (cx - radius - 1.0).floor() as i32,
            (cy - radius - 1.0).floor() as i32,
        );
        let (x1, y1) = (
            (cx + radius + 1.0).ceil() as i32,
            (cy + radius + 1.0).ceil() as i32,
        );
        for py in y0..=y1 {
            for px in x0..=x1 {
                let distance = ((px as f32 + 0.5 - cx).powi(2) + (py as f32 + 0.5 - cy).powi(2))
                    .sqrt()
                    - radius;
                let coverage = (0.5 - distance).clamp(0.0, 1.0);
                self.plot(px, py, color, (coverage * 255.0) as u32);
            }
        }
    }

    /// Straight-alpha image over the canvas.
    pub fn blit(&mut self, x: i32, y: i32, image: &Image) {
        for (index, pixel) in image.rgba.as_chunks::<4>().0.iter().enumerate() {
            let px = x + (index % usize::from(image.width)) as i32;
            let py = y + (index / usize::from(image.width)) as i32;
            self.plot(px, py, [pixel[0], pixel[1], pixel[2]], u32::from(pixel[3]));
        }
    }

    /// Tint a straight-alpha image to one colour (a glyph or mark) and blit it.
    pub fn stamp(&mut self, x: i32, y: i32, image: &Image, color: u32) {
        let color = channels(color);
        for (index, pixel) in image.rgba.as_chunks::<4>().0.iter().enumerate() {
            let px = x + (index % usize::from(image.width)) as i32;
            let py = y + (index / usize::from(image.width)) as i32;
            self.plot(px, py, color, u32::from(pixel[3]));
        }
    }

    /// Draw `text` with its baseline at `baseline`; returns the advance.
    pub fn text(&mut self, face: &Face, x: i32, baseline: i32, text: &str, color: u32) -> i32 {
        let color = channels(color);
        let mut pen = x as f32;
        let mut previous = None;
        for c in text.chars() {
            if let Some(p) = previous
                && let Some(kern) = face.font.horizontal_kern(p, c, face.px)
            {
                pen += kern;
            }
            let (metrics, coverage) = face.font.rasterize(c, face.px);
            let left = pen.round() as i32 + metrics.xmin;
            let top = baseline - metrics.height as i32 - metrics.ymin;
            for (index, &alpha) in coverage.iter().enumerate() {
                let px = left + (index % metrics.width.max(1)) as i32;
                let py = top + (index / metrics.width.max(1)) as i32;
                // Light glyph edges need more coverage on a dark surface. Keep
                // images and shadows linear, and leave dark text unchanged.
                let mut coverage = u32::from(alpha);
                if px >= 0 && py >= 0 && px < self.width() && py < self.height() {
                    let at = ((py * self.width() + px) * 4) as usize;
                    let luminance = |p: &[u8]| {
                        2126 * u32::from(p[0]) + 7152 * u32::from(p[1]) + 722 * u32::from(p[2])
                    };
                    if luminance(&color) > luminance(&self.image.rgba[at..at + 3]) {
                        coverage =
                            ((f32::from(alpha) / 255.0).powf(1.0 / 1.4) * 255.0).round() as u32;
                    }
                }
                self.plot(px, py, color, coverage);
            }
            pen += metrics.advance_width;
            previous = Some(c);
        }
        pen.round() as i32 - x
    }
}

// A sliding sum makes each blur pass linear in the number of pixels.
fn box_pass(source: &[f32], target: &mut [f32], width: usize, height: usize, horizontal: bool) {
    let (lines, length, step) = if horizontal {
        (height, width, 1)
    } else {
        (width, height, width)
    };
    for line in 0..lines {
        let base = if horizontal { line * width } else { line };
        let mut sum: f32 = (0..9.min(length)).map(|i| source[base + i * step]).sum();
        for i in 0..length {
            target[base + i * step] = sum / 17.0;
            if i >= 8 {
                sum -= source[base + (i - 8) * step];
            }
            if i + 9 < length {
                sum += source[base + (i + 9) * step];
            }
        }
    }
}

/// Resample an RGBA image to `width`×`height` with a box filter.
pub fn resample(image: &Image, width: u16, height: u16) -> Image {
    let (sw, sh) = (u32::from(image.width), u32::from(image.height));
    let argb: Vec<u32> = image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            (u32::from(p[3]) << 24)
                | (u32::from(p[0]) << 16)
                | (u32::from(p[1]) << 8)
                | u32::from(p[2])
        })
        .collect();
    // Square targets only; the callers scale marks and icons to a square slot.
    let _ = height;
    Image::from_argb_scaled(sw, sh, &argb, width).unwrap_or_else(|| image.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face() -> Face {
        let bytes = include_bytes!("../assets/fonts/Geist-Regular.ttf").as_slice();
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).unwrap();
        Face::new(&Arc::new(font), 13.0)
    }

    #[test]
    fn a_rounded_pill_leaves_its_corners_to_the_background() {
        let mut canvas = Canvas::new(40, 20, 0x000000);
        canvas.round_rect(0, 0, 40, 20, 10.0, 0xffffff);
        assert_eq!(&canvas.image.rgba[..3], &[0, 0, 0], "corner stays black");
        let centre = ((10 * 40 + 20) * 4) as usize;
        assert_eq!(&canvas.image.rgba[centre..centre + 3], &[255, 255, 255]);
    }

    #[test]
    fn text_is_antialiased_and_measured() {
        let face = face();
        let mut canvas = Canvas::new(120, 24, 0x000000);
        let advance = canvas.text(&face, 4, 17, "14:32", 0xffffff);
        assert!(advance > 20 && advance < 60, "{advance}");
        assert_eq!(face.width("14:32"), advance);
        let levels: std::collections::HashSet<u8> =
            canvas.image.rgba.iter().step_by(4).copied().collect();
        assert!(levels.len() > 2, "antialiased text has grey levels");
        assert!(face.fit("Selenium form — Chromium", 60).ends_with('…'));
        assert_eq!(face.fit("Jobs", 400), "Jobs");
    }

    #[test]
    fn a_light_glyph_edge_gets_more_coverage_but_dark_ink_does_not() {
        let face = face();
        let (metrics, coverage) = face.font.rasterize('G', face.px);
        let edge = coverage.iter().position(|&a| a > 20 && a < 200).unwrap();
        let x = 4 + metrics.xmin + (edge % metrics.width) as i32;
        let y = 20 - metrics.height as i32 - metrics.ymin + (edge / metrics.width) as i32;
        let at = ((y * 40 + x) * 4) as usize;
        let mut light = Canvas::new(40, 30, 0);
        light.text(&face, 4, 20, "G", 0xffffff);
        let expected = ((f32::from(coverage[edge]) / 255.0).powf(1.0 / 1.4) * 255.0).round() as u8;
        assert_eq!(light.image.rgba[at], expected);
        assert!(expected > coverage[edge]);
        let mut dark = Canvas::new(40, 30, 0xffffff);
        dark.text(&face, 4, 20, "G", 0);
        assert_eq!(dark.image.rgba[at], 255 - coverage[edge]);
        assert_eq!(blend(0x8fb8ff, 0x111114, 0.16), 0x252c3a);
    }

    #[test]
    fn shadows_are_soft_offset_and_bounded() {
        let mut canvas = Canvas::new(180, 180, 0xffffff);
        canvas.shadow(50, 40, 80, 80);
        let pixel = |x: usize, y: usize| canvas.image.rgba[(y * 180 + x) * 4];
        assert_eq!(pixel(0, 0), 255);
        assert_eq!(pixel(90, 90), 140, "45 percent maximum alpha");
        assert!(pixel(49, 90) > pixel(50, 90));
        assert!(
            pixel(90, 135) < pixel(90, 25),
            "shadow is lower than the window"
        );
        assert_eq!(pixel(25, 90), 255, "three passes stop at 24 pixels");
        canvas.shadow(-100_000, -100_000, 10, 10);
    }

    #[test]
    fn icons_downscale_with_their_alpha() {
        let argb = vec![0xff00ff00u32; 32 * 32];
        let icon = Image::from_argb_scaled(32, 32, &argb, 16).unwrap();
        assert_eq!((icon.width, icon.height), (16, 16));
        assert_eq!(&icon.rgba[..4], &[0, 255, 0, 255]);
        assert!(Image::from_argb_scaled(32, 32, &argb[..10], 16).is_none());
    }
}
