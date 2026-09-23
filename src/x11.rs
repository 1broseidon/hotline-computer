use std::io::Cursor;

use serde::Serialize;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageData, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask,
    ImageFormat, MapState, Screen, Setup, Window as XWindow,
};
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Debug, Serialize)]
pub struct Window {
    pub id: String,
    pub title: String,
    pub class: String,
    pub bounds: [i32; 4],
    pub focused: bool,
    pub pid: Option<u32>,
    pub maximized: bool,
    pub minimum_size: [i32; 2],
}

pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn screenshot(display: &str) -> Result<Screenshot, String> {
    let (connection, screen_number) =
        x11rb::connect(Some(display)).map_err(|error| format!("x11 connect: {error}"))?;
    let screen = &connection.setup().roots[screen_number];
    grab(
        &connection,
        screen,
        0,
        0,
        screen.width_in_pixels,
        screen.height_in_pixels,
    )
}

/// The pixels of one rectangle of the root window, as RGBA.
pub fn grab<C: Connection>(
    connection: &C,
    screen: &Screen,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
) -> Result<Screenshot, String> {
    let reply = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            screen.root,
            x,
            y,
            width,
            height,
            u32::MAX,
        )
        .map_err(|error| format!("XGetImage: {error}"))?
        .reply()
        .map_err(|error| format!("XGetImage: {error}"))?;
    unpack(connection.setup(), reply.depth, width, height, &reply.data)
}

/// ZPixmap bytes at the server's depth, as RGBA.
pub fn unpack(
    setup: &Setup,
    depth: u8,
    width: u16,
    height: u16,
    data: &[u8],
) -> Result<Screenshot, String> {
    let format = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == depth)
        .ok_or_else(|| format!("x11: no pixel format for depth {depth}"))?;
    let bytes_per_pixel = usize::from(format.bits_per_pixel / 8);
    if bytes_per_pixel < 3 {
        return Err(format!(
            "x11: unsupported {} bits per pixel",
            format.bits_per_pixel
        ));
    }
    let width = usize::from(width);
    let height = usize::from(height);
    let stride = (width * usize::from(format.bits_per_pixel)).div_ceil(32) * 4;
    if data.len() < stride * height {
        return Err("x11: image reply was shorter than the rectangle".to_owned());
    }
    let mut rgba = vec![0_u8; width * height * 4];
    for row in 0..height {
        for column in 0..width {
            let source = row * stride + column * bytes_per_pixel;
            let target = (row * width + column) * 4;
            rgba[target] = data[source + 2];
            rgba[target + 1] = data[source + 1];
            rgba[target + 2] = data[source];
            rgba[target + 3] = 255;
        }
    }
    Ok(Screenshot {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

/// A rectangle of the screen as a PNG, and where it came from.
pub struct Picture {
    pub png: Vec<u8>,
    /// The screen rectangle shown: x, y, width, height.
    pub rect: [i32; 4],
    pub width: u32,
    pub height: u32,
}

impl Picture {
    /// How to turn a point in the image into a point on the screen, which
    /// is what input takes.
    pub fn describe(&self) -> String {
        let [x, y, width, height] = self.rect;
        if self.width as i32 == width {
            format!(
                "screenshot of {x},{y} {width}x{height} at full size: screen point = ({x} + image x, {y} + image y)"
            )
        } else {
            let factor = f64::from(width) / f64::from(self.width);
            format!(
                "screenshot of {x},{y} {width}x{height} shown at {}x{}: screen point = ({x} + image x × {factor:.3}, {y} + image y × {factor:.3})",
                self.width, self.height
            )
        }
    }
}

/// `rect` of the screen (all of it by default), scaled down to fit
/// `max_edge` when it is larger.
pub fn picture(
    display: &str,
    rect: Option<[i32; 4]>,
    max_edge: Option<u32>,
) -> Result<Picture, String> {
    let (connection, screen_number) =
        x11rb::connect(Some(display)).map_err(|error| format!("x11 connect: {error}"))?;
    let screen = &connection.setup().roots[screen_number];
    let full = [
        0,
        0,
        i32::from(screen.width_in_pixels),
        i32::from(screen.height_in_pixels),
    ];
    let rect = clip(rect.unwrap_or(full), full).ok_or_else(|| {
        format!(
            "the region is off the screen, which is {}x{}",
            full[2], full[3]
        )
    })?;
    let shot = grab(
        &connection,
        screen,
        rect[0] as i16,
        rect[1] as i16,
        rect[2] as u16,
        rect[3] as u16,
    )?;
    let longer = shot.width.max(shot.height);
    let limit = max_edge.unwrap_or(longer);
    let (width, height, pixels) = if longer <= limit {
        (shot.width, shot.height, shot.rgba)
    } else {
        let width = (u64::from(shot.width) * u64::from(limit) / u64::from(longer)).max(1) as u32;
        let height = (u64::from(shot.height) * u64::from(limit) / u64::from(longer)).max(1) as u32;
        (width, height, scale(&shot, width, height))
    };
    Ok(Picture {
        png: encode_png(width, height, &pixels, png::Compression::default())?,
        rect,
        width,
        height,
    })
}

/// The part of `rect` inside `screen`, if any.
fn clip(rect: [i32; 4], screen: [i32; 4]) -> Option<[i32; 4]> {
    let left = rect[0].max(screen[0]);
    let top = rect[1].max(screen[1]);
    let right = (rect[0].saturating_add(rect[2])).min(screen[0] + screen[2]);
    let bottom = (rect[1].saturating_add(rect[3])).min(screen[1] + screen[3]);
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

pub fn scaled_png(display: &str, max_edge: u32) -> Result<Vec<u8>, String> {
    picture(display, None, Some(max_edge)).map(|picture| picture.png)
}

fn scale(source: &Screenshot, width: u32, height: u32) -> Vec<u8> {
    let mut target = vec![0_u8; width as usize * height as usize * 4];
    for y in 0..height {
        let source_y = u64::from(y) * u64::from(source.height) / u64::from(height);
        for x in 0..width {
            let source_x = u64::from(x) * u64::from(source.width) / u64::from(width);
            let from = (source_y * u64::from(source.width) + source_x) as usize * 4;
            let to = (u64::from(y) * u64::from(width) + u64::from(x)) as usize * 4;
            target[to..to + 4].copy_from_slice(&source.rgba[from..from + 4]);
        }
    }
    target
}

pub fn encode_png(
    width: u32,
    height: u32,
    rgba: &[u8],
    compression: png::Compression,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(compression);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("encode PNG: {error}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|error| format!("encode PNG: {error}"))?;
    drop(writer);
    Ok(bytes)
}

pub fn windows(display: &str) -> Result<Vec<Window>, String> {
    let (connection, screen_number) =
        x11rb::connect(Some(display)).map_err(|error| format!("x11 connect: {error}"))?;
    let root = connection.setup().roots[screen_number].root;
    let clients = atom(&connection, b"_NET_CLIENT_LIST")?;
    let active_atom = atom(&connection, b"_NET_ACTIVE_WINDOW")?;
    let utf8 = atom(&connection, b"UTF8_STRING")?;
    let name_atom = atom(&connection, b"_NET_WM_NAME")?;
    let active = property_windows(&connection, root, active_atom)?
        .into_iter()
        .next();
    let ids = property_windows(&connection, root, clients)?;
    let mut result = Vec::new();
    for id in ids {
        let attributes = match connection
            .get_window_attributes(id)
            .map_err(|error| error.to_string())?
            .reply()
        {
            Ok(reply) if reply.map_state == MapState::VIEWABLE => reply,
            _ => continue,
        };
        let _ = attributes;
        let title = property_string(&connection, id, name_atom, utf8).unwrap_or_default();
        let title = if title.is_empty() {
            property_string(
                &connection,
                id,
                AtomEnum::WM_NAME.into(),
                AtomEnum::STRING.into(),
            )
            .unwrap_or_default()
        } else {
            title
        };
        let class = property_string(
            &connection,
            id,
            AtomEnum::WM_CLASS.into(),
            AtomEnum::STRING.into(),
        )
        .unwrap_or_default()
        .replace('\0', ".")
        .trim_matches('.')
        .to_owned();
        let Some(geometry) = live_reply(
            connection
                .get_geometry(id)
                .map_err(|error| error.to_string())?
                .reply(),
        )?
        else {
            continue;
        };
        let Some(translated) = live_reply(
            connection
                .translate_coordinates(id, root, 0, 0)
                .map_err(|error| error.to_string())?
                .reply(),
        )?
        else {
            continue;
        };
        result.push(Window {
            id: format!("0x{id:08x}"),
            title,
            class,
            bounds: [
                i32::from(translated.dst_x),
                i32::from(translated.dst_y),
                i32::from(geometry.width),
                i32::from(geometry.height),
            ],
            minimum_size: {
                let hints = connection
                    .get_property(
                        false,
                        id,
                        AtomEnum::WM_NORMAL_HINTS,
                        AtomEnum::WM_SIZE_HINTS,
                        0,
                        18,
                    )
                    .map_err(|e| e.to_string())?
                    .reply()
                    .ok()
                    .and_then(|reply| reply.value32().map(Iterator::collect::<Vec<_>>))
                    .unwrap_or_default();
                if hints.len() >= 7 && hints[0] & (1 << 4) != 0 {
                    [
                        hints[5].clamp(1, i32::MAX as u32) as i32,
                        hints[6].clamp(1, i32::MAX as u32) as i32,
                    ]
                } else {
                    [1, 1]
                }
            },
            focused: active == Some(id),
            pid: cardinal_property(&connection, id, atom(&connection, b"_NET_WM_PID")?)
                .first()
                .copied(),
            maximized: {
                let Some(state) = live_reply(
                    connection
                        .get_property(
                            false,
                            id,
                            atom(&connection, b"_NET_WM_STATE")?,
                            AtomEnum::ATOM,
                            0,
                            64,
                        )
                        .map_err(|e| e.to_string())?
                        .reply(),
                )?
                else {
                    continue;
                };
                let values: Vec<_> = state.value32().map(Iterator::collect).unwrap_or_default();
                values.contains(&atom(&connection, b"_NET_WM_STATE_MAXIMIZED_VERT")?)
                    && values.contains(&atom(&connection, b"_NET_WM_STATE_MAXIMIZED_HORZ")?)
            },
        });
    }
    Ok(result)
}

// Clients may disappear between enumerating the root list and reading properties.
fn live_reply<T>(reply: Result<T, x11rb::errors::ReplyError>) -> Result<Option<T>, String> {
    match reply {
        Ok(value) => Ok(Some(value)),
        Err(x11rb::errors::ReplyError::X11Error(error))
            if matches!(
                error.error_kind,
                x11rb::protocol::ErrorKind::Window | x11rb::protocol::ErrorKind::Drawable
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Ask the window manager, which is this agent's desktop thread, to
/// maximize or restore a window.
pub fn maximize(display: &str, window: &str, enabled: bool) -> Result<(), String> {
    let (connection, root) = connect(display)?;
    let state = atom(&connection, b"_NET_WM_STATE")?;
    let vertical = atom(&connection, b"_NET_WM_STATE_MAXIMIZED_VERT")?;
    let horizontal = atom(&connection, b"_NET_WM_STATE_MAXIMIZED_HORZ")?;
    ask_manager(
        &connection,
        root,
        window_id(window)?,
        state,
        [u32::from(enabled), vertical, horizontal, 1, 0],
    )
}

/// Ask the window manager to focus and raise a window.
pub fn activate(display: &str, window: &str) -> Result<(), String> {
    let (connection, root) = connect(display)?;
    let active = atom(&connection, b"_NET_ACTIVE_WINDOW")?;
    ask_manager(
        &connection,
        root,
        window_id(window)?,
        active,
        [2, 0, 0, 0, 0],
    )
}

/// Ask the window manager to close a window the polite way first.
pub fn close(display: &str, window: &str) -> Result<(), String> {
    let (connection, root) = connect(display)?;
    let close = atom(&connection, b"_NET_CLOSE_WINDOW")?;
    ask_manager(
        &connection,
        root,
        window_id(window)?,
        close,
        [0, 2, 0, 0, 0],
    )
}

/// Move and size a window. The request is redirected to the window
/// manager, which honours it for a window that is not maximized.
pub fn place(
    display: &str,
    window: &str,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let (connection, _) = connect(display)?;
    connection
        .configure_window(
            window_id(window)?,
            &ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .width(width.max(1))
                .height(height.max(1)),
        )
        .map_err(|error| error.to_string())?
        .check()
        .map_err(|error| error.to_string())?;
    connection.flush().map_err(|error| error.to_string())
}

fn connect(display: &str) -> Result<(RustConnection, XWindow), String> {
    let (connection, screen_number) =
        x11rb::connect(Some(display)).map_err(|error| format!("x11 connect: {error}"))?;
    let root = connection.setup().roots[screen_number].root;
    Ok((connection, root))
}

fn window_id(window: &str) -> Result<XWindow, String> {
    u32::from_str_radix(window.trim_start_matches("0x"), 16)
        .map_err(|_| "window_id must be an X11 window id".to_owned())
}

/// An EWMH client message on the root, which the desktop thread receives
/// because it selected substructure redirection there.
fn ask_manager(
    connection: &RustConnection,
    root: XWindow,
    window: XWindow,
    kind: u32,
    data: [u32; 5],
) -> Result<(), String> {
    let event = ClientMessageEvent::new(32, window, kind, ClientMessageData::from(data));
    connection
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .map_err(|error| error.to_string())?
        .check()
        .map_err(|error| error.to_string())?;
    connection.flush().map_err(|error| error.to_string())
}

fn atom<C: Connection>(connection: &C, name: &[u8]) -> Result<u32, String> {
    connection
        .intern_atom(false, name)
        .map_err(|error| error.to_string())?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| error.to_string())
}

fn property_windows<C: Connection>(
    connection: &C,
    window: XWindow,
    property: u32,
) -> Result<Vec<XWindow>, String> {
    let reply = connection
        .get_property(false, window, property, AtomEnum::WINDOW, 0, u32::MAX)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?;
    // A property the window manager has not set yet — `_NET_ACTIVE_WINDOW`
    // on a desktop nothing has focused — comes back with format 0, which is
    // an empty list, not a wrong type.
    if reply.format == 0 {
        return Ok(Vec::new());
    }
    reply
        .value32()
        .map(Iterator::collect)
        .ok_or_else(|| "x11: window property has the wrong type".to_owned())
}

fn property_string<C: Connection>(
    connection: &C,
    window: XWindow,
    property: u32,
    property_type: u32,
) -> Result<String, String> {
    let value = connection
        .get_property(false, window, property, property_type, 0, u32::MAX)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?
        .value;
    Ok(String::from_utf8_lossy(&value).into_owned())
}

fn cardinal_property(connection: &RustConnection, window: XWindow, property: u32) -> Vec<u32> {
    connection
        .get_property(false, window, property, AtomEnum::CARDINAL, 0, 16)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(Iterator::collect))
        .unwrap_or_default()
}

pub fn workarea(display: &str) -> Result<[i32; 4], String> {
    let (connection, root) = connect(display)?;
    let values = cardinal_property(&connection, root, atom(&connection, b"_NET_WORKAREA")?);
    if values.len() >= 4 && values[2] > 0 && values[3] > 0 {
        return Ok([
            values[0] as i32,
            values[1] as i32,
            values[2] as i32,
            values[3] as i32,
        ]);
    }
    let geometry = connection
        .get_geometry(root)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    Ok([0, 0, i32::from(geometry.width), i32::from(geometry.height)])
}

pub fn publish_jobs(display: &str, jobs: &[crate::jobs::Summary]) -> Result<(), String> {
    use x11rb::wrapper::ConnectionExt as _;
    let (connection, root) = connect(display)?;
    let running = jobs.iter().filter(|job| job.state == "running").count() as u32;
    let completed = jobs
        .iter()
        .filter(|job| job.state == "exited" && job.exit_code == Some(0))
        .count() as u32;
    let failed = jobs.iter().filter(|job| job.attention).count() as u32;
    connection
        .change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            root,
            atom(&connection, b"_HOTLINE_JOB_SUMMARY")?,
            atom(&connection, b"UTF8_STRING")?,
            &serde_json::to_vec(jobs).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    connection
        .change_property32(
            x11rb::protocol::xproto::PropMode::REPLACE,
            root,
            atom(&connection, b"_HOTLINE_JOBS_RUNNING")?,
            AtomEnum::CARDINAL,
            &[running, completed, failed],
        )
        .map_err(|e| e.to_string())?
        // Wait for X11 to apply both properties before dropping this connection.
        .check()
        .map_err(|e| e.to_string())
}

/// The bar shows who holds the machine: `_HOTLINE_HOLDER` carries the holder and
/// when the lease lapses, or nothing when nobody holds it.
pub fn publish_holder(display: &str, lease: Option<(&str, u64)>) -> Result<(), String> {
    use x11rb::wrapper::ConnectionExt as _;
    let (connection, root) = connect(display)?;
    let value = match lease {
        Some((holder, expires_ms)) => {
            serde_json::json!({"holder": holder, "expires_ms": expires_ms}).to_string()
        }
        None => String::new(),
    };
    connection
        .change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            root,
            atom(&connection, b"_HOTLINE_HOLDER")?,
            atom(&connection, b"UTF8_STRING")?,
            value.as_bytes(),
        )
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_is_clipped_to_the_screen() {
        let screen = [0, 0, 1920, 1080];
        assert_eq!(clip([100, 50, 200, 100], screen), Some([100, 50, 200, 100]));
        assert_eq!(clip([-10, 1000, 50, 200], screen), Some([0, 1000, 40, 80]));
        assert_eq!(clip([1920, 0, 10, 10], screen), None);
        assert_eq!(clip([0, 0, 0, 10], screen), None);
    }
}
