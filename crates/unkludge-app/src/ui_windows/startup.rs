//! Starting up: the first launch, the other copy, making the window, and its message loop.

use std::cell::RefCell;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
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
    CLASS, DEMO_CLASS, SHOT_GIVE_UP_MS, TIMER_EXIT, TIMER_SHOT_GIVE_UP, WM_SHOWAPP, WM_WORKER,
    trim_memory,
};
use super::{App, tray};
use crate::args::Args;
use crate::i18n::t;
use crate::session::{Session, SystemAutostart};
use crate::settings::{self, Settings};
use crate::sys;

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
        if msg == WM_DESTROY {
            PostQuitMessage(0);
        }
        let result = match cell.try_borrow_mut() {
            Ok(mut app) => app.handle(msg, wp, lp),
            Err(_) => match when_busy(msg) {
                WhenBusy::PostAgain => {
                    let _ = PostMessageW(Some(hwnd), msg, wp, lp);
                    Some(0)
                }
                WhenBusy::Default => None,
            },
        };
        App::deferred_action(cell, hwnd);
        App::open_what_waits(cell);
        let quitting = cell.try_borrow().is_ok_and(|app| app.quit && !app.modal);
        if quitting {
            let worker = {
                let mut app = cell.borrow_mut();
                // Destroying the window sends more messages through here, and it only quits once.
                app.quit = false;
                app.save_settings();
                app.remove_tray();
                app.worker.take()
            };
            if let Some(worker) = worker {
                worker.shutdown();
            }
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        match result {
            Some(value) => LRESULT(value),
            None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// What to do with a message that comes while the app is busy with another.
#[derive(Debug, PartialEq, Eq)]
enum WhenBusy {
    /// The system's handling is all it needs.
    Default,
    /// It has to reach the app, and would be lost otherwise. The worker's wake-up is one: no other
    /// comes until it's been answered, so losing it would leave the window deaf.
    PostAgain,
}

fn when_busy(msg: u32) -> WhenBusy {
    match msg {
        WM_WORKER | WM_CLOSE => WhenBusy::PostAgain,
        _ => WhenBusy::Default,
    }
}

/// Asks the copy that's running to show its window, and lets it come to the front, which the system
/// only allows if the program in front says so.
fn show_other_copy() {
    // SAFETY: a null name means any window of the class. The number is written by the call, and the
    // rest takes plain numbers and can only fail.
    unsafe {
        if let Ok(hwnd) = FindWindowW(CLASS, PCWSTR::null()) {
            let mut process = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut process));
            let _ = AllowSetForegroundWindow(process);
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
    let session = Session::new(args.demo);
    if session.claims_single_instance() && !sys::claim_single_instance() {
        if Session::shows_other_copy(args.tray) {
            show_other_copy();
        }
        return;
    }
    let installed =
        std::env::current_exe().is_ok_and(|exe| settings::starts_with_system_on_first_launch(&exe));
    let data_dir = sys::data_dir();
    let settings = session.load_settings(data_dir.as_deref(), &SystemAutostart, installed);
    if let Err(error) = run_window(settings, session, args.tray) {
        panic!("the window can't be made: {error}");
    }
}

fn run_window(settings: Settings, session: Session, start_hidden: bool) -> Result<()> {
    let class = if session.is_demo() { DEMO_CLASS } else { CLASS };
    let title = if session.is_demo() {
        HSTRING::from(format!("Unkludge - {}", t().demo))
    } else {
        HSTRING::from("Unkludge")
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
            hIcon: tray::make_icon(tray::large_icon_size()),
            hIconSm: tray::make_icon(tray::small_icon_size()),
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

        let forced_dpi = session.demo_args().and_then(|args| args.dpi);
        let scripted = session.is_scripted();
        let exit_after = session.demo_args().and_then(|args| args.exit_after);

        // Before anything can move the window, as moving it to another monitor sends a message
        // about its DPI, and the app should get that.
        let app = App::new(hwnd, settings, session)?;
        let cell = Box::new(RefCell::new(app));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&raw const *cell) as isize);

        // Its size depends on the screen it's on, so that comes first.
        move_to_monitor_of_cursor(hwnd);
        {
            let mut app = cell.borrow_mut();
            app.monitor_dpi = forced_dpi.unwrap_or_else(|| GetDpiForWindow(hwnd)) as f32;
            app.fit_window();
            app.center_window();
            if !scripted {
                app.add_tray();
            }
        }
        if scripted {
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
        // After showing, so that the first write to a keyboard, which is the clock, doesn't come from
        // a program that hasn't appeared yet.
        cell.borrow_mut().start_worker();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // If something else ended the window, what quitting does hasn't been done.
        {
            let mut app = cell.borrow_mut();
            app.remove_tray();
            if let Some(worker) = app.worker.take() {
                worker.shutdown();
            }
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(cell);
    }
    Ok(())
}

/// Puts the window on the monitor the mouse is on, which is where it was started from.
///
/// # Safety
/// `hwnd` must be a window of this thread, and the app mustn't be borrowed.
unsafe fn move_to_monitor_of_cursor(hwnd: HWND) {
    // SAFETY: as the caller promises, and the structures are written by the calls.
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let _ = SetWindowPos(
                hwnd,
                None,
                info.rcWork.left,
                info.rcWork.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workers_wake_up_is_never_lost() {
        assert_eq!(when_busy(WM_WORKER), WhenBusy::PostAgain);
    }

    #[test]
    fn closing_the_window_is_never_lost() {
        assert_eq!(when_busy(WM_CLOSE), WhenBusy::PostAgain);
    }

    #[test]
    fn other_messages_get_the_systems_handling() {
        for message in [WM_PAINT, WM_SIZE, WM_MOUSEMOVE, WM_DESTROY] {
            assert_eq!(when_busy(message), WhenBusy::Default);
        }
    }
}
