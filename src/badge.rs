//! A short-lived X11 receipt for a successful sealed fill, outside the page.
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ConnectionExt as _, CreateGCAux, CreateWindowAux, ImageFormat,
    PropMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::paint::{Canvas, Face};

const SURFACE: u32 = 0x17171b;
const RULE: u32 = 0x2a2a30;
const INK: u32 = 0xececef;
const OK: u32 = 0x86d69c;
const HEIGHT: u16 = 34;

struct Request {
    display: String,
    bounds: [i32; 4],
    name: String,
    origin_matched: bool,
}

#[derive(Clone, Default)]
pub struct Badges(Arc<Mutex<Option<mpsc::SyncSender<Request>>>>);

impl Badges {
    pub fn show(&self, display: &str, bounds: [i32; 4], name: String, origin_matched: bool) {
        let mut sender = self.0.lock().unwrap();
        if sender.is_none() {
            let (tx, rx) = mpsc::sync_channel(1);
            // One worker and one pending receipt, regardless of call rate.
            // Repeated fills replace the visible receipt and restart its timer.
            if std::thread::Builder::new()
                .name("sealed-fill-badge".into())
                .spawn(move || worker(rx))
                .is_err()
            {
                return;
            }
            *sender = Some(tx);
        }
        if let Some(sender) = sender.as_ref() {
            let _ = sender.try_send(Request {
                display: display.into(),
                bounds,
                name,
                origin_matched,
            });
        }
    }
}

fn worker(receiver: mpsc::Receiver<Request>) {
    let Ok(font) = fontdue::Font::from_bytes(
        include_bytes!("../assets/fonts/Geist-Regular.ttf").as_slice(),
        fontdue::FontSettings::default(),
    ) else {
        return;
    };
    let face = Face::new(&Arc::new(font), 13.0);
    let mut overlay = None;
    loop {
        let next = if overlay.is_some() {
            match receiver.recv_timeout(Duration::from_secs(4)) {
                Ok(request) => Some(request),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    overlay = None;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => None,
            }
        } else {
            receiver.recv().ok()
        };
        let Some(request) = next else { break };
        overlay = draw(&request, &face).ok();
    }
}

fn position(bounds: [i32; 4], screen: [u16; 2], width: u16) -> Option<(i16, i16)> {
    let [x, y, w, h] = bounds.map(i64::from);
    let [sw, sh] = screen.map(i64::from);
    if w <= 0 || h <= 0 || x + w <= 0 || x >= sw || y + h <= 0 || y >= sh || i64::from(width) > sw {
        return None;
    }
    let below = y + h + 6;
    // No room below the field means no badge, rather than covering the field
    // or placing a receipt somewhere unrelated to the successful fill.
    if below + i64::from(HEIGHT) > sh {
        return None;
    }
    Some((x.clamp(0, sw - i64::from(width)) as i16, below as i16))
}

/// Keeping this connection alive keeps the window/pixmap alive. Dropping it
/// removes the receipt and all its X resources, including on a paint error.
struct Overlay {
    _connection: RustConnection,
}

fn draw(request: &Request, face: &Face) -> Result<Overlay, String> {
    let (connection, number) = x11rb::connect(Some(&request.display)).map_err(|e| e.to_string())?;
    let screen = &connection.setup().roots[number];
    let name: String = crate::jobs::plain(&request.name)
        .chars()
        .filter(|c| !c.is_control())
        .take(96)
        .collect();
    let matched = if request.origin_matched {
        " · origin matched"
    } else {
        ""
    };
    let limit = i32::from(screen.width_in_pixels).saturating_sub(40 + face.width(matched));
    let label = face.fit(&format!("Typed from sealed secret {name}"), limit);
    let width =
        (40 + face.width(&label) + face.width(matched)).clamp(1, i32::from(u16::MAX)) as u16;
    let (x, y) = position(
        request.bounds,
        [screen.width_in_pixels, screen.height_in_pixels],
        width,
    )
    .ok_or("no room below the focused field")?;
    let window = connection.generate_id().map_err(|e| e.to_string())?;
    let pixmap = connection.generate_id().map_err(|e| e.to_string())?;
    let gc = connection.generate_id().map_err(|e| e.to_string())?;
    connection
        .create_pixmap(screen.root_depth, pixmap, screen.root, width, HEIGHT)
        .map_err(|e| e.to_string())?;
    connection
        .create_gc(gc, pixmap, &CreateGCAux::new())
        .map_err(|e| e.to_string())?;
    let mut canvas = Canvas::new(width, HEIGHT, RULE);
    canvas.fill_rect(1, 1, i32::from(width) - 2, i32::from(HEIGHT) - 2, SURFACE);
    canvas.padlock(10, 10, OK);
    let baseline = face.baseline_in(0, i32::from(HEIGHT));
    let advance = canvas.text(face, 30, baseline, &label, INK);
    canvas.text(face, 30 + advance, baseline, matched, OK);
    // Xvfb uses the same packed BGRX root visual as the desktop painter.
    let pixels: Vec<u8> = canvas
        .image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], 0])
        .collect();
    let stride = usize::from(width) * 4;
    let rows = (60_000 / stride).max(1);
    for (index, bytes) in pixels.chunks(stride * rows).enumerate() {
        connection
            .put_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                gc,
                width,
                (bytes.len() / stride) as u16,
                0,
                (index * rows) as i16,
                0,
                screen.root_depth,
                bytes,
            )
            .map_err(|e| e.to_string())?;
    }
    connection
        .create_window(
            screen.root_depth,
            window,
            screen.root,
            x,
            y,
            width,
            HEIGHT,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixmap(pixmap),
        )
        .map_err(|e| e.to_string())?;
    // Fail before mapping if SHAPE is unavailable. No input surface is safer
    // than a receipt that eats a click intended for the form beneath it.
    connection
        .shape_rectangles(
            SO::SET,
            SK::INPUT,
            ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[],
        )
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    connection
        .change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"Hotline sealed fill",
        )
        .map_err(|e| e.to_string())?;
    connection
        .map_window(window)
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    connection.flush().map_err(|e| e.to_string())?;
    Ok(Overlay {
        _connection: connection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_badge_stays_below_a_real_visible_field() {
        assert_eq!(
            position([490, 648, 300, 44], [1920, 1080], 480),
            Some((490, 698))
        );
        assert_eq!(
            position([1880, 50, 40, 30], [1920, 1080], 480),
            Some((1440, 86))
        );
        for bounds in [
            [0, 0, 0, 0],
            [-500, 50, 40, 30],
            [0, 1070, 100, 10],
            [i32::MAX; 4],
        ] {
            assert_eq!(position(bounds, [1920, 1080], 480), None);
        }
    }
}
