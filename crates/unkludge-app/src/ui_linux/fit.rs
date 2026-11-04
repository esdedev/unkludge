//! Fitting the window on the screen. There's no asking winit for a screen's work area, so what panels
//! and the title bar take is an allowance, and from there it's the same sums as on Windows: the same
//! layout, drawn smaller when it wouldn't fit, down to where small text stays readable.
//!
//! Slint's own way of scaling is `SLINT_SCALE_FACTOR`, which is read as a window gets made, and the
//! screen can't be known before that. So the window starts at the system's scale, and once it exists
//! this tells it the scale that fits, as the system does when a window moves to another monitor. Going
//! up takes a bigger surface first, as drawing to one that's still too small is a panic in the
//! renderer, so the window grows first and only then does the scale change. See [`next_step`].

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit;
use std::time::Duration;

use slint::{ComponentHandle, PhysicalPosition, PhysicalSize, Timer};

use super::AppWindow;
use crate::layout::{PixelRect, WIDTH, effective_dpi, keep_in_work_area, pixels};

/// A panel or two along one side of the screen.
const PANELS_DIPS: f32 = 48.0;
const TITLE_BAR_DIPS: f32 = 38.0;
/// What a scale must differ by to be worth changing.
const SCALE_STEP: f32 = 0.005;
/// How long a window's new limits take to reach the window manager, which holds it to the old ones.
const ASK_AGAIN_AFTER: Duration = Duration::from_millis(100);

fn dips(value: f32, dpi: f32) -> i32 {
    pixels(value, dpi)
}

/// What a window's frame takes at a DPI.
fn frame_at(dpi: f32) -> (i32, i32) {
    (0, dips(TITLE_BAR_DIPS, dpi))
}

/// The scale to draw at. `os_scale` is the system's, `work` the pixels left for windows.
pub fn scale_for(os_scale: f32, height_dips: f32, work: (i32, i32)) -> f32 {
    let dpi = os_scale * 96.0;
    effective_dpi(dpi, (WIDTH, height_dips), frame_at(dpi), work) / 96.0
}

/// What's left for windows on a screen of this many pixels.
pub fn work_of_screen(screen: (i32, i32), os_scale: f32) -> (i32, i32) {
    let panels = dips(PANELS_DIPS, os_scale * 96.0);
    (screen.0, (screen.1 - panels).max(1))
}

/// How wide to make a thin border, in Slint's pixels, for it to take up one real one at least.
pub fn hairline_at(scale: f32) -> f32 {
    (1.0 / scale).max(1.0)
}

/// The size of the picture of the window, in pixels.
pub fn size_at(height_dips: f32, scale: f32) -> (u32, u32) {
    let dpi = scale * 96.0;
    (pixels(WIDTH, dpi) as u32, pixels(height_dips, dpi) as u32)
}

/// What to do about the window's scale and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    /// The scale to take now, if it's to change.
    pub scale: Option<f32>,
    /// The size the window should have, in pixels.
    pub size: (u32, u32),
    /// The scale is to go up once the window has that size, which it doesn't yet.
    pub growing: bool,
}

/// The next step from a scale of `current` and a window of `size` pixels, wanting `fitting`. The scale
/// goes down at once, as a surface that's too large does no harm. It only goes up with a surface that
/// already has the room.
pub fn next_step(current: f32, fitting: f32, height_dips: f32, size: (u32, u32)) -> Step {
    if (fitting - current).abs() < SCALE_STEP {
        return Step {
            scale: None,
            size: size_at(height_dips, current),
            growing: false,
        };
    }
    let wanted = size_at(height_dips, fitting);
    let has_room = size.0 >= wanted.0 && size.1 >= wanted.1;
    let growing = fitting > current && !has_room;
    Step {
        scale: (!growing).then_some(fitting),
        size: wanted,
        growing,
    }
}

/// How fitting a window went.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fitted {
    pub scale: f32,
    /// It's waiting for the window to grow, and should be looked at again.
    pub growing: bool,
}

/// What winit knows of the screen the window is on.
struct Screen {
    /// The system's scale for the window, wherever it is.
    scale: f32,
    size: (i32, i32),
    position: (i32, i32),
}

fn screen_of(window: &winit::window::Window) -> Option<Screen> {
    let monitor = window
        .current_monitor()
        .or_else(|| window.primary_monitor())
        .or_else(|| window.available_monitors().next())?;
    let size = monitor.size();
    let position = monitor.position();
    Some(Screen {
        scale: window.scale_factor() as f32,
        size: (size.width as i32, size.height as i32),
        position: (position.x, position.y),
    })
}

/// Whether a screen's size means anything. While X11 is still saying what screens it has, winit makes
/// do with a screen of one pixel, and fitting the window to that would leave it tiny for good.
fn is_a_real_screen((width, height): (i32, i32)) -> bool {
    width >= 320 && height >= 200
}

/// The scale the window was made at when that wasn't the system's to say.
fn scale_asked_for() -> Option<f32> {
    std::env::var("SLINT_SCALE_FACTOR")
        .ok()?
        .parse()
        .ok()
        .filter(|scale| *scale > 0.0)
}

/// Whether the window is on Wayland, where it can't place itself or be dropped files on.
pub fn is_wayland(window: &AppWindow) -> bool {
    use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    window
        .window()
        .with_winit_window(|winit_window| {
            winit_window
                .display_handle()
                .is_ok_and(|handle| matches!(handle.as_raw(), RawDisplayHandle::Wayland(_)))
        })
        .unwrap_or(false)
}

/// Fits a window that exists to its screen, and says what scale it's now at. With no screen to go
/// by it stays as it is. `work_area` is what a demo said the screen is like.
pub fn fit(window: &AppWindow, height_dips: f32, work_area: Option<(i32, i32)>) -> Option<Fitted> {
    let screen = window
        .window()
        .with_winit_window(screen_of)
        .flatten()
        .filter(|screen| is_a_real_screen(screen.size))?;
    let os_scale = scale_asked_for().unwrap_or(screen.scale);
    let work = work_area.unwrap_or_else(|| work_of_screen(screen.size, os_scale));
    let current = window.window().scale_factor();
    let now = window.window().size();
    let step = next_step(
        current,
        scale_for(os_scale, height_dips, work),
        height_dips,
        (now.width, now.height),
    );
    if let Some(scale) = step.scale {
        change_scale(window, scale, height_dips);
    }
    // Slint asks for a size in its logical pixels and leaves it to winit, whose scale can be another,
    // and that goes wrong too when the window starts on one monitor and ends up on another. So what
    // counts is the size it has, in real pixels.
    if window.window().size() != PhysicalSize::new(step.size.0, step.size.1) {
        take_size(window, step.size);
    }
    let scale = step.scale.unwrap_or(current);
    if !is_wayland(window) {
        keep_on_screen(window, &screen, step.size, os_scale);
    }
    Some(Fitted {
        scale,
        growing: step.growing,
    })
}

/// Changes the scale, and says the window's size in Slint's pixels with it, before anything gets to
/// draw: a window that grew in the old scale has too many of those for the new one.
fn change_scale(window: &AppWindow, scale: f32, height_dips: f32) {
    use slint::platform::WindowEvent;
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
    window.window().dispatch_event(WindowEvent::Resized {
        size: slint::LogicalSize::new(WIDTH, height_dips),
    });
}

/// Takes a size in real pixels, limits first, as Slint only works those out when the size in its own
/// pixels changes. Its asking for that may come later than this, in winit's pixels, so this looks again.
fn take_size(window: &AppWindow, (width, height): (u32, u32)) {
    let size = PhysicalSize::new(width, height);
    set_size_and_limits(window, size);
    let weak = window.as_weak();
    Timer::single_shot(ASK_AGAIN_AFTER, move || {
        if let Some(window) = weak.upgrade()
            && window.window().size() != size
        {
            set_size_and_limits(&window, size);
        }
    });
}

fn set_size_and_limits(window: &AppWindow, size: PhysicalSize) {
    let limit = winit::dpi::PhysicalSize::new(size.width, size.height);
    window.window().with_winit_window(|winit_window| {
        winit_window.set_min_inner_size(Some(limit));
        winit_window.set_max_inner_size(Some(limit));
    });
    window.window().set_size(size);
}

/// On Wayland the compositor places the window, and on X11 the window manager does at first, but it
/// won't necessarily move a window that has grown past the edge.
fn keep_on_screen(window: &AppWindow, screen: &Screen, size: (u32, u32), os_scale: f32) {
    let now = window.window().position();
    let dpi = os_scale * 96.0;
    let outer = (
        size.0 as i32 + frame_at(dpi).0,
        size.1 as i32 + frame_at(dpi).1,
    );
    let work = work_rect(screen, dpi);
    let (x, y) = keep_in_work_area(now.x, now.y, outer, work);
    if (x, y) != (now.x, now.y) {
        window.window().set_position(PhysicalPosition::new(x, y));
    }
}

fn work_rect(screen: &Screen, dpi: f32) -> PixelRect {
    let panels = dips(PANELS_DIPS, dpi);
    (
        screen.position.0,
        screen.position.1,
        screen.position.0 + screen.size.0,
        screen.position.1 + (screen.size.1 - panels).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Frame, MIN_DPI};

    /// The tallest the window gets: two banners and the tallest of the common previews.
    fn tallest() -> f32 {
        Frame::new(2, 172).height
    }

    fn fits_on(screen: (i32, i32), os_scale: f32, height: f32) {
        let work = work_of_screen(screen, os_scale);
        let scale = scale_for(os_scale, height, work);
        assert!(scale <= os_scale, "{screen:?} at {os_scale}: {scale}");
        let (width, drawn) = size_at(height, scale);
        let title_bar = frame_at(os_scale * 96.0).1;
        assert!(
            width as i32 <= work.0,
            "{screen:?} at {os_scale}: {width} wide in {work:?}"
        );
        assert!(
            drawn as i32 + title_bar <= work.1,
            "{screen:?} at {os_scale}: {drawn} tall and {title_bar} of title bar in {work:?}"
        );
    }

    #[test]
    fn it_fits_a_small_laptop_with_or_without_banners() {
        for os_scale in [1.0, 1.25] {
            for banners in 0..3 {
                fits_on((1366, 768), os_scale, Frame::new(banners, 172).height);
            }
        }
    }

    #[test]
    fn it_fits_common_screens_at_common_scales() {
        for (screen, os_scale) in [
            ((1280, 720), 1.0),
            ((1536, 864), 1.25),
            ((1920, 1080), 1.0),
            ((1920, 1080), 1.5),
            ((2560, 1440), 2.0),
            ((3840, 2160), 3.0),
        ] {
            fits_on(screen, os_scale, tallest());
        }
    }

    #[test]
    fn a_screen_with_room_keeps_the_systems_scale() {
        let height = Frame::new(0, 172).height;
        for (screen, os_scale) in [
            ((1920, 1080), 1.0),
            ((1920, 1080), 1.25),
            ((2560, 1440), 1.5),
        ] {
            let scale = scale_for(os_scale, height, work_of_screen(screen, os_scale));
            assert_eq!(scale, os_scale, "{screen:?}");
        }
    }

    #[test]
    fn at_1366_by_768_the_plain_window_needs_no_shrinking_and_two_banners_do() {
        let work = work_of_screen((1366, 768), 1.0);
        assert_eq!(scale_for(1.0, Frame::new(0, 172).height, work), 1.0);
        assert!(scale_for(1.0, Frame::new(2, 172).height, work) < 1.0);
    }

    #[test]
    fn a_screen_of_one_pixel_is_no_screen_at_all() {
        assert!(!is_a_real_screen((1, 1)));
        assert!(!is_a_real_screen((0, 0)));
        assert!(is_a_real_screen((800, 480)));
    }

    #[test]
    fn a_thin_border_stays_at_least_one_real_pixel_wide() {
        assert_eq!(hairline_at(1.0), 1.0);
        assert_eq!(hairline_at(2.0), 1.0);
        assert!(hairline_at(0.85) * 0.85 >= 0.999);
    }

    #[test]
    fn the_scale_goes_down_at_once() {
        let step = next_step(1.0, 0.9, 643.0, (900, 643));
        assert_eq!(step.scale, Some(0.9));
        assert_eq!(step.size, size_at(643.0, 0.9));
        assert!(!step.growing);
    }

    #[test]
    fn the_scale_doesnt_go_up_before_the_window_has_grown() {
        let small = size_at(643.0, 0.9);
        let step = next_step(0.9, 1.0, 643.0, small);
        assert_eq!(step.scale, None, "the renderer would panic");
        assert_eq!(step.size, size_at(643.0, 1.0));
        assert!(step.growing);
        // Room in only one direction isn't enough.
        assert_eq!(next_step(0.9, 1.0, 643.0, (900, small.1)).scale, None);
        assert_eq!(next_step(0.9, 1.0, 643.0, (small.0, 643)).scale, None);
    }

    #[test]
    fn the_scale_goes_up_once_the_window_has_room() {
        let step = next_step(0.9, 1.0, 643.0, size_at(643.0, 1.0));
        assert_eq!(step.scale, Some(1.0));
        assert!(!step.growing);
    }

    #[test]
    fn a_scale_thats_right_is_left_alone_and_the_size_still_gets_looked_at() {
        let step = next_step(1.0, 1.0, 643.0, (1050, 750));
        assert_eq!(step.scale, None);
        assert_eq!(step.size, (900, 643));
        assert!(!step.growing);
    }

    #[test]
    fn a_scale_that_differs_by_next_to_nothing_stays() {
        assert_eq!(next_step(1.0, 1.001, 643.0, (900, 643)).scale, None);
        assert_eq!(next_step(1.0, 0.999, 643.0, (900, 643)).scale, None);
    }

    #[test]
    fn nothing_gets_too_small_to_read() {
        let scale = scale_for(1.0, 640.0, (1024, 300));
        assert_eq!(scale, MIN_DPI / 96.0);
    }

    #[test]
    fn a_work_area_a_demo_says_is_taken_as_it_is() {
        let height = Frame::new(0, 172).height;
        let scale = scale_for(1.0, height, (1366, 600));
        let (_, drawn) = size_at(height, scale);
        assert!(drawn as i32 + frame_at(96.0).1 <= 600, "{drawn}");
    }

    #[test]
    fn the_work_area_starts_where_the_screen_does() {
        let screen = Screen {
            scale: 1.0,
            size: (1000, 500),
            position: (-1000, 40),
        };
        assert_eq!(work_rect(&screen, 96.0), (-1000, 40, 0, 40 + 500 - 48));
    }
}
