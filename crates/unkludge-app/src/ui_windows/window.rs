//! The window as a whole: what it's made of, its messages, its drawing and its timers.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use unkludge_protocol::Device;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U;
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_RETAIN_CONTENTS, D2D1CreateFactory,
};
use windows::Win32::Graphics::DirectWrite::{DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::Accessibility::LresultFromObject;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Shell::HDROP;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCWSTR, Result, w};

use super::gfx::{BG, Draw, Fonts, Gfx, R};
use super::{Anim, App, Deferred, DemoSession, Shared, a11y, dialogs, tray};
use crate::args::DemoArgs;
use crate::demo::{DemoKeyboard, Script};
use crate::i18n::t;
use crate::layout::{Frame, WIDTH};
use crate::settings::{self, Settings};
use crate::state::{Slider, State};
use crate::worker::{self, Cmd, Options};
use crate::{log, sys};

pub(super) const CLASS: PCWSTR = w!("UnkludgeWindow");
/// A demo's window has a class of its own, so that the real app never takes it for its other copy.
pub(super) const DEMO_CLASS: PCWSTR = w!("UnkludgeDemoWindow");
/// A demo's settings are its own, next to the real ones.
pub(super) const DEMO_SETTINGS: &str = "settings-demo.txt";

/// The bindings have this one with the common controls, which the app doesn't otherwise need.
const WM_MOUSELEAVE: u32 = 0x02A3;
pub const WM_SHOWAPP: u32 = WM_APP + 2;
const WM_WORKER: u32 = WM_APP + 10;
pub const WM_INVOKE_ITEM: u32 = WM_APP + 11;

pub const TIMER_MESSAGE: usize = 3;
pub const TIMER_PREVIEW: usize = 2;
pub const TIMER_SHOT: usize = 4;

const TIMER_ANIM: usize = 1;
pub(super) const TIMER_EXIT: usize = 5;
pub(super) const TIMER_SHOT_GIVE_UP: usize = 6;

/// How long a scripted demo waits to reach its state before giving up.
pub(super) const SHOT_GIVE_UP_MS: u32 = 90_000;

impl App {
    pub(super) fn new(hwnd: HWND, settings: Settings, demo: Option<DemoArgs>) -> Result<App> {
        // SAFETY: plain factory calls, whose results own their references.
        let (d2d, dw) = unsafe {
            (
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
            )
        };
        let fonts = Fonts::new(&dw, t().spanish)?;

        let woken = Arc::new(AtomicBool::new(false));
        let notify = {
            let woken = Arc::clone(&woken);
            let hwnd = hwnd.0 as isize;
            Arc::new(move || {
                if !woken.swap(true, Ordering::SeqCst) {
                    // SAFETY: posting to a window that's gone is only a failed call.
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd as *mut _)),
                            WM_WORKER,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            })
        };
        let options = |rk_app_running| Options {
            auto_clock: settings.auto_clock,
            palettes: settings.palettes.clone(),
            rk_app_running,
        };
        // The only place the real keyboard comes into it, and only when it isn't a demo.
        let (worker, events, session) = match demo {
            Some(args) => {
                let model = unkludge_protocol::model::find(args.pid)
                    .ok_or_else(windows::core::Error::empty)?;
                let keyboard = DemoKeyboard::new(model, args.state);
                let looking = Arc::clone(&keyboard);
                let rk_app = Arc::clone(&keyboard);
                let (worker, events) = worker::spawn(
                    move || looking.open(),
                    options(Box::new(move || rk_app.rk_app_running())),
                    notify,
                );
                let session = DemoSession {
                    script: Script::new(args.state, keyboard, sys::data_dir()),
                    args,
                    shot_timer_set: false,
                };
                (worker, events, Some(session))
            }
            None => {
                let (worker, events) = worker::spawn(
                    || Device::open(None),
                    options(Box::new(sys::rk_app_running)),
                    notify,
                );
                (worker, events, None)
            }
        };

        let is_demo = session.is_some();
        let autostart = !is_demo && sys::autostart_enabled();
        let state = State::with_settings(&settings);
        let frame = Frame::new(0, state.preview_height());
        Ok(App {
            hwnd,
            d2d,
            dw,
            fonts,
            gfx: None,
            dpi: 96.0,
            a_auto_clock: Anim::new(settings.auto_clock),
            settings,
            state,
            frame,
            worker: Some(worker),
            events,
            woken,
            demo: session,
            shared: Rc::new(RefCell::new(Shared {
                items: Vec::new(),
                focus: None,
                scale: 1.0,
            })),
            accessible: None,
            show_focus: false,
            hover: None,
            tracking_mouse: false,
            drag: None,
            autostart,
            a_autostart: Anim::new(autostart),
            preview_since: Instant::now(),
            last_paint: Instant::now(),
            preview_timer: None,
            deferred: None,
            modal: false,
            tray_icon: tray::make_icon(16),
            quit: false,
        })
    }

    pub fn invalidate(&self) {
        // SAFETY: the handle is this window's own.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn start_anim(&self) {
        // SAFETY: as in `invalidate`.
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_ANIM, 16, None);
        }
    }

    pub fn send(&self, cmd: Option<Cmd>) {
        if let (Some(worker), Some(cmd)) = (&self.worker, cmd) {
            worker.send(cmd);
        }
    }

    pub fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: as in `invalidate`.
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// A scripted demo takes its picture and goes, and leaves nothing open behind it.
    pub(super) fn scripted(&self) -> bool {
        self.demo
            .as_ref()
            .is_some_and(|demo| demo.args.shot.is_some())
    }

    pub fn save_settings(&self) {
        let saved = if self.is_demo() {
            match settings::path(DEMO_SETTINGS) {
                Some(path) => self.settings.save_to(&path),
                None => Ok(()),
            }
        } else {
            self.settings.save()
        };
        if let Err(error) = saved {
            log::error(&format!("the settings can't be saved: {error}"));
        }
    }

    /// Starts with the system or not. A demo only pretends.
    pub fn set_autostart(&mut self, on: bool) {
        if self.is_demo() {
            self.autostart = on;
        } else {
            sys::set_autostart(on);
            self.autostart = sys::autostart_enabled();
        }
        self.a_autostart.set(self.autostart);
        self.start_anim();
        self.invalidate();
    }

    pub fn toggle_auto_clock(&mut self) {
        self.settings.auto_clock = !self.settings.auto_clock;
        self.send(Some(Cmd::SetAutoClock(self.settings.auto_clock)));
        self.a_auto_clock.set(self.settings.auto_clock);
        self.start_anim();
        self.save_settings();
        self.invalidate();
    }

    /// Fits the client area to the layout, without moving the window.
    pub(super) fn fit_window(&self) {
        // SAFETY: the handle is this window's own, and the rectangle is written by the call.
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd);
            let scale = self.dpi / 96.0;
            let style = WINDOW_STYLE(GetWindowLongW(self.hwnd, GWL_STYLE) as u32);
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: (WIDTH * scale).ceil() as i32,
                bottom: (self.frame.height * scale).ceil() as i32,
            };
            let _ = AdjustWindowRectExForDpi(&mut rect, style, false, WINDOW_EX_STYLE(0), dpi);
            let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                rect.right - rect.left,
                rect.bottom - rect.top,
                flags,
            );
        }
    }

    /// The layout follows the state: the model's screen, and any banners.
    pub(super) fn refresh_frame(&mut self) {
        let frame = Frame::new(self.state.banners().len(), self.state.preview_height());
        if frame != self.frame {
            self.frame = frame;
            self.fit_window();
        }
    }

    // Drawing.

    fn ensure_gfx(&mut self) -> Result<()> {
        let mut rect = RECT::default();
        // SAFETY: the handle is this window's own, and the rectangle is written by the call.
        unsafe { GetClientRect(self.hwnd, &mut rect)? };
        let size = (
            (rect.right - rect.left).max(1) as u32,
            (rect.bottom - rect.top).max(1) as u32,
        );
        let pixels = D2D_SIZE_U {
            width: size.0,
            height: size.1,
        };
        if let Some(gfx) = &mut self.gfx {
            if gfx.size != size
                && let Some(window) = &gfx.window
            {
                // SAFETY: the render target is this window's own.
                unsafe { window.Resize(&pixels)? };
                gfx.size = size;
            }
            // SAFETY: as above.
            unsafe { gfx.rt.SetDpi(self.dpi, self.dpi) };
            return Ok(());
        }
        let gfx = Gfx::for_window(
            &self.d2d,
            D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: pixels,
                presentOptions: D2D1_PRESENT_OPTIONS_RETAIN_CONTENTS,
            },
        )?;
        // SAFETY: the render target was just made.
        unsafe { gfx.rt.SetDpi(self.dpi, self.dpi) };
        self.gfx = Some(gfx);
        Ok(())
    }

    pub(super) fn paint(&mut self) {
        if let Err(error) = self.ensure_gfx() {
            log::error(&format!("nothing can be drawn: {error}"));
            return;
        }
        self.update_preview();
        self.render();
    }

    /// Draws on another target for once, for a picture.
    pub fn paint_to(&mut self, gfx: Gfx) {
        // SAFETY: the render target was just made.
        unsafe { gfx.rt.SetDpi(self.dpi, self.dpi) };
        let window = self.gfx.replace(gfx);
        self.update_preview();
        self.render();
        self.gfx = window;
    }

    /// Draws everything on the current target.
    fn render(&mut self) {
        self.last_paint = Instant::now();
        {
            let mut shared = self.shared.borrow_mut();
            shared.items.clear();
            shared.scale = self.dpi / 96.0;
        }
        let Some(gfx) = self.gfx.as_ref() else {
            return;
        };
        let (rt, brush, glow, rainbow) = (
            gfx.rt.clone(),
            gfx.brush.clone(),
            gfx.glow.clone(),
            gfx.rainbow.clone(),
        );
        let preview = gfx.preview.as_ref().map(|p| p.bitmap.clone());
        // SAFETY: the render target is this window's own, and these calls pair up below.
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&BG));
            rt.FillRectangle(&R::new(0.0, 0.0, WIDTH, 260.0).d2d(), &glow);
        }
        let d = Draw {
            rt: &rt,
            brush: &brush,
            dw: &self.dw,
        };
        self.paint_header(&d);
        self.paint_banners(&d);
        // Every model this version connects to has a screen.
        if let Some(screen) = self.state.model().and_then(|model| model.screen) {
            self.paint_screen(&d, screen, preview.as_ref());
            self.paint_lights(&d, &rainbow);
            self.paint_clock(&d);
        } else {
            self.paint_empty(&d);
        }
        self.paint_footer(&d);
        self.paint_focus(&d);
        // SAFETY: as above.
        let ended = unsafe { rt.EndDraw(None, None) };
        if let Err(error) = ended {
            if cfg!(debug_assertions) {
                eprintln!("the frame didn't end well: {error}");
            }
            // The system took the target away, and the next paint makes another.
            self.gfx = None;
            self.invalidate();
        }
    }

    // The window itself.

    pub fn show(&mut self) {
        // What was read before may be old by now, if the window was hidden.
        if !self.is_visible() {
            self.send(Some(Cmd::Refresh));
        }
        // SAFETY: the handle is this window's own.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = ShowWindow(self.hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.restart_preview();
        self.invalidate();
    }

    /// Closing the window hides it, and while hidden nothing gets drawn, so what's for drawing and
    /// the memory it took can go.
    fn hide(&mut self) {
        // SAFETY: the handle is this window's own.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        if !self.settings.tray_note_shown {
            self.settings.tray_note_shown = true;
            self.show_tray_note();
        }
        self.save_settings();
        self.restart_preview();
        self.gfx = None;
        trim_memory();
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_ANIM => {
                let moving = self.a_autostart.step() | self.a_auto_clock.step();
                if !moving {
                    // SAFETY: the handle is this window's own.
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
                    }
                }
                self.invalidate();
            }
            TIMER_PREVIEW => self.paint_preview_frame(),
            TIMER_MESSAGE => {
                // SAFETY: as above.
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_MESSAGE);
                }
                if self.state.message.as_ref().is_some_and(|m| m.fades()) {
                    self.state.message = None;
                    self.invalidate();
                }
            }
            TIMER_SHOT => {
                // SAFETY: as above.
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_SHOT);
                }
                self.take_shot();
            }
            TIMER_SHOT_GIVE_UP => {
                log::error("the demo didn't reach its state, so there's no picture");
                self.quit = true;
            }
            TIMER_EXIT => self.quit = true,
            _ => {}
        }
    }

    pub fn dip(&self, lp: LPARAM) -> (f32, f32) {
        let x = (lp.0 & 0xFFFF) as i16 as f32;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32;
        let k = 96.0 / self.dpi;
        (x * k, y * k)
    }

    pub(super) fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<isize> {
        if msg == tray::wm_taskbar_created() {
            self.add_tray();
            return Some(0);
        }
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: the handle is this window's own, and these two calls pair up.
                unsafe { BeginPaint(self.hwnd, &mut ps) };
                self.paint();
                // SAFETY: as above.
                unsafe {
                    let _ = EndPaint(self.hwnd, &ps);
                }
                Some(0)
            }
            WM_ERASEBKGND => Some(1),
            WM_WORKER => {
                self.drain_events();
                Some(0)
            }
            WM_TIMER => {
                self.on_timer(wp.0);
                Some(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = self.dip(lp);
                self.mouse_down(x, y);
                Some(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = self.dip(lp);
                self.mouse_move(x, y);
                Some(0)
            }
            WM_MOUSELEAVE => {
                self.mouse_left();
                Some(0)
            }
            WM_LBUTTONUP => {
                self.end_drag();
                Some(0)
            }
            // Another window took the mouse in the middle of a drag.
            WM_CAPTURECHANGED => {
                if self.drag.is_some() {
                    self.drag = None;
                    self.invalidate();
                }
                Some(0)
            }
            WM_KEYDOWN => self.key_down(wp.0 as u16).then_some(0),
            WM_SETCURSOR => self.set_cursor(lp).then_some(1),
            WM_SIZE => {
                self.invalidate();
                None
            }
            WM_DPICHANGED => {
                // A demo that asked for a DPI keeps it.
                if self
                    .demo
                    .as_ref()
                    .is_none_or(|demo| demo.args.dpi.is_none())
                {
                    self.dpi = f32::from((wp.0 & 0xFFFF) as u16);
                    // SAFETY: the system passes the suggested rectangle in `lp`, valid for this
                    // message.
                    let rect = unsafe { &*(lp.0 as *const RECT) };
                    // SAFETY: the handle is this window's own.
                    unsafe {
                        let _ = SetWindowPos(
                            self.hwnd,
                            None,
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
                self.invalidate();
                Some(0)
            }
            WM_DROPFILES => {
                let drop = HDROP(wp.0 as *mut _);
                if let Some(path) = dialogs::dropped_file(drop) {
                    let cmd = self.state.choose_file(path);
                    self.send(cmd);
                    self.restart_preview();
                    self.invalidate();
                }
                Some(0)
            }
            WM_POWERBROADCAST => {
                if wp.0 as u32 == PBT_APMRESUMEAUTOMATIC {
                    self.send(Some(Cmd::Resumed));
                }
                None
            }
            WM_GETOBJECT => self.accessible_object(wp, lp),
            WM_INVOKE_ITEM => {
                let item = self.shared.borrow().items.get(wp.0).cloned();
                if let Some(item) = item.filter(|item| item.enabled) {
                    self.set_focus(Some(item.hit));
                    self.activate(item.hit);
                }
                Some(0)
            }
            WM_CLOSE => {
                self.hide();
                Some(0)
            }
            WM_SHOWAPP => {
                self.show();
                Some(0)
            }
            WM_ENDSESSION if wp.0 != 0 => {
                self.save_settings();
                Some(0)
            }
            tray::WM_TRAY => {
                match (lp.0 & 0xFFFF) as u32 {
                    WM_LBUTTONUP if !self.modal => self.show(),
                    WM_RBUTTONUP if !self.modal => self.deferred = Some(Deferred::TrayMenu),
                    _ => {}
                }
                Some(0)
            }
            _ => None,
        }
    }

    /// What a screen reader asks for.
    fn accessible_object(&mut self, wp: WPARAM, lp: LPARAM) -> Option<isize> {
        if lp.0 as i32 != OBJID_CLIENT.0 {
            return None;
        }
        let accessible = self
            .accessible
            .get_or_insert_with(|| a11y::create(self.hwnd, Rc::clone(&self.shared)));
        // SAFETY: the object is alive and implements what's asked for.
        let result = unsafe {
            LresultFromObject(
                &windows::Win32::UI::Accessibility::IAccessible::IID,
                wp,
                &*accessible,
            )
        };
        Some(result.0)
    }

    /// Runs what can't be done while handling a message.
    pub(super) fn deferred_action(cell: &RefCell<App>, hwnd: HWND) {
        // A message that comes while a dialog is open finds nothing here, and leaves the flag be.
        let deferred = match cell.try_borrow_mut() {
            // One dialog at a time. A screen reader could ask for another during it.
            Ok(mut app) => app.deferred.take().filter(|_| !app.modal),
            Err(_) => return,
        };
        let Some(deferred) = deferred else {
            return;
        };
        if let Ok(mut app) = cell.try_borrow_mut() {
            app.modal = true;
        }
        match deferred {
            Deferred::ChooseFile => {
                let path = dialogs::choose_image(hwnd);
                if let (Some(path), Ok(mut app)) = (path, cell.try_borrow_mut()) {
                    let cmd = app.state.choose_file(path);
                    app.send(cmd);
                    app.restart_preview();
                }
            }
            Deferred::CustomColor => {
                let (warn, initial) = {
                    let app = cell.borrow();
                    let colors = app.state.slot_colors(&app.settings);
                    (
                        app.state.custom_color_resets_others(&app.settings),
                        colors[0],
                    )
                };
                if !warn || dialogs::warn_first_custom_color(hwnd) {
                    let color = dialogs::choose_color(hwnd, initial);
                    if let (Some(color), Ok(app)) = (color, cell.try_borrow()) {
                        app.send(
                            app.state
                                .set_color(unkludge_protocol::lighting::ColorChoice::Custom(color)),
                        );
                    }
                }
            }
            Deferred::TrayMenu => {
                let autostart = cell.borrow().autostart;
                let choice = tray::menu(hwnd, autostart);
                if let Ok(mut app) = cell.try_borrow_mut() {
                    app.tray_choice(choice);
                }
            }
        }
        if let Ok(mut app) = cell.try_borrow_mut() {
            app.modal = false;
            app.invalidate();
        }
    }

    fn tray_choice(&mut self, choice: tray::MenuChoice) {
        match choice {
            tray::MenuChoice::None => {}
            tray::MenuChoice::Open => self.show(),
            tray::MenuChoice::SyncClock => self.send(Some(Cmd::SyncClock)),
            tray::MenuChoice::Autostart => {
                let on = !self.autostart;
                self.set_autostart(on);
            }
            tray::MenuChoice::Quit => self.quit = true,
        }
    }

    /// For the slider code, which moves a thumb to where the mouse is.
    pub fn begin_drag(&mut self, drag: Slider, x: f32) {
        self.drag = Some(drag);
        self.drag_to(x);
        // SAFETY: the handle is this window's own.
        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
        }
    }
}

pub(super) fn trim_memory() {
    // SAFETY: both values together mean to give back what the system wants.
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}
