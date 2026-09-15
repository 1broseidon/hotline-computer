//! The desktop the agent owns: the wallpaper, the bar, and the window
//! manager. Nothing else in the container has an opinion about windows.
//!
//! Chromium needs no decorations and the agent already speaks EWMH, so the
//! agent is the window manager. A normal window opens maximized into the
//! work area below the bar; a dialog opens centered at its own size. Focus
//! follows a click, so a person driving the screen reaches the window they
//! see. `_NET_CLIENT_LIST`, `_NET_ACTIVE_WINDOW` and `_NET_WM_STATE` are kept
//! current because the `windows` tool reads them.
//!
//! The bar is three answers: on the left, what can be opened (the toad menu);
//! in the middle, what is open; on the right, what the machine is doing and
//! whose it is. It is drawn as pixels by `paint` and sent whole, so its text
//! is antialiased and its state is legible at a glance in a screenshot.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Allow, Atom, AtomEnum, ButtonIndex, ButtonPressEvent, CONFIGURE_NOTIFY_EVENT,
    ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent, ConfigWindow,
    ConfigureNotifyEvent, ConfigureRequestEvent, ConfigureWindowAux, ConnectionExt, CreateGCAux,
    CreateWindowAux, EventMask, Gcontext, GrabMode, ImageFormat, InputFocus, KeyPressEvent,
    MapState, ModMask, Pixmap, PropMode, Rectangle, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use crate::paint::{Canvas, Face, Image, resample};

/// What the desktop asks of the agent side.
#[derive(Clone, Debug)]
pub enum Request {
    /// The menu asked for a browser and no browser window exists.
    OpenBrowser,
    /// The last browser window is gone. Chromium's process and its DevTools
    /// pages outlive a destroyed window, so the agent has to be told.
    BrowserClosed,
    OpenTerminal,
    /// The person asked for a shell of their own.
    OpenShell,
    OpenJob(String),
}

const MARK: &[u8] = include_bytes!("../assets/wallpaper-mark.png");
const BROWSER_CLASS: &str = "chromium";
const BACKGROUND: u32 = 0x000000;
/// The bar's height in pixels; the work area starts below it.
pub const BAR_HEIGHT: u16 = 36;
const BAR_FILL: u32 = 0x1c1c1f;
const BAR_RAISED: u32 = 0x28282c;
const BAR_HOVER: u32 = 0x343439;
const RULE: u32 = 0x38383d;
const INK: u32 = 0xe8e8ea;
const INK_2: u32 = 0xaaaaae;
const MUTED: u32 = 0x7d7d82;
const OK: u32 = 0x86d69c;
const WARN: u32 = 0xf0c27a;
const YOU: u32 = 0x7fc4f0;
const YOU_FILL: u32 = 0x25292e;
/// PutImage requests stay well under the smallest maximum request length.
const PUT_IMAGE_CHUNK: usize = 60_000;
/// The image's DejaVu; the bar has no second choice because it is ours.
const SANS: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
const MONO: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
/// Where menus drop from: under the mark, a hair below the bar.
const POPUP_X: i16 = 8;
const POPUP_Y: i16 = 40;
/// The jobs list hangs from its chip instead, so it is where the eye
/// already is; its right edge lines up with the chip's.
const EDGE: i32 = 8;
const ROW: i32 = 30;
const PAD: i32 = 6;
const MENU_WIDTH: u16 = 252;
const ABOUT_WIDTH: u16 = 300;
const TRAY_ICON: i32 = 18;
const APP_ICON: u16 = 16;
const PILL_MAX: i32 = 220;
const KEYSYM_ESCAPE: u32 = 0xff1b;
/// How far the person's shell sits in from the screen's bottom-left corner.
const SHELL_INSET: u16 = 8;

pub fn run(
    display: &str,
    requests: UnboundedSender<Request>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let mut desktop = match Desktop::new(display, requests) {
        Ok(desktop) => {
            let _ = ready.send(Ok(()));
            desktop
        }
        Err(error) => {
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    loop {
        let event = desktop
            .connection
            .wait_for_event()
            .map_err(|error| format!("display connection lost: {error}"))?;
        if let Err(error) = desktop.handle(event) {
            eprintln!("toad-computer: desktop: {error}");
        }
        desktop
            .connection
            .flush()
            .map_err(|error| format!("display connection lost: {error}"))?;
    }
}

struct Fonts {
    sans: Arc<fontdue::Font>,
    mono: Arc<fontdue::Font>,
}

impl Fonts {
    fn load() -> Result<Self, String> {
        let load = |path: &str| -> Result<Arc<fontdue::Font>, String> {
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
                .map(Arc::new)
                .map_err(|e| format!("{path}: {e}"))
        };
        Ok(Self {
            sans: load(SANS)?,
            mono: load(MONO)?,
        })
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    fn json(&self) -> serde_json::Value {
        json!([self.x, self.y, self.w, self.h])
    }
}

/// Where everything on the bar landed the last time it was drawn: clicks
/// are answered from this, and tests read it from `_TOAD_BAR_LAYOUT`.
#[derive(Default)]
struct Layout {
    mark: Rect,
    apps: Vec<(Rect, Window)>,
    jobs: Rect,
    lease: Rect,
    tray: Vec<Rect>,
    clock: Rect,
}

enum Popup {
    Menu,
    Jobs {
        jobs: Vec<crate::jobs::Summary>,
        first: usize,
        visible: usize,
    },
    About,
}

struct Open {
    window: Window,
    kind: Popup,
    width: u16,
    height: u16,
}

/// Who holds the machine, as the agent publishes it.
#[derive(Clone, Debug, PartialEq)]
struct Lease {
    holder: String,
    expires_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Holding {
    Nobody,
    Agent,
    Person,
}

struct Client {
    maximized: bool,
    /// Where the window goes when it is not maximized.
    saved: (i16, i16, u16, u16),
    /// `WM_CLASS`, lower-cased, read once: a destroyed window has none to read.
    class: String,
    /// `_NET_WM_ICON` at the bar's size, when the window offers one.
    icon: Option<Image>,
}

enum Kind {
    Normal,
    Dialog,
    /// Panels and docks of other programs: mapped where they ask, never managed.
    Furniture,
}

struct Atoms {
    net_supported: Atom,
    net_client_list: Atom,
    net_active_window: Atom,
    net_wm_state: Atom,
    maximized_vert: Atom,
    maximized_horz: Atom,
    net_wm_name: Atom,
    net_wm_icon: Atom,
    utf8_string: Atom,
    net_close_window: Atom,
    net_workarea: Atom,
    net_supporting_wm_check: Atom,
    net_wm_window_type: Atom,
    window_type_dock: Atom,
    window_type_desktop: Atom,
    window_type_toolbar: Atom,
    window_type_menu: Atom,
    window_type_splash: Atom,
    window_type_notification: Atom,
    window_type_dialog: Atom,
    window_type_normal: Atom,
    net_number_of_desktops: Atom,
    net_current_desktop: Atom,
    net_desktop_geometry: Atom,
    net_desktop_viewport: Atom,
    net_frame_extents: Atom,
    wm_protocols: Atom,
    wm_delete_window: Atom,
    tray_selection: Atom,
    tray_opcode: Atom,
    xembed: Atom,
    jobs_running: Atom,
    jobs_summary: Atom,
    holder: Atom,
    tick: Atom,
    layout: Atom,
}

impl Atoms {
    fn intern(connection: &RustConnection, screen_number: usize) -> Result<Self, String> {
        let atom = |name: &[u8]| -> Result<Atom, String> {
            connection
                .intern_atom(false, name)
                .map_err(|error| error.to_string())?
                .reply()
                .map(|reply| reply.atom)
                .map_err(|error| error.to_string())
        };
        Ok(Self {
            net_supported: atom(b"_NET_SUPPORTED")?,
            net_client_list: atom(b"_NET_CLIENT_LIST")?,
            net_active_window: atom(b"_NET_ACTIVE_WINDOW")?,
            net_wm_state: atom(b"_NET_WM_STATE")?,
            maximized_vert: atom(b"_NET_WM_STATE_MAXIMIZED_VERT")?,
            maximized_horz: atom(b"_NET_WM_STATE_MAXIMIZED_HORZ")?,
            net_wm_name: atom(b"_NET_WM_NAME")?,
            net_wm_icon: atom(b"_NET_WM_ICON")?,
            utf8_string: atom(b"UTF8_STRING")?,
            net_close_window: atom(b"_NET_CLOSE_WINDOW")?,
            net_workarea: atom(b"_NET_WORKAREA")?,
            net_supporting_wm_check: atom(b"_NET_SUPPORTING_WM_CHECK")?,
            net_wm_window_type: atom(b"_NET_WM_WINDOW_TYPE")?,
            window_type_dock: atom(b"_NET_WM_WINDOW_TYPE_DOCK")?,
            window_type_desktop: atom(b"_NET_WM_WINDOW_TYPE_DESKTOP")?,
            window_type_toolbar: atom(b"_NET_WM_WINDOW_TYPE_TOOLBAR")?,
            window_type_menu: atom(b"_NET_WM_WINDOW_TYPE_MENU")?,
            window_type_splash: atom(b"_NET_WM_WINDOW_TYPE_SPLASH")?,
            window_type_notification: atom(b"_NET_WM_WINDOW_TYPE_NOTIFICATION")?,
            window_type_dialog: atom(b"_NET_WM_WINDOW_TYPE_DIALOG")?,
            window_type_normal: atom(b"_NET_WM_WINDOW_TYPE_NORMAL")?,
            net_number_of_desktops: atom(b"_NET_NUMBER_OF_DESKTOPS")?,
            net_current_desktop: atom(b"_NET_CURRENT_DESKTOP")?,
            net_desktop_geometry: atom(b"_NET_DESKTOP_GEOMETRY")?,
            net_desktop_viewport: atom(b"_NET_DESKTOP_VIEWPORT")?,
            net_frame_extents: atom(b"_NET_FRAME_EXTENTS")?,
            wm_protocols: atom(b"WM_PROTOCOLS")?,
            wm_delete_window: atom(b"WM_DELETE_WINDOW")?,
            tray_selection: atom(format!("_NET_SYSTEM_TRAY_S{screen_number}").as_bytes())?,
            tray_opcode: atom(b"_NET_SYSTEM_TRAY_OPCODE")?,
            xembed: atom(b"_XEMBED")?,
            jobs_running: atom(b"_TOAD_JOBS_RUNNING")?,
            jobs_summary: atom(b"_TOAD_JOB_SUMMARY")?,
            holder: atom(b"_TOAD_HOLDER")?,
            tick: atom(b"_TOAD_TICK")?,
            layout: atom(b"_TOAD_BAR_LAYOUT")?,
        })
    }

    fn supported(&self) -> Vec<Atom> {
        vec![
            self.net_client_list,
            self.net_active_window,
            self.net_wm_state,
            self.maximized_vert,
            self.maximized_horz,
            self.net_wm_name,
            self.net_wm_icon,
            self.net_close_window,
            self.net_workarea,
            self.net_supporting_wm_check,
            self.net_wm_window_type,
            self.window_type_dock,
            self.window_type_dialog,
            self.window_type_normal,
            self.net_number_of_desktops,
            self.net_current_desktop,
            self.net_desktop_geometry,
            self.net_desktop_viewport,
            self.net_frame_extents,
        ]
    }
}

struct Desktop {
    connection: RustConnection,
    root: Window,
    width: u16,
    height: u16,
    depth: u8,
    atoms: Atoms,
    gc: Gcontext,
    bar: Window,
    /// Managed windows in mapping order, which is what `_NET_CLIENT_LIST` lists.
    order: Vec<Window>,
    clients: HashMap<Window, Client>,
    active: Option<Window>,
    requests: UnboundedSender<Request>,
    tray: Vec<Window>,
    popup: Option<Open>,
    fonts: Fonts,
    mark: Image,
    layout: Layout,
    lease: Option<Lease>,
    /// The first keysym of every keycode, so a menu can read A to D and Escape.
    keysyms: (u8, usize, Vec<u32>),
    started: Instant,
    /// What the bar last showed of the clock and the lease, so a tick that
    /// changes neither draws nothing.
    shown: String,
}

impl Desktop {
    fn new(display: &str, requests: UnboundedSender<Request>) -> Result<Self, String> {
        let (connection, screen_number) =
            x11rb::connect(Some(display)).map_err(|error| format!("x11 connect: {error}"))?;
        let screen = connection.setup().roots[screen_number].clone();
        if screen.root_depth != 24 {
            return Err(format!(
                "the display is {}-bit; the desktop draws 24-bit pixels",
                screen.root_depth
            ));
        }
        connection
            .change_window_attributes(
                screen.root,
                &ChangeWindowAttributesAux::new().event_mask(
                    EventMask::SUBSTRUCTURE_REDIRECT
                        | EventMask::SUBSTRUCTURE_NOTIFY
                        | EventMask::PROPERTY_CHANGE,
                ),
            )
            .map_err(|error| error.to_string())?
            .check()
            .map_err(|_| "another window manager owns this display".to_owned())?;
        let atoms = Atoms::intern(&connection, screen_number)?;
        let gc = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        connection
            .create_gc(gc, screen.root, &CreateGCAux::new().foreground(BACKGROUND))
            .map_err(|error| error.to_string())?;
        let fonts = Fonts::load()?;
        let setup = connection.setup();
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        let mapping = connection
            .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let keysyms = (
            min_keycode,
            usize::from(mapping.keysyms_per_keycode),
            mapping.keysyms,
        );
        let bar = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        let mark = resample(&decode_png(MARK)?, 18, 18);

        let mut desktop = Self {
            connection,
            root: screen.root,
            width: screen.width_in_pixels,
            height: screen.height_in_pixels,
            depth: screen.root_depth,
            atoms,
            gc,
            bar,
            order: Vec::new(),
            clients: HashMap::new(),
            active: None,
            requests,
            tray: Vec::new(),
            popup: None,
            fonts,
            mark,
            layout: Layout::default(),
            lease: None,
            keysyms,
            started: Instant::now(),
            shown: String::new(),
        };
        desktop.announce()?;
        desktop.paint_wallpaper()?;
        desktop.create_bar(screen.root_visual)?;
        desktop.own_tray()?;
        desktop.adopt_existing()?;
        desktop
            .connection
            .flush()
            .map_err(|error| error.to_string())?;
        spawn_ticker(display.to_owned());
        Ok(desktop)
    }

    /// The bar always retains its own work area.
    fn work_height(&self) -> u16 {
        self.height.saturating_sub(BAR_HEIGHT)
    }

    fn announce(&self) -> Result<(), String> {
        let check = self
            .connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        self.connection
            .create_window(
                0,
                check,
                self.root,
                -1,
                -1,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().override_redirect(1),
            )
            .map_err(|error| error.to_string())?;
        let atoms = &self.atoms;
        self.property32(
            check,
            atoms.net_supporting_wm_check,
            AtomEnum::WINDOW,
            &[check],
        )?;
        self.connection
            .change_property8(
                PropMode::REPLACE,
                check,
                atoms.net_wm_name,
                atoms.utf8_string,
                b"toad-computer",
            )
            .map_err(|error| error.to_string())?;
        let root = self.root;
        self.property32(
            root,
            atoms.net_supporting_wm_check,
            AtomEnum::WINDOW,
            &[check],
        )?;
        self.property32(
            root,
            atoms.net_supported,
            AtomEnum::ATOM,
            &atoms.supported(),
        )?;
        self.property32(root, atoms.net_number_of_desktops, AtomEnum::CARDINAL, &[1])?;
        self.property32(root, atoms.net_current_desktop, AtomEnum::CARDINAL, &[0])?;
        self.property32(
            root,
            atoms.net_desktop_geometry,
            AtomEnum::CARDINAL,
            &[u32::from(self.width), u32::from(self.height)],
        )?;
        self.property32(
            root,
            atoms.net_desktop_viewport,
            AtomEnum::CARDINAL,
            &[0, 0],
        )?;
        self.property32(
            root,
            atoms.net_workarea,
            AtomEnum::CARDINAL,
            &[
                0,
                u32::from(BAR_HEIGHT),
                u32::from(self.width),
                u32::from(self.work_height()),
            ],
        )?;
        self.property32(root, atoms.net_client_list, AtomEnum::WINDOW, &[])?;
        self.property32(root, atoms.net_active_window, AtomEnum::WINDOW, &[])?;
        Ok(())
    }

    fn property32(
        &self,
        window: Window,
        property: Atom,
        kind: AtomEnum,
        data: &[u32],
    ) -> Result<(), String> {
        self.connection
            .change_property32(PropMode::REPLACE, window, property, kind, data)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn property_text(&self, window: Window, property: Atom, text: &str) -> Result<(), String> {
        self.connection
            .change_property8(
                PropMode::REPLACE,
                window,
                property,
                self.atoms.utf8_string,
                text.as_bytes(),
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Black, with the mark centred in the work area below the bar.
    fn paint_wallpaper(&self) -> Result<(), String> {
        let pixmap: Pixmap = self
            .connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        self.connection
            .create_pixmap(self.depth, pixmap, self.root, self.width, self.height)
            .map_err(|error| error.to_string())?;
        self.fill(pixmap, BACKGROUND, 0, 0, self.width, self.height)?;
        let mark = decode_png(MARK)?;
        let x = (self.width.saturating_sub(mark.width) / 2) as i16;
        let y = (BAR_HEIGHT + self.work_height().saturating_sub(mark.height) / 2) as i16;
        self.put_image(pixmap, &mark, x, y, BACKGROUND)?;
        self.connection
            .change_window_attributes(
                self.root,
                &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .clear_area(false, self.root, 0, 0, 0, 0)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn create_bar(&mut self, visual: u32) -> Result<(), String> {
        self.connection
            .create_window(
                self.depth,
                self.bar,
                self.root,
                0,
                0,
                self.width,
                BAR_HEIGHT,
                0,
                WindowClass::INPUT_OUTPUT,
                visual,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .background_pixel(BAR_FILL)
                    .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS),
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .map_window(self.bar)
            .map_err(|error| error.to_string())?;
        self.draw_bar()
    }

    // ---- what the bar knows -------------------------------------------------

    fn job_counts(&self) -> (u32, u32, u32) {
        let counts = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms.jobs_running,
                AtomEnum::CARDINAL,
                0,
                3,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(Iterator::collect::<Vec<_>>))
            .unwrap_or_default();
        (
            counts.first().copied().unwrap_or(0),
            counts.get(1).copied().unwrap_or(0),
            counts.get(2).copied().unwrap_or(0),
        )
    }

    fn read_lease(&mut self) -> Result<(), String> {
        let value = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms.holder,
                self.atoms.utf8_string,
                0,
                4096,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map(|reply| reply.value)
            .unwrap_or_default();
        self.lease = serde_json::from_slice::<serde_json::Value>(&value)
            .ok()
            .and_then(|json| {
                Some(Lease {
                    holder: json["holder"].as_str()?.to_owned(),
                    expires_ms: json["expires_ms"].as_u64()?,
                })
            });
        Ok(())
    }

    fn holding(&self) -> Holding {
        match &self.lease {
            None => Holding::Nobody,
            // A person's hold is renewed by every twitch of their hands and
            // ends when they hand the screen back, so it is not timed out here.
            Some(lease) if lease.holder.starts_with(crate::viewer::PERSON) => Holding::Person,
            Some(lease) if lease.expires_ms > now_ms() => Holding::Agent,
            Some(_) => Holding::Nobody,
        }
    }

    fn clock(&self) -> String {
        chrono::Local::now().format("%H:%M").to_string()
    }

    fn title_of(&self, window: Window) -> Result<String, String> {
        let title = self
            .connection
            .get_property(
                false,
                window,
                self.atoms.net_wm_name,
                self.atoms.utf8_string,
                0,
                256,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map(|r| String::from_utf8_lossy(&r.value).into_owned())
            .unwrap_or_default();
        Ok(if title.is_empty() {
            self.clients
                .get(&window)
                .map(|c| {
                    c.class
                        .split('\0')
                        .find(|part| !part.is_empty())
                        .unwrap_or("window")
                        .to_owned()
                })
                .unwrap_or_default()
        } else {
            title
        })
    }

    /// The window's own icon at the bar's size: the smallest it offers that
    /// is at least that size, else the largest.
    fn icon_of(&self, window: Window) -> Option<Image> {
        let data = self
            .connection
            .get_property(
                false,
                window,
                self.atoms.net_wm_icon,
                AtomEnum::CARDINAL,
                0,
                1 << 20,
            )
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .collect::<Vec<u32>>();
        best_icon(&data, APP_ICON)
    }

    // ---- the bar -----------------------------------------------------------

    fn draw_bar(&mut self) -> Result<(), String> {
        let width = i32::from(self.width);
        let height = i32::from(BAR_HEIGHT);
        let sans = Face::new(&self.fonts.sans, 13.0);
        let small = Face::new(&self.fonts.sans, 12.0);
        let mono = Face::new(&self.fonts.mono, 10.0);
        let mut canvas = Canvas::new(self.width, BAR_HEIGHT, BAR_FILL);
        canvas.fill_rect(0, height - 1, width, 1, RULE);
        // The mark is the menu.
        let mut layout = Layout {
            mark: Rect {
                x: 8,
                y: 5,
                w: 28,
                h: 26,
            },
            ..Layout::default()
        };
        if matches!(
            self.popup,
            Some(Open {
                kind: Popup::Menu,
                ..
            })
        ) {
            canvas.round_rect(
                layout.mark.x,
                layout.mark.y,
                layout.mark.w,
                layout.mark.h,
                6.0,
                BAR_HOVER,
            );
        }
        canvas.stamp(layout.mark.x + 5, layout.mark.y + 4, &self.mark, INK);

        // The right cluster is laid out from the edge inwards.
        let clock = self.clock();
        let clock_width = sans.width(&clock);
        let mut right = width - 10;
        layout.clock = Rect {
            x: right - clock_width - 4,
            y: 0,
            w: clock_width + 8,
            h: height,
        };
        canvas.text(
            &sans,
            layout.clock.x + 4,
            sans.baseline_in(0, height),
            &clock,
            INK,
        );
        right = layout.clock.x - 4;
        if !self.tray.is_empty() {
            let count = self.tray.len() as i32;
            let tray_width = 8 + count * TRAY_ICON + (count - 1) * 6 + 8;
            let left = right - tray_width;
            canvas.fill_rect(left, 8, 1, 20, RULE);
            canvas.fill_rect(right, 8, 1, 20, RULE);
            for index in 0..count {
                layout.tray.push(Rect {
                    x: left + 8 + index * (TRAY_ICON + 6),
                    y: (height - TRAY_ICON) / 2,
                    w: TRAY_ICON,
                    h: TRAY_ICON,
                });
            }
            right = left - 6;
        }
        let (text, color, fill) = match self.holding() {
            Holding::Person => ("person in control", YOU, Some(YOU_FILL)),
            Holding::Agent => ("agent in control", INK_2, None),
            Holding::Nobody => ("agent at work", MUTED, None),
        };
        layout.lease = chip(&mut canvas, &small, right, text, color, color, fill);
        right = layout.lease.x - 4;
        let (running, completed, failed) = self.job_counts();
        let jobs_text = if failed > 0 {
            format!("{failed} failed · {running} running")
        } else if running > 0 {
            format!("{running} running · {completed} done")
        } else if completed > 0 {
            format!("{completed} done")
        } else {
            "no jobs".to_owned()
        };
        let (jobs_color, jobs_dot) = if failed > 0 {
            (WARN, WARN)
        } else if running > 0 {
            (INK, OK)
        } else {
            (MUTED, MUTED)
        };
        layout.jobs = chip(
            &mut canvas,
            &small,
            right,
            &jobs_text,
            jobs_color,
            jobs_dot,
            None,
        );
        right = layout.jobs.x - 16;

        // The middle is every managed window, in mapping order, as far as fits.
        canvas.fill_rect(44, 9, 1, 18, RULE);
        let mut x = 52;
        for window in self.order.clone() {
            let title = self.title_of(window)?;
            let text = sans.fit(&title, PILL_MAX - 8 - i32::from(APP_ICON) - 7 - 10);
            let pill = Rect {
                x,
                y: 5,
                w: 8 + i32::from(APP_ICON) + 7 + sans.width(&text) + 10,
                h: 26,
            };
            if pill.x + pill.w > right {
                break;
            }
            let active = self.active == Some(window);
            if active {
                canvas.round_rect(pill.x, pill.y, pill.w, pill.h, 6.0, BAR_RAISED);
                canvas.fill_rect(pill.x + 6, pill.y + pill.h - 2, pill.w - 12, 2, INK_2);
            }
            let icon_x = pill.x + 8;
            let icon_y = pill.y + (pill.h - i32::from(APP_ICON)) / 2;
            let client = self.clients.get(&window);
            match client.and_then(|c| c.icon.as_ref()) {
                Some(icon) => canvas.blit(icon_x, icon_y, icon),
                None => {
                    canvas.round_rect(icon_x, icon_y, 16, 16, 4.0, BAR_HOVER);
                    let glyph = if client.is_some_and(|c| c.class.contains("toadterminal")) {
                        ">_".to_owned()
                    } else if client.is_some_and(|c| c.class.contains("toadshell")) {
                        "$".to_owned()
                    } else {
                        title
                            .chars()
                            .next()
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_default()
                    };
                    let glyph_width = mono.width(&glyph);
                    canvas.text(
                        &mono,
                        icon_x + (16 - glyph_width) / 2,
                        mono.baseline_in(icon_y, 16),
                        &glyph,
                        INK,
                    );
                }
            }
            canvas.text(
                &sans,
                icon_x + i32::from(APP_ICON) + 7,
                sans.baseline_in(pill.y, pill.h),
                &text,
                if active { INK } else { INK_2 },
            );
            layout.apps.push((pill, window));
            x += pill.w + 2;
        }

        self.put_image(self.bar, &canvas.image, 0, 0, BAR_FILL)?;
        self.layout = layout;
        self.shown = format!("{clock}|{:?}", self.holding());
        self.layout_tray()?;
        self.publish_layout()
    }

    /// Tests and tools find the bar's parts here rather than by guessing pixels.
    fn publish_layout(&self) -> Result<(), String> {
        let layout = json!({
            "height": BAR_HEIGHT,
            "mark": self.layout.mark.json(),
            "apps": self.layout.apps.iter().map(|(rect, window)| json!({"window": window, "rect": rect.json()})).collect::<Vec<_>>(),
            "jobs": self.layout.jobs.json(),
            "lease": self.layout.lease.json(),
            "tray": self.layout.tray.iter().map(Rect::json).collect::<Vec<_>>(),
            "clock": self.layout.clock.json(),
            "popup": {
                "x": POPUP_X,
                "y": POPUP_Y,
                "jobs_x": self.jobs_popup_x(self.width.saturating_sub(16).min(640)),
                "row": ROW,
                "pad": PAD,
            },
        });
        self.property_text(self.root, self.atoms.layout, &layout.to_string())
    }

    /// A second has passed: redraw only if the bar would read differently.
    fn tick(&mut self) -> Result<(), String> {
        let now = format!("{}|{:?}", self.clock(), self.holding());
        if now != self.shown {
            self.draw_bar()?;
        }
        Ok(())
    }

    // ---- popups: the toad menu, the jobs list, about ------------------------

    fn open_popup(&mut self, kind: Popup) -> Result<(), String> {
        self.close_popup()?;
        let (width, height) = match &kind {
            Popup::Menu => (MENU_WIDTH, (PAD + 4 * ROW + 11 + 22 + PAD) as u16),
            Popup::Jobs { visible, .. } => (
                self.width.saturating_sub(16).min(640),
                ((*visible as i32 + 1) * ROW + 2 * PAD) as u16,
            ),
            Popup::About => (ABOUT_WIDTH, (PAD + 8 + 22 + 6 * 18 + 8 + PAD) as u16),
        };
        let x = match &kind {
            Popup::Jobs { .. } => self.jobs_popup_x(width),
            _ => POPUP_X,
        };
        let window = self.connection.generate_id().map_err(|e| e.to_string())?;
        self.connection
            .create_window(
                self.depth,
                window,
                self.root,
                x,
                POPUP_Y,
                width,
                height,
                1,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .background_pixel(BAR_FILL)
                    .border_pixel(RULE)
                    .event_mask(
                        EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS,
                    ),
            )
            .map_err(|e| e.to_string())?;
        self.connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        self.connection
            .configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .map_err(|e| e.to_string())?;
        // The letters go to the menu while it is open, wherever focus was.
        let _ = self
            .connection
            .grab_keyboard(
                false,
                window,
                CURRENT_TIME,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(|e| e.to_string())?
            .reply();
        self.popup = Some(Open {
            window,
            kind,
            width,
            height,
        });
        self.draw_popup()?;
        self.draw_bar()
    }

    /// Where a jobs list of `width` sits: under the jobs chip, right edges
    /// aligned, kept on the screen.
    fn jobs_popup_x(&self, width: u16) -> i16 {
        let chip = self.layout.jobs;
        let right = if chip.w > 0 {
            chip.x + chip.w
        } else {
            i32::from(self.width) - EDGE
        };
        (right - i32::from(width) - 2).clamp(
            EDGE,
            (i32::from(self.width) - i32::from(width) - EDGE - 2).max(EDGE),
        ) as i16
    }

    fn close_popup(&mut self) -> Result<(), String> {
        if let Some(open) = self.popup.take() {
            self.connection
                .ungrab_keyboard(CURRENT_TIME)
                .map_err(|e| e.to_string())?;
            self.connection
                .destroy_window(open.window)
                .map_err(|e| e.to_string())?;
            self.draw_bar()?;
        }
        Ok(())
    }

    fn open_jobs(&mut self) -> Result<(), String> {
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms.jobs_summary,
                self.atoms.utf8_string,
                0,
                65536,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let jobs: Vec<crate::jobs::Summary> =
            serde_json::from_slice(&reply.value).unwrap_or_default();
        let visible = jobs
            .len()
            .min(usize::from(self.work_height().saturating_sub(30) / 30).min(12));
        self.open_popup(Popup::Jobs {
            jobs,
            first: 0,
            visible,
        })
    }

    fn draw_popup(&self) -> Result<(), String> {
        let Some(open) = &self.popup else {
            return Ok(());
        };
        let sans = Face::new(&self.fonts.sans, 13.0);
        let small = Face::new(&self.fonts.sans, 12.0);
        let mono = Face::new(&self.fonts.mono, 11.0);
        let width = i32::from(open.width);
        let mut canvas = Canvas::new(open.width, open.height, BAR_FILL);
        match &open.kind {
            Popup::Menu => {
                let rows: [(&str, &str, &str); 4] = [
                    ("Browser", "open or focus", "B"),
                    ("Terminal", "a shell of your own", "T"),
                    ("Observe", "the teammate's jobs", "O"),
                    ("About this computer", "", "A"),
                ];
                for (index, (label, note, key)) in rows.iter().enumerate() {
                    let y = menu_row_y(index);
                    let baseline = sans.baseline_in(y, ROW);
                    let advance = canvas.text(&sans, PAD + 10, baseline, label, INK);
                    if !note.is_empty() {
                        canvas.text(&small, PAD + 10 + advance + 6, baseline, note, MUTED);
                    }
                    let cap = Rect {
                        x: width - PAD - 8 - 18,
                        y: y + (ROW - 16) / 2,
                        w: 18,
                        h: 16,
                    };
                    canvas.round_rect(cap.x, cap.y, cap.w, cap.h + 1, 4.0, RULE);
                    canvas.round_rect(cap.x, cap.y, cap.w, cap.h, 4.0, BAR_RAISED);
                    canvas.text(
                        &mono,
                        cap.x + (cap.w - mono.width(key)) / 2,
                        mono.baseline_in(cap.y, cap.h),
                        key,
                        INK_2,
                    );
                }
                canvas.fill_rect(PAD + 4, PAD + 3 * ROW + 5, width - 2 * PAD - 8, 1, RULE);
                let foot = format!(
                    "toad-computer {} · {}",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::ARCH
                );
                canvas.text(
                    &mono,
                    PAD + 10,
                    mono.baseline_in(PAD + 4 * ROW + 11, 22),
                    &foot,
                    MUTED,
                );
            }
            Popup::Jobs {
                jobs,
                first,
                visible,
            } => {
                let baseline = sans.baseline_in(PAD, ROW);
                let advance = canvas.text(&sans, PAD + 10, baseline, "Jobs", INK);
                canvas.text(
                    &small,
                    PAD + 10 + advance + 8,
                    baseline,
                    "click a job to inspect it · scroll for more",
                    MUTED,
                );
                let open_terminal = "open terminal";
                canvas.text(
                    &small,
                    width - PAD - 10 - small.width(open_terminal),
                    baseline,
                    open_terminal,
                    INK_2,
                );
                for (row, job) in jobs.iter().skip(*first).take(*visible).enumerate() {
                    let y = PAD + (row as i32 + 1) * ROW;
                    let (dot, note) = if job.state == "running" {
                        (OK, "running".to_owned())
                    } else if job.exit_code == Some(0) {
                        (MUTED, "exit 0".to_owned())
                    } else {
                        (
                            WARN,
                            job.exit_code
                                .map_or_else(|| job.state.clone(), |code| format!("exit {code}")),
                        )
                    };
                    canvas.dot((PAD + 14) as f32, (y + ROW / 2) as f32, 3.0, dot);
                    let note_width = mono.width(&note);
                    let label = sans.fit(&job.label, width - PAD - 26 - note_width - 20 - PAD);
                    canvas.text(&sans, PAD + 26, sans.baseline_in(y, ROW), &label, INK_2);
                    canvas.text(
                        &mono,
                        width - PAD - 10 - note_width,
                        mono.baseline_in(y, ROW),
                        &note,
                        MUTED,
                    );
                }
            }
            Popup::About => {
                canvas.stamp(PAD + 10, PAD + 8 + 3, &self.mark, INK);
                canvas.text(
                    &sans,
                    PAD + 10 + 18 + 8,
                    sans.baseline_in(PAD + 8, 22),
                    "About this computer",
                    INK,
                );
                let identity = crate::guide::identity();
                let uptime = self.started.elapsed().as_secs();
                let rows = [
                    ("version", env!("CARGO_PKG_VERSION").to_owned()),
                    (
                        "channel",
                        identity["channel"].as_str().unwrap_or("unknown").to_owned(),
                    ),
                    (
                        "revision",
                        identity["revision"]
                            .as_str()
                            .unwrap_or("unknown")
                            .chars()
                            .take(12)
                            .collect(),
                    ),
                    (
                        "arch",
                        format!(
                            "{} · {}×{}",
                            std::env::consts::ARCH,
                            self.width,
                            self.height
                        ),
                    ),
                    (
                        "nixpkgs",
                        crate::workspace::NIXPKGS.chars().take(12).collect(),
                    ),
                    (
                        "uptime",
                        format!("{} h {:02} min", uptime / 3600, uptime % 3600 / 60),
                    ),
                ];
                for (index, (key, value)) in rows.iter().enumerate() {
                    let y = PAD + 8 + 22 + index as i32 * 18;
                    canvas.text(&small, PAD + 10, small.baseline_in(y, 18), key, MUTED);
                    canvas.text(&mono, PAD + 10 + 70, mono.baseline_in(y, 18), value, INK_2);
                }
            }
        }
        self.put_image(open.window, &canvas.image, 0, 0, BAR_FILL)
    }

    /// The menu's letter or click: A to D.
    fn menu_pick(&mut self, index: usize) -> Result<(), String> {
        match index {
            0 => {
                self.close_popup()?;
                self.dock_action(Request::OpenBrowser)
            }
            1 => {
                self.close_popup()?;
                self.dock_action(Request::OpenShell)
            }
            2 => {
                self.close_popup()?;
                self.dock_action(Request::OpenTerminal)
            }
            3 => self.open_popup(Popup::About),
            _ => Ok(()),
        }
    }

    fn keysym(&self, keycode: u8) -> u32 {
        let (min, per, keysyms) = &self.keysyms;
        keysyms
            .get(usize::from(keycode.saturating_sub(*min)) * per)
            .copied()
            .unwrap_or(0)
    }

    fn key_press(&mut self, event: KeyPressEvent) -> Result<(), String> {
        let Some(open) = &self.popup else {
            return Ok(());
        };
        let keysym = self.keysym(event.detail);
        if keysym == KEYSYM_ESCAPE {
            return self.close_popup();
        }
        if matches!(open.kind, Popup::Menu)
            && let Some(index) =
                MENU_KEYS.find(|c| u32::from(c) == keysym || u32::from(c) - 32 == keysym)
        {
            return self.menu_pick(index);
        }
        Ok(())
    }

    fn popup_click(&mut self, event: &ButtonPressEvent) -> Result<(), String> {
        let Some(open) = &mut self.popup else {
            return Ok(());
        };
        let y = i32::from(event.event_y);
        match &mut open.kind {
            Popup::Menu => {
                let index = (0..4)
                    .find(|index| (menu_row_y(*index)..menu_row_y(*index) + ROW).contains(&y));
                match index {
                    Some(index) => self.menu_pick(index),
                    None => Ok(()),
                }
            }
            Popup::About => self.close_popup(),
            Popup::Jobs {
                jobs,
                first,
                visible,
            } => {
                if event.detail == 4 || event.detail == 5 {
                    *first = if event.detail == 4 {
                        first.saturating_sub(1)
                    } else {
                        (*first + 1).min(jobs.len().saturating_sub(*visible))
                    };
                    return self.draw_popup();
                }
                let row = usize::try_from((y - PAD).max(0)).unwrap_or(0) / ROW as usize;
                let request = if row == 0 {
                    Request::OpenTerminal
                } else {
                    let Some(job) = jobs.get(*first + row - 1) else {
                        return Ok(());
                    };
                    Request::OpenJob(job.id.clone())
                };
                self.close_popup()?;
                self.dock_action(request)
            }
        }
    }

    // ---- the tray ---------------------------------------------------------

    fn own_tray(&self) -> Result<(), String> {
        let orientation = self
            .connection
            .intern_atom(false, b"_NET_SYSTEM_TRAY_ORIENTATION")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        self.property32(self.bar, orientation, AtomEnum::CARDINAL, &[0])?;
        self.connection
            .set_selection_owner(self.bar, self.atoms.tray_selection, CURRENT_TIME)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        let manager = self
            .connection
            .intern_atom(false, b"MANAGER")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::STRUCTURE_NOTIFY,
                ClientMessageEvent::new(
                    32,
                    self.root,
                    manager,
                    ClientMessageData::from([
                        CURRENT_TIME,
                        self.atoms.tray_selection,
                        self.bar,
                        0,
                        0,
                    ]),
                ),
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn embed_tray(&mut self, window: Window) -> Result<(), String> {
        if window == NONE || self.tray.contains(&window) {
            return Ok(());
        }
        self.connection
            .change_save_set(x11rb::protocol::xproto::SetMode::INSERT, window)
            .map_err(|e| e.to_string())?;
        self.connection
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|e| e.to_string())?;
        self.connection
            .reparent_window(window, self.bar, 0, 9)
            .map_err(|e| e.to_string())?;
        self.connection
            .send_event(
                false,
                window,
                EventMask::NO_EVENT,
                ClientMessageEvent::new(
                    32,
                    window,
                    self.atoms.xembed,
                    ClientMessageData::from([CURRENT_TIME, 0, 0, self.bar, 0]),
                ),
            )
            .map_err(|e| e.to_string())?;
        self.tray.push(window);
        self.draw_bar()?;
        self.connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn layout_tray(&self) -> Result<(), String> {
        for (window, slot) in self.tray.iter().zip(&self.layout.tray) {
            self.connection
                .configure_window(
                    *window,
                    &ConfigureWindowAux::new()
                        .x(slot.x)
                        .y(slot.y)
                        .width(slot.w as u32)
                        .height(slot.h as u32),
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // ---- pixels to the server ----------------------------------------------

    fn fill(
        &self,
        drawable: u32,
        color: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    ) -> Result<(), String> {
        self.connection
            .change_gc(
                self.gc,
                &x11rb::protocol::xproto::ChangeGCAux::new().foreground(color),
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .poly_fill_rectangle(
                drawable,
                self.gc,
                &[Rectangle {
                    x,
                    y,
                    width,
                    height,
                }],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Composite an RGBA image onto a solid colour and send it as 24-bit
    /// ZPixmap rows, in chunks small enough for any server's request limit.
    fn put_image(
        &self,
        drawable: u32,
        image: &Image,
        x: i16,
        y: i16,
        background: u32,
    ) -> Result<(), String> {
        let stride = usize::from(image.width) * 4;
        let pixels = composite(image, background);
        let rows_per_chunk = (PUT_IMAGE_CHUNK / stride.max(1)).max(1);
        for (chunk, rows) in pixels.chunks(stride * rows_per_chunk).enumerate() {
            let row_count = (rows.len() / stride) as u16;
            self.connection
                .put_image(
                    ImageFormat::Z_PIXMAP,
                    drawable,
                    self.gc,
                    image.width,
                    row_count,
                    x,
                    y + (chunk * rows_per_chunk) as i16,
                    0,
                    self.depth,
                    rows,
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    // ---- the window manager -------------------------------------------------

    fn adopt_existing(&mut self) -> Result<(), String> {
        let children = self
            .connection
            .query_tree(self.root)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?
            .children;
        for window in children {
            let Ok(attributes) = self
                .connection
                .get_window_attributes(window)
                .map_err(|error| error.to_string())?
                .reply()
            else {
                continue;
            };
            if attributes.map_state == MapState::VIEWABLE && !attributes.override_redirect {
                self.manage(window)?;
            }
        }
        Ok(())
    }

    fn handle(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::MapRequest(event) => self.manage(event.window),
            Event::ConfigureRequest(event) => self.configure_request(event),
            Event::UnmapNotify(event) => self.forget(event.window),
            Event::DestroyNotify(event) if self.tray.contains(&event.window) => {
                self.tray.retain(|w| *w != event.window);
                self.draw_bar()
            }
            Event::DestroyNotify(event) => self.forget(event.window),
            Event::PropertyNotify(event) if event.window == self.root => {
                if event.atom == self.atoms.jobs_running {
                    self.draw_bar()
                } else if event.atom == self.atoms.holder {
                    self.read_lease()?;
                    self.draw_bar()
                } else if event.atom == self.atoms.tick {
                    self.tick()
                } else {
                    Ok(())
                }
            }
            Event::PropertyNotify(event) if self.clients.contains_key(&event.window) => {
                if event.atom == self.atoms.net_wm_name {
                    self.draw_bar()
                } else if event.atom == self.atoms.net_wm_icon {
                    let icon = self.icon_of(event.window);
                    if let Some(client) = self.clients.get_mut(&event.window) {
                        client.icon = icon;
                    }
                    self.draw_bar()
                } else {
                    Ok(())
                }
            }
            Event::ClientMessage(event) => {
                let data = event.data.as_data32();
                if event.type_ == self.atoms.tray_opcode && data[1] == 0 {
                    return self.embed_tray(data[2]);
                }
                if event.type_ == self.atoms.net_wm_state {
                    self.change_state(event.window, data[0], [data[1], data[2]])
                } else if event.type_ == self.atoms.net_active_window {
                    if self.clients.contains_key(&event.window) {
                        self.focus(event.window)?;
                    }
                    Ok(())
                } else if event.type_ == self.atoms.net_close_window {
                    self.close(event.window)
                } else {
                    Ok(())
                }
            }
            Event::ButtonPress(event) => self.button_press(event),
            Event::KeyPress(event) => self.key_press(event),
            Event::Expose(event)
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|open| open.window == event.window)
                    && event.count == 0 =>
            {
                self.draw_popup()
            }
            Event::Expose(event) if event.window == self.bar && event.count == 0 => self.draw_bar(),
            _ => Ok(()),
        }
    }

    fn manage(&mut self, window: Window) -> Result<(), String> {
        let Ok(attributes) = self
            .connection
            .get_window_attributes(window)
            .map_err(|error| error.to_string())?
            .reply()
        else {
            return Ok(());
        };
        if self.tray.contains(&window) {
            self.connection
                .map_window(window)
                .map_err(|e| e.to_string())?;
            return self.layout_tray();
        }
        if attributes.override_redirect {
            return Ok(());
        }
        if self.clients.contains_key(&window) {
            self.connection
                .map_window(window)
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let geometry = self
            .connection
            .get_geometry(window)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?;
        let class = self.class_of(window)?;
        let icon = if class.contains("toadterminal") || class.contains("toadshell") {
            None
        } else {
            self.icon_of(window)
        };
        let client = match self.kind(window)? {
            Kind::Furniture => {
                self.connection
                    .map_window(window)
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
            // The person's shell is a small window in the bottom-left corner,
            // over whatever the teammate has open, a hair in from the edge.
            Kind::Normal if class.contains("toadshell") => Client {
                maximized: false,
                saved: {
                    let width = (self.width * 2 / 5).max(640).min(self.width);
                    let height = (self.work_height() * 2 / 5)
                        .max(320)
                        .min(self.work_height());
                    (
                        SHELL_INSET as i16,
                        (self.height - height - SHELL_INSET) as i16,
                        width,
                        height,
                    )
                },
                class,
                icon,
            },
            Kind::Normal if class.contains("toadterminal") => Client {
                maximized: false,
                saved: (
                    (self.width * 2 / 3) as i16,
                    BAR_HEIGHT as i16,
                    self.width - self.width * 2 / 3,
                    self.work_height(),
                ),
                class,
                icon,
            },
            Kind::Normal => Client {
                maximized: true,
                saved: (geometry.x, geometry.y, geometry.width, geometry.height),
                class,
                icon,
            },
            Kind::Dialog => {
                let width = geometry.width.min(self.width);
                let height = geometry.height.min(self.work_height());
                Client {
                    maximized: false,
                    saved: (
                        ((self.width - width) / 2) as i16,
                        (BAR_HEIGHT + (self.work_height() - height) / 2) as i16,
                        width,
                        height,
                    ),
                    class,
                    icon,
                }
            }
        };
        // The terminal keeps the right third; the app being watched keeps the rest.
        if client.class.contains("toadterminal") {
            let width = self.width * 2 / 3;
            let height = self.work_height();
            let others: Vec<_> = self
                .clients
                .iter()
                .filter(|(_, c)| c.maximized)
                .map(|(id, _)| *id)
                .collect();
            for id in others {
                if let Some(other) = self.clients.get_mut(&id) {
                    other.maximized = false;
                    other.saved = (0, BAR_HEIGHT as i16, width, height);
                }
                self.apply_geometry(id)?;
            }
        }
        self.connection
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(|e| e.to_string())?;
        self.clients.insert(window, client);
        self.order.push(window);
        self.apply_geometry(window)?;
        self.property32(
            window,
            self.atoms.net_frame_extents,
            AtomEnum::CARDINAL,
            &[0, 0, 0, 0],
        )?;
        // A click anywhere in the window focuses it, then reaches the window
        // as if nothing had been in the way.
        self.connection
            .grab_button(
                true,
                window,
                EventMask::BUTTON_PRESS,
                GrabMode::SYNC,
                GrabMode::ASYNC,
                NONE,
                NONE,
                ButtonIndex::M1,
                ModMask::ANY,
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .map_window(window)
            .map_err(|error| error.to_string())?;
        self.publish_client_list()?;
        self.focus(window)
    }

    fn kind(&self, window: Window) -> Result<Kind, String> {
        let types = self
            .connection
            .get_property(
                false,
                window,
                self.atoms.net_wm_window_type,
                AtomEnum::ATOM,
                0,
                32,
            )
            .map_err(|error| error.to_string())?
            .reply()
            .ok()
            .and_then(|reply| reply.value32().map(Iterator::collect::<Vec<Atom>>))
            .unwrap_or_default();
        let atoms = &self.atoms;
        let furniture = [
            atoms.window_type_dock,
            atoms.window_type_desktop,
            atoms.window_type_toolbar,
            atoms.window_type_menu,
            atoms.window_type_splash,
            atoms.window_type_notification,
        ];
        if types.iter().any(|kind| furniture.contains(kind)) {
            return Ok(Kind::Furniture);
        }
        if types.contains(&atoms.window_type_dialog) {
            return Ok(Kind::Dialog);
        }
        let transient = self
            .connection
            .get_property(
                false,
                window,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .map_err(|error| error.to_string())?
            .reply()
            .map(|reply| reply.value32().is_some_and(|mut ids| ids.next().is_some()))
            .unwrap_or(false);
        Ok(if transient {
            Kind::Dialog
        } else {
            Kind::Normal
        })
    }

    /// Put the window where its state says, tell it so, and record the
    /// state where other clients read it.
    fn apply_geometry(&self, window: Window) -> Result<(), String> {
        let Some(client) = self.clients.get(&window) else {
            return Ok(());
        };
        let (x, y, width, height) = if client.maximized {
            (0, BAR_HEIGHT as i16, self.width, self.work_height())
        } else {
            client.saved
        };
        self.connection
            .configure_window(
                window,
                &ConfigureWindowAux::new()
                    .x(i32::from(x))
                    .y(i32::from(y))
                    .width(u32::from(width))
                    .height(u32::from(height))
                    .border_width(0),
            )
            .map_err(|error| error.to_string())?;
        let notify = ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: window,
            window,
            above_sibling: NONE,
            x,
            y,
            width,
            height,
            border_width: 0,
            override_redirect: false,
        };
        self.connection
            .send_event(false, window, EventMask::STRUCTURE_NOTIFY, notify)
            .map_err(|error| error.to_string())?;
        let state: Vec<Atom> = if client.maximized {
            vec![self.atoms.maximized_vert, self.atoms.maximized_horz]
        } else {
            Vec::new()
        };
        self.property32(window, self.atoms.net_wm_state, AtomEnum::ATOM, &state)
    }

    fn configure_request(&mut self, event: ConfigureRequestEvent) -> Result<(), String> {
        if self.tray.contains(&event.window) {
            return self.layout_tray();
        }
        match self.clients.get_mut(&event.window) {
            Some(client) if client.maximized => self.apply_geometry(event.window),
            Some(client) => {
                if event.value_mask.contains(ConfigWindow::X) {
                    client.saved.0 = event.x;
                }
                if event.value_mask.contains(ConfigWindow::Y) {
                    client.saved.1 = event.y;
                }
                if event.value_mask.contains(ConfigWindow::WIDTH) {
                    client.saved.2 = event.width;
                }
                if event.value_mask.contains(ConfigWindow::HEIGHT) {
                    client.saved.3 = event.height;
                }
                self.connection
                    .configure_window(
                        event.window,
                        &ConfigureWindowAux::from_configure_request(&event),
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            }
            None => {
                self.connection
                    .configure_window(
                        event.window,
                        &ConfigureWindowAux::from_configure_request(&event),
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            }
        }
    }

    fn change_state(
        &mut self,
        window: Window,
        action: u32,
        properties: [Atom; 2],
    ) -> Result<(), String> {
        let about_maximize = properties
            .iter()
            .any(|atom| *atom == self.atoms.maximized_vert || *atom == self.atoms.maximized_horz);
        if !about_maximize {
            return Ok(());
        }
        let Some(client) = self.clients.get_mut(&window) else {
            return Ok(());
        };
        let maximized = match action {
            0 => false,
            1 => true,
            2 => !client.maximized,
            _ => return Ok(()),
        };
        if maximized == client.maximized {
            return Ok(());
        }
        if maximized {
            let geometry = self
                .connection
                .get_geometry(window)
                .map_err(|error| error.to_string())?
                .reply()
                .map_err(|error| error.to_string())?;
            client.saved = (geometry.x, geometry.y, geometry.width, geometry.height);
        }
        client.maximized = maximized;
        self.apply_geometry(window)
    }

    fn focus(&mut self, window: Window) -> Result<(), String> {
        self.connection
            .set_input_focus(InputFocus::POINTER_ROOT, window, CURRENT_TIME)
            .map_err(|error| error.to_string())?;
        self.connection
            .configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .configure_window(
                self.bar,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .map_err(|error| error.to_string())?;
        if let Some(open) = &self.popup {
            self.connection
                .configure_window(
                    open.window,
                    &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                )
                .map_err(|error| error.to_string())?;
        }
        self.property32(
            self.root,
            self.atoms.net_active_window,
            AtomEnum::WINDOW,
            &[window],
        )?;
        self.active = Some(window);
        self.draw_bar()
    }

    fn forget(&mut self, window: Window) -> Result<(), String> {
        let Some(client) = self.clients.remove(&window) else {
            return Ok(());
        };
        self.order.retain(|client| *client != window);
        self.publish_client_list()?;
        if client.class.contains(BROWSER_CLASS) && self.client_with_class(BROWSER_CLASS).is_none() {
            let _ = self.requests.send(Request::BrowserClosed);
        }
        if self.active == Some(window) {
            self.active = None;
            match self.topmost_client()? {
                Some(next) => self.focus(next)?,
                None => {
                    self.connection
                        .set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME)
                        .map_err(|error| error.to_string())?;
                    self.property32(
                        self.root,
                        self.atoms.net_active_window,
                        AtomEnum::WINDOW,
                        &[],
                    )?;
                }
            }
        }
        self.draw_bar()?;
        Ok(())
    }

    /// The managed window highest in the stack, which is the one a person sees.
    fn topmost_client(&self) -> Result<Option<Window>, String> {
        let children = self
            .connection
            .query_tree(self.root)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?
            .children;
        Ok(children
            .into_iter()
            .rev()
            .find(|window| self.clients.contains_key(window)))
    }

    fn publish_client_list(&self) -> Result<(), String> {
        self.property32(
            self.root,
            self.atoms.net_client_list,
            AtomEnum::WINDOW,
            &self.order,
        )
    }

    fn close(&self, window: Window) -> Result<(), String> {
        let protocols = self
            .connection
            .get_property(
                false,
                window,
                self.atoms.wm_protocols,
                AtomEnum::ATOM,
                0,
                32,
            )
            .map_err(|error| error.to_string())?
            .reply()
            .ok()
            .and_then(|reply| reply.value32().map(Iterator::collect::<Vec<Atom>>))
            .unwrap_or_default();
        if protocols.contains(&self.atoms.wm_delete_window) {
            let message = ClientMessageEvent::new(
                32,
                window,
                self.atoms.wm_protocols,
                ClientMessageData::from([self.atoms.wm_delete_window, CURRENT_TIME, 0, 0, 0]),
            );
            self.connection
                .send_event(false, window, EventMask::NO_EVENT, message)
                .map_err(|error| error.to_string())?;
        } else {
            self.connection
                .kill_client(window)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn button_press(&mut self, event: ButtonPressEvent) -> Result<(), String> {
        if let Some(open) = &self.popup {
            if event.event == open.window {
                return self.popup_click(&event);
            }
            // Any click elsewhere closes the menu. On the mark that is the
            // whole gesture; anywhere else the click goes on to mean itself.
            let was_menu = matches!(open.kind, Popup::Menu);
            self.close_popup()?;
            if event.event == self.bar
                && was_menu
                && self
                    .layout
                    .mark
                    .contains(i32::from(event.event_x), i32::from(event.event_y))
            {
                return Ok(());
            }
        }
        if event.event == self.bar {
            let (x, y) = (i32::from(event.event_x), i32::from(event.event_y));
            if self.layout.mark.contains(x, y) {
                return self.open_popup(Popup::Menu);
            }
            if self.layout.jobs.contains(x, y) {
                return self.open_jobs();
            }
            if let Some((_, window)) = self
                .layout
                .apps
                .iter()
                .find(|(pill, _)| pill.contains(x, y))
            {
                let window = *window;
                return if event.detail == 3 {
                    self.close(window)
                } else {
                    self.focus(window)
                };
            }
            return Ok(());
        }
        if self.clients.contains_key(&event.event) {
            self.focus(event.event)?;
        }
        self.connection
            .allow_events(Allow::REPLAY_POINTER, event.time)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn dock_action(&mut self, request: Request) -> Result<(), String> {
        match request {
            Request::OpenBrowser => {
                if let Some(window) = self.client_with_class(BROWSER_CLASS) {
                    return self.focus(window);
                }
                let _ = self.requests.send(request);
                Ok(())
            }
            Request::OpenTerminal | Request::OpenShell | Request::OpenJob(_) => {
                let _ = self.requests.send(request);
                Ok(())
            }
            Request::BrowserClosed => Ok(()),
        }
    }

    fn class_of(&self, window: Window) -> Result<String, String> {
        let value = self
            .connection
            .get_property(
                false,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                0,
                u32::MAX,
            )
            .map_err(|error| error.to_string())?
            .reply()
            .map(|reply| reply.value)
            .unwrap_or_default();
        Ok(String::from_utf8_lossy(&value).to_ascii_lowercase())
    }

    fn client_with_class(&self, class: &str) -> Option<Window> {
        self.order.iter().rev().copied().find(|window| {
            self.clients
                .get(window)
                .is_some_and(|client| client.class.contains(class))
        })
    }
}

/// The letter that picks each menu row: the row's own initial.
const MENU_KEYS: &str = "btoa";

/// The top of menu row `index`: three rows, a separator, then About.
fn menu_row_y(index: usize) -> i32 {
    if index < 3 {
        PAD + index as i32 * ROW
    } else {
        PAD + 3 * ROW + 11
    }
}

/// A status chip laid out leftwards from `right`: a dot, then a label.
fn chip(
    canvas: &mut Canvas,
    face: &Face,
    right: i32,
    text: &str,
    color: u32,
    dot: u32,
    fill: Option<u32>,
) -> Rect {
    let rect = Rect {
        x: right - (9 + 6 + 6 + face.width(text) + 9),
        y: 6,
        w: 9 + 6 + 6 + face.width(text) + 9,
        h: 24,
    };
    if let Some(fill) = fill {
        canvas.round_rect(rect.x, rect.y, rect.w, rect.h, 12.0, fill);
    }
    canvas.dot((rect.x + 12) as f32, (rect.y + 12) as f32, 3.0, dot);
    canvas.text(
        face,
        rect.x + 21,
        face.baseline_in(rect.y, rect.h),
        text,
        color,
    );
    rect
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `_NET_WM_ICON` is a run of `width, height, pixels…` entries. Pick the one
/// that reduces best to `size` and box-filter it.
fn best_icon(data: &[u32], size: u16) -> Option<Image> {
    let mut best: Option<(u32, u32, &[u32])> = None;
    let mut at = 0;
    while at + 2 <= data.len() {
        let (width, height) = (data[at], data[at + 1]);
        let pixels = (width as usize).checked_mul(height as usize)?;
        if width == 0 || height == 0 || at + 2 + pixels > data.len() {
            break;
        }
        let candidate = (width, height, &data[at + 2..at + 2 + pixels]);
        best = Some(match best {
            None => candidate,
            Some(current) => {
                let big_enough = |w: u32| w >= u32::from(size);
                match (big_enough(current.0), big_enough(width)) {
                    (true, true) if width < current.0 => candidate,
                    (false, true) => candidate,
                    (false, false) if width > current.0 => candidate,
                    _ => current,
                }
            }
        });
        at += 2 + pixels;
    }
    let (width, height, pixels) = best?;
    Image::from_argb_scaled(width, height, pixels, size)
}

/// Once a second a property on the root changes, so the desktop's event loop
/// wakes to move the clock and let a lapsed lease fade from the bar.
fn spawn_ticker(display: String) {
    let _ = std::thread::Builder::new()
        .name("desktop-tick".to_owned())
        .spawn(move || {
            let Ok((connection, screen)) = x11rb::connect(Some(&display)) else {
                return;
            };
            let root = connection.setup().roots[screen].root;
            let Ok(cookie) = connection.intern_atom(false, b"_TOAD_TICK") else {
                return;
            };
            let Ok(reply) = cookie.reply() else {
                return;
            };
            let mut count: u32 = 0;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                count = count.wrapping_add(1);
                if connection
                    .change_property32(
                        PropMode::REPLACE,
                        root,
                        reply.atom,
                        AtomEnum::CARDINAL,
                        &[count],
                    )
                    .is_err()
                    || connection.flush().is_err()
                {
                    return;
                }
            }
        });
}

/// Blend onto a solid colour and lay out as the server's 32-bit ZPixmap: B, G, R, pad.
fn composite(image: &Image, background: u32) -> Vec<u8> {
    let [_, back_r, back_g, back_b] = background.to_be_bytes();
    let mut out = Vec::with_capacity(image.rgba.len());
    for pixel in image.rgba.as_chunks::<4>().0 {
        let alpha = u32::from(pixel[3]);
        let blend = |fore: u8, back: u8| -> u8 {
            ((u32::from(fore) * alpha + u32::from(back) * (255 - alpha)) / 255) as u8
        };
        out.extend_from_slice(&[
            blend(pixel[2], back_b),
            blend(pixel[1], back_g),
            blend(pixel[0], back_r),
            0,
        ]);
    }
    out
}

fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("decode PNG: {error}"))?;
    let mut buffer = vec![0_u8; reader.output_buffer_size().ok_or("decode PNG: no size")?];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| format!("decode PNG: {error}"))?;
    let pixels = &buffer[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        other => return Err(format!("decode PNG: unsupported colour type {other:?}")),
    };
    Ok(Image {
        width: u16::try_from(info.width).map_err(|_| "decode PNG: too wide")?,
        height: u16::try_from(info.height).map_err(|_| "decode PNG: too tall")?,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_decodes_and_blends_onto_black() {
        let mark = decode_png(MARK).unwrap();
        assert_eq!((mark.width, mark.height), (480, 480));
        let pixels = composite(&mark, BACKGROUND);
        assert_eq!(pixels.len(), 480 * 480 * 4);
        // Transparent corners stay black; the body of the mark is grey.
        assert_eq!(&pixels[..4], &[0, 0, 0, 0]);
        let centre = (240 * 480 + 240) * 4;
        assert!(pixels[centre] > 0x30, "centre pixel is the mark's grey");
        let small = resample(&mark, 18, 18);
        assert_eq!((small.width, small.height), (18, 18));
    }

    #[test]
    fn compositing_is_a_straight_alpha_blend() {
        let image = Image {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 128],
        };
        assert_eq!(composite(&image, 0x000000), vec![0, 0, 128, 0]);
        assert_eq!(composite(&image, 0xffffff), vec![127, 127, 255, 0]);
    }

    #[test]
    fn the_bar_takes_the_smallest_icon_that_is_still_big_enough() {
        let mut data = vec![8, 8];
        data.extend(std::iter::repeat_n(0xff0000ffu32, 64));
        data.extend([32, 32]);
        data.extend(std::iter::repeat_n(0xff00ff00u32, 1024));
        data.extend([64, 64]);
        data.extend(std::iter::repeat_n(0xffff0000u32, 4096));
        let icon = best_icon(&data, 16).unwrap();
        assert_eq!((icon.width, icon.height), (16, 16));
        assert_eq!(
            &icon.rgba[..4],
            &[0, 255, 0, 255],
            "the 32px icon, not 8 or 64"
        );
        let only_small = best_icon(&data[..66], 16).unwrap();
        assert_eq!(&only_small.rgba[..4], &[0, 0, 255, 255]);
        assert!(
            best_icon(&[16, 16, 1, 2], 16).is_none(),
            "a truncated icon is no icon"
        );
    }

    #[test]
    fn menu_rows_leave_room_for_the_separator() {
        assert_eq!(menu_row_y(0), PAD);
        assert_eq!(menu_row_y(2), PAD + 2 * ROW);
        assert_eq!(menu_row_y(3), PAD + 3 * ROW + 11);
        assert_eq!(MENU_KEYS.len(), 4, "one letter per row");
    }
}
