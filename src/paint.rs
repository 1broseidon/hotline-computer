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
                if a == 0 {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    rgba.extend_from_slice(&[
                        (r / a) as u8,
                        (g / a) as u8,
                        (b / a) as u8,
                        (a / n.max(1)) as u8,
                    ]);
                }
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
        let (ascent, descent) =
            metrics.map_or((self.px * 0.93, -self.px * 0.24), |m| (m.ascent, m.descent));
        // Centre the cap height rather than the full ascent/descent box, which
        // is what the eye reads as centred for short labels.
        let caps = ascent * 0.73;
        top + ((height as f32 + caps) / 2.0).round() as i32 + (descent * 0.0) as i32
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
                self.plot(px, py, color, u32::from(alpha));
            }
            pen += metrics.advance_width;
            previous = Some(c);
        }
        pen.round() as i32 - x
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

    fn face() -> Option<Face> {
        let bytes = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()?;
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()?;
        Some(Face::new(&Arc::new(font), 13.0))
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
        let Some(face) = face() else {
            eprintln!("DejaVu Sans is not installed here; skipping");
            return;
        };
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
    fn icons_downscale_with_their_alpha() {
        let argb = vec![0xff00ff00u32; 32 * 32];
        let icon = Image::from_argb_scaled(32, 32, &argb, 16).unwrap();
        assert_eq!((icon.width, icon.height), (16, 16));
        assert_eq!(&icon.rgba[..4], &[0, 255, 0, 255]);
        assert!(Image::from_argb_scaled(32, 32, &argb[..10], 16).is_none());
    }
}
