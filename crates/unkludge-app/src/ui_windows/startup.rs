//! Starting up: the first launch, the other copy, making the window, and its message loop.

use std::cell::RefCell;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Shell::DragAcceptFiles;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result};

use super::window::{
    CLASS, DEMO_CLASS, DEMO_SETTINGS, SHOT_GIVE_UP_MS, TIMER_EXIT, TIMER_SHOT_GIVE_UP, WM_SHOWAPP,
    trim_memory,
};
use super::{App, tray};
use crate::args::Args;
use crate::i18n::t;
use crate::settings::{self, Settings};
use crate::{log, sys};

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: the pointer is the one `run` stored, whose box outlives the message loop, and is taken
    // back before that box goes. Messages come only on this thread. A message arriving while the
    // app is already borrowed, from a call made while handling another, gets the default handling.
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let cell = &*ptr;
        let result = match cell.try_borrow_mut() {
            Ok(mut app) => app.handle(msg, wp, lp),
            Err(_) => None,
        };
        App::deferred_action(cell, hwnd);
        let quitting = cell.try_borrow().is_ok_and(|app| app.quit && !app.modal);
        if quitting {
            let worker = {
                let mut app = cell.borrow_mut();
                app.save_settings();
                app.remove_tray();
                app.worker.take()
            };
            if let Some(worker) = worker {
                worker.shutdown();
            }
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        match result {
            Some(value) => LRESULT(value),
            None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// The settings of the real app, and what its first launch does.
fn real_settings() -> Settings {
    let settings = Settings::load();
    if settings.existed {
        // If the entry points at a copy that's gone, points it at this one.
        sys::refresh_autostart(false);
        return settings;
    }
    let installed =
        std::env::current_exe().is_ok_and(|exe| settings::starts_with_system_on_first_launch(&exe));
    if installed {
        sys::set_autostart(true);
    }
    if let Err(error) = settings.save() {
        log::error(&format!("the settings can't be saved: {error}"));
    }
    settings
}

/// A demo's settings are its own file, in the data folder if there's one.
fn demo_settings() -> Settings {
    match settings::path(DEMO_SETTINGS) {
        Some(path) => Settings::load_from(&path),
        None => Settings::default(),
    }
}

/// Asks the copy that's running to show its window.
fn show_other_copy() {
    // SAFETY: a null name means any window of the class, and posting to it can only fail.
    unsafe {
        if let Ok(hwnd) = FindWindowW(CLASS, PCWSTR::null()) {
            let _ = PostMessageW(Some(hwnd), WM_SHOWAPP, WPARAM(0), LPARAM(0));
        }
    }
}

/// Runs the app until it's told to quit.
pub fn run(args: Args) {
    // SAFETY: these set up the process, before any window exists.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let settings = match &args.demo {
        // A demo has no business with the real copy, autostart or settings.
        Some(_) => demo_settings(),
        None => {
            if !sys::claim_single_instance() {
                show_other_copy();
                return;
            }
            real_settings()
        }
    };
    if let Err(error) = run_window(settings, args) {
        panic!("the window can't be made: {error}");
    }
}

fn run_window(settings: Settings, args: Args) -> Result<()> {
    let demo = args.demo;
    let class = if demo.is_some() { DEMO_CLASS } else { CLASS };
    let title = match &demo {
        Some(_) => HSTRING::from(format!("Unkludge - {}", t().demo)),
        None => HSTRING::from("Unkludge"),
    };
    // SAFETY: the message loop below owns the window's whole life, with `cell` outliving it. Each
    // call's arguments are valid for it.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hIcon: tray::make_icon(32),
            hIconSm: tray::make_icon(16),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        tray::wm_taskbar_created();
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &title,
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            700,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            size_of::<i32>() as u32,
        );
        // The background's colour, as 0x00BBGGRR.
        let caption: u32 = 0x000F_0B0B;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            (&raw const caption).cast(),
            size_of::<u32>() as u32,
        );
        DragAcceptFiles(hwnd, true);

        let start_hidden = args.tray;
        let forced_dpi = demo.as_ref().and_then(|demo| demo.dpi);
        let scripted = demo.as_ref().is_some_and(|demo| demo.shot.is_some());
        let shot_give_up = scripted;
        let exit_after = demo.as_ref().and_then(|demo| demo.exit_after);

        let mut app = App::new(hwnd, settings, demo)?;
        app.dpi = forced_dpi.unwrap_or_else(|| GetDpiForWindow(hwnd)) as f32;
        app.fit_window();
        place_window(hwnd);
        if !scripted {
            app.add_tray();
        }
        let cell = Box::new(RefCell::new(app));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&raw const *cell) as isize);

        if shot_give_up {
            SetTimer(Some(hwnd), TIMER_SHOT_GIVE_UP, SHOT_GIVE_UP_MS, None);
        }
        if let Some(after) = exit_after {
            let ms = after.min(Duration::from_secs(86_400)).as_millis() as u32;
            SetTimer(Some(hwnd), TIMER_EXIT, ms.max(1), None);
        }
        // A scripted demo draws its picture without ever showing up on the desktop.
        if start_hidden || scripted {
            trim_memory();
        } else {
            cell.borrow_mut().show();
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(cell);
    }
    Ok(())
}

/// In the middle of the screen.
///
/// # Safety
/// `hwnd` must be a window of this thread.
unsafe fn place_window(hwnd: HWND) {
    // SAFETY: as the caller promises, and the rectangle is written by the call.
    unsafe {
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
    }
}
