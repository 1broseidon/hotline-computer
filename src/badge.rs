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
    /// The window holding the field: the receipt stays inside it.
    area: Option<[i32; 4]>,
    name: String,
    origin_matched: bool,
}

#[derive(Clone, Default)]
pub struct Badges(Arc<Mutex<Option<mpsc::SyncSender<Request>>>>);

impl Badges {
    pub fn show(
        &self,
        display: &str,
        bounds: [i32; 4],
        area: Option<[i32; 4]>,
        name: String,
        origin_matched: bool,
    ) {
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
                area,
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

fn position(bounds: [i32; 4], area: [i32; 4], width: u16) -> Option<(i16, i16)> {
    let [x, y, w, h] = bounds.map(i64::from);
    let [ax, ay, aw, ah] = area.map(i64::from);
    let (right, bottom) = (ax + aw, ay + ah);
    let (bw, bh) = (i64::from(width), i64::from(HEIGHT));
    if w <= 0 || h <= 0 || x + w <= ax || x >= right || y + h <= ay || y >= bottom || bw > aw {
        return None;
    }
    // Beside the field first: below it is usually the form's next control,
    // often the submit button the agent is about to press.
    let middle = (y + (h - bh) / 2).clamp(ay, bottom - bh);
    if x + w + 10 + bw <= right - 8 {
        return Some(((x + w + 10) as i16, middle as i16));
    }
    // Then above, over the field's own label, then below as a last resort.
    let x = x.clamp(ax, right - bw);
    if y - 8 - bh >= ay {
        return Some((x as i16, (y - 8 - bh) as i16));
    }
    let below = y + h + 6;
    (below + bh <= bottom).then_some((x as i16, below as i16))
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
    let label = face.fit(&format!("Sealed secret {name}"), limit);
    let width =
        (42 + face.width(&label) + face.width(matched)).clamp(1, i32::from(u16::MAX)) as u16;
    let (x, y) = position(
        request.bounds,
        request.area.unwrap_or([
            0,
            0,
            i32::from(screen.width_in_pixels),
            i32::from(screen.height_in_pixels),
        ]),
        width,
    )
    .ok_or("no room beside the focused field")?;
    let window = connection.generate_id().map_err(|e| e.to_string())?;
    let pixmap = connection.generate_id().map_err(|e| e.to_string())?;
    let gc = connection.generate_id().map_err(|e| e.to_string())?;
    connection
        .create_pixmap(screen.root_depth, pixmap, screen.root, width, HEIGHT)
        .map_err(|e| e.to_string())?;
    connection
        .create_gc(gc, pixmap, &CreateGCAux::new())
        .map_err(|e| e.to_string())?;
    // Start from what is on screen beneath the receipt, so its rounded
    // corners antialias against the real page rather than a guessed colour.
    let mut canvas = Canvas::new(width, HEIGHT, SURFACE);
    if let Ok(under) = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            screen.root,
            x,
            y,
            width,
            HEIGHT,
            u32::MAX,
        )
        .map_err(|e| e.to_string())
        .and_then(|cookie| cookie.reply().map_err(|e| e.to_string()))
        && under.data.len() >= usize::from(width) * usize::from(HEIGHT) * 4
    {
        for (pixel, bgrx) in canvas
            .image
            .rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(under.data.as_chunks::<4>().0)
        {
            *pixel = [bgrx[2], bgrx[1], bgrx[0], 255];
        }
    }
    canvas.round_rect(0, 0, i32::from(width), i32::from(HEIGHT), 9.0, RULE);
    canvas.round_rect(
        1,
        1,
        i32::from(width) - 2,
        i32::from(HEIGHT) - 2,
        8.0,
        SURFACE,
    );
    canvas.padlock(11, 10, OK);
    let baseline = face.baseline_in(0, i32::from(HEIGHT));
    let advance = canvas.text(face, 31, baseline, &label, INK);
    canvas.text(face, 31 + advance, baseline, matched, OK);
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
    fn a_badge_sits_beside_the_field_and_never_on_the_next_control() {
        // Beside a login field, centred on it, clear of the button below.
        assert_eq!(
            position([490, 648, 300, 44], [0, 0, 1920, 1080], 480),
            Some((800, 653))
        );
        // No room to the right: above, over the field's own label.
        assert_eq!(
            position([1600, 400, 300, 44], [0, 0, 1920, 1080], 480),
            Some((1440, 358))
        );
        // Inside a window narrower than the screen, the window is the limit.
        assert_eq!(
            position([438, 657, 404, 42], [16, 56, 1248, 1008], 430),
            Some((438, 615))
        );
        // Neither: below, as a last resort.
        assert_eq!(
            position([1880, 20, 40, 30], [0, 0, 1920, 1080], 480),
            Some((1440, 56))
        );
        for bounds in [[0, 0, 0, 0], [-500, 50, 40, 30], [i32::MAX; 4]] {
            assert_eq!(position(bounds, [0, 0, 1920, 1080], 480), None);
        }
    }
}
