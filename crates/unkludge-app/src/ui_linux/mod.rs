//! The Linux interface: a window drawn in software by Slint, and a tray icon through StatusNotifierItem.
//!
//! The markup in `slint/` only draws what it's given and passes on what's done to it. `view` works out
//! what it's given, `app` holds it all together and does what follows from each thing, `fit` sizes the
//! window for its screen, `tray` is the tray, `dialogs` asks for files, `resume` notices the computer
//! waking up and `shot` draws the demo's pictures with no window. What the state means and where things
//! go aren't here: see `state`, `drive` and `layout`, which the Windows interface shares.

mod app;
mod dialogs;
mod fit;
mod resume;
mod shot;
#[cfg(test)]
mod tests;
mod tray;
mod view;

use slint::ComponentHandle;

use crate::args::Args;
use crate::session::{Session, SystemAutostart};
use crate::{settings, sys};
use app::{App, RealOutside};

slint::slint! {
    export {
        Actions, AppWindow, Area, Banner, Choice, ColorDialog, DialogKind, EmptyView, Frame,
        FooterView, HeaderData, Labels, LightsView, Palette, ScreenView, Swatch, Tone, View
    } from "slint/app.slint";
}

/// The id desktop files and docks know the app by.
const APP_ID: &str = "unkludge";

/// Runs the app until it's told to quit.
pub fn run(args: Args) {
    let session = Session::new(args.demo);
    if session.claims_single_instance() && !sys::claim_single_instance() {
        if Session::shows_other_copy(args.tray) {
            sys::show_other_copy();
        }
        return;
    }
    // A scripted demo draws its picture without ever showing up on the desktop.
    if session.is_scripted() {
        shot::use_headless_platform();
    } else {
        // So that the dock groups the window with its icon.
        let _ = slint::set_xdg_app_id(APP_ID);
        // This is Slint's way to have a scale other than the system's. See `fit`.
        if let Some(dpi) = session.demo_args().and_then(|args| args.dpi) {
            // SAFETY: nothing else is running yet to be reading the environment.
            unsafe { std::env::set_var("SLINT_SCALE_FACTOR", (dpi as f32 / 96.0).to_string()) };
        }
    }
    let window =
        AppWindow::new().unwrap_or_else(|error| panic!("the window can't be made: {error}"));
    // Only now, as a first launch makes the app start with the system, which a copy with no display
    // to open a window on shouldn't do.
    let installed =
        std::env::current_exe().is_ok_and(|exe| settings::starts_with_system_on_first_launch(&exe));
    let data_dir = sys::data_dir();
    let settings = session.load_settings(data_dir.as_deref(), &SystemAutostart, installed);
    App::start(
        window.clone_strong(),
        settings,
        session,
        args.tray,
        &RealOutside,
    );
    if let Err(error) = slint::run_event_loop_until_quit() {
        crate::log::error(&format!("the window's event loop ended badly: {error}"));
    }
    app::finish();
}
