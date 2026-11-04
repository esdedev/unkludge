//! What connects the window to the app: what the markup asks for, what winit tells, and a few things
//! that are asked of the window.

use std::time::{Duration, Instant};

use unkludge_protocol::lighting::ColorChoice;
use slint::platform::{Key, WindowEvent};
use slint::{
    CloseRequestResponse, ComponentHandle, Global, Image, Rgba8Pixel, SharedPixelBuffer,
    SharedString, Timer,
};

use super::{APP, App, with_app};
use crate::icon;
use crate::state::Slider;
use crate::ui_linux::{Actions, AppWindow, view};
use crate::worker::Cmd;

/// Size of the icon that goes in the window and its header.
const ICON_SIZE: usize = 128;

/// The app's icon, as the window takes it.
pub(super) fn app_icon() -> Image {
    let bgra = icon::pixels(ICON_SIZE);
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(ICON_SIZE as u32, ICON_SIZE as u32);
    let (source, _) = bgra.as_chunks::<4>();
    for (target, &[b, g, r, a]) in pixels.make_mut_slice().iter_mut().zip(source) {
        *target = Rgba8Pixel { r, g, b, a };
    }
    Image::from_rgba8(pixels)
}

/// Brings the window to the front, as far as the window manager lets it.
pub(super) fn raise(window: &AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    window
        .window()
        .with_winit_window(|winit_window| winit_window.focus_window());
}

/// What Slint doesn't deal with itself: files dropped, the scale changing or the window moving to
/// another screen, which have to go through the sums again, and signs that a window it thinks is
/// minimized is back.
pub(in crate::ui_linux) fn hook_window_events(window: &AppWindow) {
    use slint::winit_030::winit::event::WindowEvent as Winit;
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    window
        .window()
        .on_winit_window_event(|window, event| match event {
            Winit::DroppedFile(path) => {
                let (path, now) = (path.clone(), Instant::now());
                with_app(move |app| app.file_dropped(path, now));
                EventResult::PreventDefault
            }
            Winit::ScaleFactorChanged { .. } => {
                Timer::single_shot(Duration::ZERO, || with_app(App::fit_window));
                EventResult::Propagate
            }
            Winit::Moved(_) => {
                with_app(App::refit_soon);
                EventResult::Propagate
            }
            Winit::Resized(_) => {
                with_app(App::window_resized);
                EventResult::Propagate
            }
            Winit::Focused(true)
            | Winit::CursorEntered { .. }
            | Winit::CursorMoved { .. }
            | Winit::MouseInput { .. }
            | Winit::KeyboardInput { .. }
                if window.is_minimized() =>
            {
                // Not from in here, where Slint is in the middle of dealing with the event.
                Timer::single_shot(Duration::ZERO, || with_app(App::window_seen));
                EventResult::Propagate
            }
            _ => EventResult::Propagate,
        });
}

/// A key going down and up, as if it had been pressed.
pub(in crate::ui_linux) fn press(window: &slint::Window, key: Key) {
    let text = SharedString::from(key);
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

/// Connects what the markup does to what follows from it.
pub(in crate::ui_linux) fn wire(window: &AppWindow) {
    let actions = Actions::get(window);
    actions.on_report(|| with_app(|app| app.send(Some(Cmd::Report))));
    actions.on_choose_file(|by_keyboard| with_app(move |app| app.ask_for_file(by_keyboard)));
    actions.on_pick_fit(|index| {
        with_app(move |app| {
            if let Some(fit) = view::fit_at(index) {
                let cmd = app.state.set_fit(fit);
                app.send(cmd);
                app.refresh();
            }
        });
    });
    actions.on_slower(|| with_app(|app| step_delay(app, true)));
    actions.on_faster(|| with_app(|app| step_delay(app, false)));
    actions.on_upload(|by_keyboard| with_app(move |app| app.upload(by_keyboard)));
    actions.on_cancel(|| {
        with_app(|app| {
            app.send(app.state.cancel_upload());
            app.refresh();
        });
    });
    actions.on_sync_clock(|| with_app(|app| app.send(Some(Cmd::SyncClock))));
    actions.on_toggle_auto_clock(|| with_app(App::toggle_auto_clock));
    actions.on_pick_effect(|index| {
        with_app(move |app| {
            if let Some(mode) = view::effect_at(&app.state, index) {
                app.send(app.state.set_mode(mode));
            }
        });
    });
    actions.on_drag_brightness(|x, x0, x1| {
        with_app(move |app| app.drag_slider(Slider::Brightness, x, x0, x1));
    });
    actions.on_nudge_brightness(|by| with_app(move |app| app.nudge(Slider::Brightness, by)));
    actions.on_drag_speed(|x, x0, x1| {
        with_app(move |app| app.drag_slider(Slider::Speed, x, x0, x1));
    });
    actions.on_nudge_speed(|by| with_app(move |app| app.nudge(Slider::Speed, by)));
    actions.on_pick_slot(|slot| {
        with_app(move |app| {
            app.send(app.state.set_color(ColorChoice::Slot(slot as u8)));
        });
    });
    actions.on_pick_rainbow(|| with_app(|app| app.send(app.state.set_color(ColorChoice::Rainbow))));
    actions.on_custom_color(|by_keyboard| {
        with_app(move |app| app.open_color_dialog(by_keyboard));
    });
    actions.on_pick_sleep(|index| {
        with_app(move |app| {
            if let Some(sleep) = view::sleep_at(index) {
                app.send(app.state.set_sleep(sleep));
            }
        });
    });
    actions.on_toggle_autostart(|| with_app(App::toggle_autostart));
    actions.on_grant_access(|| with_app(App::grant_access));
    actions.on_color_channel(|channel, value| {
        with_app(move |app| app.color_channel(channel, value));
    });
    actions.on_color_code_edited(|code| with_app(move |app| app.color_code_edited(code.as_str())));
    actions.on_color_ok(|| with_app(App::color_ok));
    actions.on_path_ok(|text| with_app(move |app| app.path_entered(text.as_str())));
    actions.on_dialog_dismissed(|| with_app(App::dialog_dismissed));
    actions.on_quit(|| with_app(App::quit));
    let weak = window.as_weak();
    actions.on_move_focus(move |forward| {
        // Not while the key that asked for this is still being handled.
        let weak = weak.clone();
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(window) = weak.upgrade() {
                let key = if forward { Key::Tab } else { Key::Backtab };
                press(window.window(), key);
            }
        });
    });
    let weak = window.as_weak();
    actions.on_drop_focus(move || {
        if let Some(window) = weak.upgrade() {
            window.invoke_drop_focus();
        }
    });
    window.window().on_close_requested(|| {
        let mut response = CloseRequestResponse::KeepWindowShown;
        APP.with(|cell| {
            if let Ok(mut app) = cell.try_borrow_mut()
                && let Some(app) = app.as_mut()
            {
                response = app.close_requested();
            }
        });
        response
    });
}

fn step_delay(app: &mut App, slower: bool) {
    let cmd = app.state.step_delay(slower);
    app.send(cmd);
    app.refresh();
}
