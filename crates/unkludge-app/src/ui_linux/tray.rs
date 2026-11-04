//! The tray icon, through StatusNotifierItem: KDE has it, and GNOME with the AppIndicator extension.
//! Without one the app can't hide in it, so it stays minimized instead, and starts that way.

use std::sync::atomic::{AtomicBool, Ordering};

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem};

use super::app::{App, on_ui};
use crate::i18n::t;
use crate::icon;
use crate::worker::Cmd;

/// Size of the tray's icon, as it goes with no scaling.
const ICON_SIZE: usize = 32;

/// Whether there's a tray to stay in right now.
static ONLINE: AtomicBool = AtomicBool::new(false);

pub fn is_online() -> bool {
    ONLINE.load(Ordering::Relaxed)
}

/// How the window starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    Shown,
    /// In the tray, where nothing shows.
    Hidden,
    /// In the dock, as there's no tray to go to.
    Minimized,
}

/// `--tray` means out of the way, and how far depends on there being a tray.
pub fn start(start_hidden: bool, tray: bool) -> Start {
    match (start_hidden, tray) {
        (false, _) => Start::Shown,
        (true, true) => Start::Hidden,
        (true, false) => Start::Minimized,
    }
}

/// What closing the window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    /// Hides it in the tray.
    Hide,
    /// Says first that it keeps running. Closing again hides it.
    NoteFirst,
    /// There's no tray: into the dock.
    Minimize,
    /// There's no tray and it was just minimized, and yet here it is again, so the window manager
    /// doesn't minimize. Hiding it is all that's left, and opening the app again shows it.
    HideAnyway,
}

pub fn close(tray: bool, note_shown: bool, just_minimized: bool) -> Close {
    match (tray, note_shown, just_minimized) {
        (true, false, _) => Close::NoteFirst,
        (true, true, _) => Close::Hide,
        (false, _, false) => Close::Minimize,
        (false, _, true) => Close::HideAnyway,
    }
}

/// See the module's description.
pub struct Tray {
    pub title: String,
    pub id: &'static str,
    pub autostart: bool,
}

/// What a tray icon wants: `A R G B`, from the `B G R A` of the icon.
fn argb_from_bgra(bgra: &[u8]) -> Vec<u8> {
    let (pixels, _) = bgra.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[b, g, r, a]| [a, r, g, b])
        .collect()
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        self.id.into()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![ksni::Icon {
            width: ICON_SIZE as i32,
            height: ICON_SIZE as i32,
            data: argb_from_bgra(&icon::pixels(ICON_SIZE)),
        }]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        on_ui(App::show_window);
    }

    fn watcher_online(&self) {
        ONLINE.store(true, Ordering::Relaxed);
        on_ui(App::tray_came_online);
    }

    /// The service keeps waiting, in case one comes back.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        ONLINE.store(false, Ordering::Relaxed);
        on_ui(App::tray_went_offline);
        true
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: t().tray_open.into(),
                activate: Box::new(|_| on_ui(App::show_window)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: t().tray_sync_clock.into(),
                activate: Box::new(|_| on_ui(|app| app.send(Some(Cmd::SyncClock)))),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: t().start_with_system.into(),
                checked: self.autostart,
                activate: Box::new(|_| on_ui(App::toggle_autostart)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: t().tray_quit.into(),
                activate: Box::new(|_| on_ui(App::quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Starts the tray. Having no tray to show in is fine, as it can turn up later, but with no session
/// bus at all there's none now and never will be.
pub fn spawn(tray: Tray) -> Option<Handle<Tray>> {
    ONLINE.store(true, Ordering::Relaxed);
    match tray.assume_sni_available(true).spawn() {
        Ok(handle) => Some(handle),
        Err(error) => {
            crate::log::error(&format!("there's no tray: {error}"));
            ONLINE.store(false, Ordering::Relaxed);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    fn tray(autostart: bool) -> Tray {
        Tray {
            title: "Unkludge".to_string(),
            id: "unkludge",
            autostart,
        }
    }

    fn labels(tray: &Tray) -> Vec<String> {
        tray.menu()
            .iter()
            .map(|item| match item {
                ksni::MenuItem::Standard(item) => item.label.clone(),
                ksni::MenuItem::Checkmark(item) => item.label.clone(),
                ksni::MenuItem::Separator => "-".to_string(),
                _ => "?".to_string(),
            })
            .collect()
    }

    #[test]
    fn the_menu_has_what_the_spec_says() {
        assert_eq!(
            labels(&tray(false)),
            [
                t().tray_open,
                t().tray_sync_clock,
                t().start_with_system,
                "-",
                t().tray_quit
            ]
        );
    }

    #[test]
    fn the_menu_shows_whether_it_starts_with_the_system() {
        for on in [false, true] {
            let menu = tray(on).menu();
            let checked = menu.iter().find_map(|item| match item {
                ksni::MenuItem::Checkmark(item) => Some(item.checked),
                _ => None,
            });
            assert_eq!(checked, Some(on));
        }
    }

    #[test]
    fn its_icon_is_the_size_it_says() {
        let icons = tray(false).icon_pixmap();
        assert_eq!(icons.len(), 1);
        assert_eq!(icons[0].data.len(), ICON_SIZE * ICON_SIZE * 4);
    }

    #[test]
    fn pixels_go_into_the_order_a_tray_takes() {
        assert_eq!(argb_from_bgra(&[1, 2, 3, 4]), [4, 3, 2, 1]);
    }

    #[test]
    fn with_no_tray_a_start_in_the_tray_is_minimized_rather_than_lost() {
        assert_eq!(start(false, true), Start::Shown);
        assert_eq!(start(false, false), Start::Shown);
        assert_eq!(start(true, true), Start::Hidden);
        assert_eq!(start(true, false), Start::Minimized);
    }

    #[test]
    fn closing_says_once_that_it_keeps_running() {
        assert_eq!(close(true, false, false), Close::NoteFirst);
        assert_eq!(close(true, true, false), Close::Hide);
    }

    #[test]
    fn closing_with_no_tray_minimizes_and_hides_if_that_didnt_work() {
        assert_eq!(close(false, false, false), Close::Minimize);
        assert_eq!(close(false, true, false), Close::Minimize);
        assert_eq!(close(false, true, true), Close::HideAnyway);
    }
}
