//! The desktop the agent owns: the wallpaper, the dock, and the window
//! manager. Nothing else in the container has an opinion about windows.
//!
//! Chromium needs no decorations and the agent already speaks EWMH, so the
//! agent is the window manager. A normal window opens maximized into the
//! work area below the dock; a dialog opens centered at its own size. Focus
//! follows a click, so a person driving the screen reaches the window they
//! see. `_NET_CLIENT_LIST`, `_NET_ACTIVE_WINDOW` and `_NET_WM_STATE` are kept
//! current because the `windows` tool reads them.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc;

use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Allow, Atom, AtomEnum, ButtonIndex, ButtonPressEvent, CONFIGURE_NOTIFY_EVENT, ChangeGCAux,
    ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent, ConfigWindow,
    ConfigureNotifyEvent, ConfigureRequestEvent, ConfigureWindowAux, ConnectionExt, CreateGCAux,
    CreateWindowAux, EventMask, Gcontext, GrabMode, ImageFormat, InputFocus, MapState, ModMask,
    Pixmap, PropMode, Rectangle, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

/// What the desktop asks of the agent side.
#[derive(Clone, Debug)]
pub enum Request {
    /// The dock's browser was clicked and no browser window exists.
    OpenBrowser,
    /// The last browser window is gone. Chromium's process and its DevTools
    /// pages outlive a destroyed window, so the agent has to be told.
    BrowserClosed,
    OpenTerminal,
    OpenJob(String),
}

const MARK: &[u8] = include_bytes!("../assets/wallpaper-mark.png");
const BROWSER_ICON: &str = "/usr/share/icons/hicolor/32x32/apps/chromium.png";
const BROWSER_CLASS: &str = "chromium";
const BACKGROUND: u32 = 0x000000;
const DOCK_FILL: u32 = 0x161616;
const DOCK_EDGE: u32 = 0x2c2c2c;
const DOCK_PAD: u16 = 8;
const ICON: u16 = 32;
const DOCK_HEIGHT: u16 = ICON + 2 * DOCK_PAD;
/// PutImage requests stay well under the smallest maximum request length.
const PUT_IMAGE_CHUNK: usize = 60_000;

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

struct Image {
    width: u16,
    height: u16,
    rgba: Vec<u8>,
}

struct DockItem {
    icon: Option<Image>,
    request: Request,
}

struct JobMenu {
    window: Window,
    jobs: Vec<crate::jobs::Summary>,
    first: usize,
    visible: usize,
    width: u16,
}

struct Client {
    maximized: bool,
    /// Where the window goes when it is not maximized.
    saved: (i16, i16, u16, u16),
    /// `WM_CLASS`, lower-cased, read once: a destroyed window has none to read.
    class: String,
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
}

impl Atoms {
    fn intern(connection: &RustConnection) -> Result<Self, String> {
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
    dock: Window,
    dock_x: i16,
    dock_width: u16,
    items: Vec<DockItem>,
    /// Managed windows in mapping order, which is what `_NET_CLIENT_LIST` lists.
    order: Vec<Window>,
    clients: HashMap<Window, Client>,
    active: Option<Window>,
    requests: UnboundedSender<Request>,
    tray: Vec<Window>,
    tray_selection: Atom,
    tray_opcode: Atom,
    xembed: Atom,
    jobs_running: Atom,
    jobs_summary: Atom,
    job_menu: Option<JobMenu>,
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
        let atoms = Atoms::intern(&connection)?;
        let gc = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        connection
            .create_gc(gc, screen.root, &CreateGCAux::new().foreground(BACKGROUND))
            .map_err(|error| error.to_string())?;

        let items = vec![
            DockItem {
                icon: std::fs::read(BROWSER_ICON)
                    .ok()
                    .and_then(|bytes| decode_png(&bytes).ok()),
                request: Request::OpenBrowser,
            },
            DockItem {
                icon: None,
                request: Request::OpenTerminal,
            },
        ];
        let dock_width = screen.width_in_pixels;
        let dock_x = 0;
        let intern = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .map_err(|e| e.to_string())?
                .reply()
                .map(|r| r.atom)
                .map_err(|e| e.to_string())
        };
        let tray_selection = intern(format!("_NET_SYSTEM_TRAY_S{screen_number}").as_bytes())?;
        let tray_opcode = intern(b"_NET_SYSTEM_TRAY_OPCODE")?;
        let xembed = intern(b"_XEMBED")?;
        let jobs_running = intern(b"_TOAD_JOBS_RUNNING")?;
        let jobs_summary = intern(b"_TOAD_JOB_SUMMARY")?;
        let font = connection.generate_id().map_err(|e| e.to_string())?;
        if connection
            .open_font(
                font,
                b"-misc-fixed-medium-r-normal--18-120-100-100-c-90-iso8859-1",
            )
            .map_err(|e| e.to_string())?
            .check()
            .is_err()
        {
            connection
                .open_font(font, b"fixed")
                .map_err(|e| e.to_string())?
                .check()
                .map_err(|e| e.to_string())?;
        }
        connection
            .change_gc(gc, &ChangeGCAux::new().font(font))
            .map_err(|e| e.to_string())?;
        let dock = connection
            .generate_id()
            .map_err(|error| error.to_string())?;

        let mut desktop = Self {
            connection,
            root: screen.root,
            width: screen.width_in_pixels,
            height: screen.height_in_pixels,
            depth: screen.root_depth,
            atoms,
            gc,
            dock,
            dock_x,
            dock_width,
            items,
            order: Vec::new(),
            clients: HashMap::new(),
            active: None,
            requests,
            tray: Vec::new(),
            tray_selection,
            tray_opcode,
            xembed,
            jobs_running,
            jobs_summary,
            job_menu: None,
        };
        desktop.announce()?;
        desktop.paint_wallpaper()?;
        desktop.create_dock(screen.root_visual)?;
        desktop.own_tray()?;
        desktop.adopt_existing()?;
        desktop
            .connection
            .flush()
            .map_err(|error| error.to_string())?;
        Ok(desktop)
    }

    /// The top bar always retains its own work area.
    fn work_height(&self) -> u16 {
        self.height.saturating_sub(DOCK_HEIGHT)
    }

    fn dock_y(&self) -> i16 {
        0
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
                u32::from(DOCK_HEIGHT),
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

    /// Black, with the mark centred in the work area below the dock.
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
        let y = (DOCK_HEIGHT + self.work_height().saturating_sub(mark.height) / 2) as i16;
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

    fn create_dock(&self, visual: u32) -> Result<(), String> {
        self.connection
            .create_window(
                self.depth,
                self.dock,
                self.root,
                self.dock_x,
                self.dock_y(),
                self.dock_width,
                DOCK_HEIGHT,
                0,
                WindowClass::INPUT_OUTPUT,
                visual,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .background_pixel(DOCK_FILL)
                    .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS),
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .map_window(self.dock)
            .map_err(|error| error.to_string())?;
        self.draw_dock()
    }

    fn draw_dock(&self) -> Result<(), String> {
        self.fill(self.dock, DOCK_FILL, 0, 0, self.dock_width, DOCK_HEIGHT)?;
        self.connection
            .change_gc(self.gc, &ChangeGCAux::new().foreground(DOCK_EDGE))
            .map_err(|error| error.to_string())?;
        self.connection
            .poly_rectangle(
                self.dock,
                self.gc,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: self.dock_width - 1,
                    height: DOCK_HEIGHT - 1,
                }],
            )
            .map_err(|error| error.to_string())?;
        let mark = decode_png(MARK)?;
        let mut small = Image {
            width: ICON,
            height: (u32::from(ICON) * u32::from(mark.height) / u32::from(mark.width)).max(1)
                as u16,
            rgba: Vec::with_capacity(usize::from(ICON * ICON) * 4),
        };
        for y in 0..small.height {
            for x in 0..ICON {
                let offset = (usize::from(y) * usize::from(mark.height)
                    / usize::from(small.height)
                    * usize::from(mark.width)
                    + usize::from(x) * usize::from(mark.width) / usize::from(ICON))
                    * 4;
                small.rgba.extend_from_slice(&mark.rgba[offset..offset + 4]);
            }
        }
        self.put_image(
            self.dock,
            &small,
            DOCK_PAD as i16,
            ((DOCK_HEIGHT - small.height) / 2) as i16,
            DOCK_FILL,
        )?;
        for (index, item) in self.items.iter().enumerate() {
            let x = (DOCK_PAD + (index as u16 + 1) * (ICON + DOCK_PAD)) as i16;
            if let Some(icon) = &item.icon {
                self.put_image(self.dock, icon, x, DOCK_PAD as i16, DOCK_FILL)?;
            } else {
                self.label(x, 30, ">_", 0xe0e0e0)?;
            }
        }
        let counts = self
            .connection
            .get_property(
                false,
                self.root,
                self.jobs_running,
                AtomEnum::CARDINAL,
                0,
                3,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .ok()
            .and_then(|r| r.value32().map(Iterator::collect::<Vec<_>>))
            .unwrap_or_default();
        let running = counts.first().copied().unwrap_or(0);
        let completed = counts.get(1).copied().unwrap_or(0);
        let failed = counts.get(2).copied().unwrap_or(0);
        self.label(
            132,
            20,
            &format!("{running} active"),
            if running > 0 { 0x98d8a0 } else { 0x999999 },
        )?;
        self.label(
            132,
            39,
            &format!("{completed} ok {failed} err"),
            if failed > 0 { 0xf0b37e } else { 0x999999 },
        )?;
        let available = self.width.saturating_sub(240 + self.tray.len() as u16 * 40);
        for (index, window) in self
            .order
            .iter()
            .take(usize::from(available / 160))
            .enumerate()
        {
            let x = 240 + index as i16 * 160;
            if self.active == Some(*window) {
                self.fill(self.dock, 0x343434, x, 4, 154, DOCK_HEIGHT - 8)?;
            }
            let title = self
                .connection
                .get_property(
                    false,
                    *window,
                    self.atoms.net_wm_name,
                    self.atoms.utf8_string,
                    0,
                    128,
                )
                .map_err(|e| e.to_string())?
                .reply()
                .map(|r| String::from_utf8_lossy(&r.value).into_owned())
                .unwrap_or_default();
            let title = if title.is_empty() {
                self.clients
                    .get(window)
                    .map(|c| c.class.replace('\0', " "))
                    .unwrap_or_default()
            } else {
                title
            };
            self.label(x + 8, 30, &title, 0xd6d6d6)?;
        }
        self.layout_tray()?;
        Ok(())
    }

    fn label(&self, x: i16, y: i16, text: &str, color: u32) -> Result<(), String> {
        self.text_at(self.dock, x, y, text, 16, color)
    }

    fn text_at(
        &self,
        window: Window,
        x: i16,
        y: i16,
        text: &str,
        limit: usize,
        color: u32,
    ) -> Result<(), String> {
        let text: Vec<_> = text
            .chars()
            .take(limit)
            .map(|c| {
                if c.is_ascii_graphic() || c == ' ' {
                    c as u8
                } else {
                    b'?'
                }
            })
            .collect();
        self.connection
            .change_gc(
                self.gc,
                &ChangeGCAux::new().foreground(color).background(DOCK_FILL),
            )
            .map_err(|e| e.to_string())?;
        self.connection
            .image_text8(window, self.gc, x, y, &text)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn open_job_menu(&mut self) -> Result<(), String> {
        if self.job_menu.is_some() {
            return self.close_job_menu();
        }
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                self.jobs_summary,
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
        let width = self.width.saturating_sub(120).min(640);
        let window = self.connection.generate_id().map_err(|e| e.to_string())?;
        self.connection
            .create_window(
                self.depth,
                window,
                self.root,
                120,
                DOCK_HEIGHT as i16,
                width,
                ((visible + 1) * 30) as u16,
                1,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .background_pixel(DOCK_FILL)
                    .border_pixel(DOCK_EDGE)
                    .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS),
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
        self.job_menu = Some(JobMenu {
            window,
            jobs,
            first: 0,
            visible,
            width,
        });
        self.draw_job_menu()
    }

    fn close_job_menu(&mut self) -> Result<(), String> {
        if let Some(menu) = self.job_menu.take() {
            self.connection
                .destroy_window(menu.window)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn draw_job_menu(&self) -> Result<(), String> {
        let Some(menu) = &self.job_menu else {
            return Ok(());
        };
        self.fill(
            menu.window,
            DOCK_FILL,
            0,
            0,
            menu.width,
            ((menu.visible + 1) * 30) as u16,
        )?;
        let limit = usize::from(menu.width.saturating_sub(16) / 9);
        self.text_at(
            menu.window,
            8,
            21,
            "All retained jobs (scroll for more)",
            limit,
            0xd6d6d6,
        )?;
        for (row, job) in menu
            .jobs
            .iter()
            .skip(menu.first)
            .take(menu.visible)
            .enumerate()
        {
            let color = if job.state == "running" {
                0x98d8a0
            } else if job.exit_code == Some(0) {
                0xd6d6d6
            } else {
                0xf0b37e
            };
            self.text_at(
                menu.window,
                8,
                (row as i16 + 1) * 30 + 21,
                &format!("{}  {}", job.state, job.label),
                limit,
                color,
            )?;
        }
        Ok(())
    }

    fn own_tray(&self) -> Result<(), String> {
        let orientation = self
            .connection
            .intern_atom(false, b"_NET_SYSTEM_TRAY_ORIENTATION")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        self.property32(self.dock, orientation, AtomEnum::CARDINAL, &[0])?;
        self.connection
            .set_selection_owner(self.dock, self.tray_selection, CURRENT_TIME)
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
                    ClientMessageData::from([CURRENT_TIME, self.tray_selection, self.dock, 0, 0]),
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
            .reparent_window(window, self.dock, 0, 8)
            .map_err(|e| e.to_string())?;
        self.connection
            .send_event(
                false,
                window,
                EventMask::NO_EVENT,
                ClientMessageEvent::new(
                    32,
                    window,
                    self.xembed,
                    ClientMessageData::from([CURRENT_TIME, 0, 0, self.dock, 0]),
                ),
            )
            .map_err(|e| e.to_string())?;
        self.tray.push(window);
        self.layout_tray()?;
        self.connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        self.draw_dock()
    }

    fn layout_tray(&self) -> Result<(), String> {
        for (index, window) in self.tray.iter().enumerate() {
            let x = i32::from(self.width) - 40 * (index as i32 + 1);
            self.connection
                .configure_window(
                    *window,
                    &ConfigureWindowAux::new().x(x).y(8).width(32).height(32),
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

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
            .change_gc(self.gc, &ChangeGCAux::new().foreground(color))
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
                self.draw_dock()
            }
            Event::DestroyNotify(event) => self.forget(event.window),
            Event::PropertyNotify(event)
                if event.window == self.root && event.atom == self.jobs_running =>
            {
                self.draw_dock()
            }
            Event::PropertyNotify(event)
                if self.clients.contains_key(&event.window)
                    && event.atom == self.atoms.net_wm_name =>
            {
                self.draw_dock()
            }
            Event::ClientMessage(event) => {
                let data = event.data.as_data32();
                if event.type_ == self.tray_opcode && data[1] == 0 {
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
            Event::Expose(event)
                if self
                    .job_menu
                    .as_ref()
                    .is_some_and(|menu| menu.window == event.window)
                    && event.count == 0 =>
            {
                self.draw_job_menu()
            }
            Event::Expose(event) if event.window == self.dock && event.count == 0 => {
                self.draw_dock()
            }
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
        let client = match self.kind(window)? {
            Kind::Furniture => {
                self.connection
                    .map_window(window)
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
            Kind::Normal if class.contains("toadterminal") => Client {
                maximized: false,
                saved: (
                    (self.width * 3 / 5) as i16,
                    DOCK_HEIGHT as i16,
                    self.width - self.width * 3 / 5,
                    self.work_height(),
                ),
                class,
            },
            Kind::Normal => Client {
                maximized: true,
                saved: (geometry.x, geometry.y, geometry.width, geometry.height),
                class,
            },
            Kind::Dialog => {
                let width = geometry.width.min(self.width);
                let height = geometry.height.min(self.work_height());
                Client {
                    maximized: false,
                    saved: (
                        ((self.width - width) / 2) as i16,
                        (DOCK_HEIGHT + (self.work_height() - height) / 2) as i16,
                        width,
                        height,
                    ),
                    class,
                }
            }
        };
        if client.class.contains("toadterminal") {
            let width = self.width * 3 / 5;
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
                    other.saved = (0, DOCK_HEIGHT as i16, width, height);
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
            (0, DOCK_HEIGHT as i16, self.width, self.work_height())
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
                self.dock,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .map_err(|error| error.to_string())?;
        self.property32(
            self.root,
            self.atoms.net_active_window,
            AtomEnum::WINDOW,
            &[window],
        )?;
        self.active = Some(window);
        self.draw_dock()
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
        self.draw_dock()?;
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
        if let Some(menu) = &mut self.job_menu {
            if event.event == menu.window {
                if event.detail == 4 || event.detail == 5 {
                    menu.first = if event.detail == 4 {
                        menu.first.saturating_sub(1)
                    } else {
                        (menu.first + 1).min(menu.jobs.len().saturating_sub(menu.visible))
                    };
                    return self.draw_job_menu();
                }
                let row = usize::try_from(event.event_y).unwrap_or(0) / 30;
                let request = if row == 0 {
                    Request::OpenTerminal
                } else {
                    let Some(job) = menu.jobs.get(menu.first + row - 1) else {
                        return Ok(());
                    };
                    Request::OpenJob(job.id.clone())
                };
                self.close_job_menu()?;
                return self.dock_action(request);
            }
            self.close_job_menu()?;
        }
        if event.event == self.dock {
            if event.event_x >= 240 {
                let index = (event.event_x as usize - 240) / 160;
                if let Some(window) = self.order.get(index).copied() {
                    if event.detail == 3 {
                        self.close(window)?;
                    } else {
                        self.focus(window)?;
                    }
                }
                return Ok(());
            }
            if event.event_x >= 120 {
                return self.open_job_menu();
            }
            let slot = usize::from(u16::try_from(event.event_x).unwrap_or(0) / (ICON + DOCK_PAD));
            if slot == 0 {
                return self.dock_action(Request::OpenBrowser);
            }
            if let Some(item) = self.items.get(slot - 1) {
                self.dock_action(item.request.clone())?;
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
            Request::OpenTerminal | Request::OpenJob(_) => {
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
}
