//! Where things go in the window, in DIPs (1/96 inch). Pure numbers, so that they're tested without
//! a window. Both interfaces draw to these; the rest of what they draw is their own business.

/// A rectangle: left, top, right, bottom.
pub type Rect = (f32, f32, f32, f32);

pub const WIDTH: f32 = 900.0;
const LEFT: f32 = 20.0;
const RIGHT: f32 = WIDTH - LEFT;
const GAP: f32 = 16.0;
/// The screen card holds the widest preview, 320 wide, with room to breathe.
const SCREEN_CARD_W: f32 = 400.0;
const HEADER_H: f32 = 76.0;
const BANNER_H: f32 = 44.0;
const BANNER_GAP: f32 = 8.0;
const FOOTER_H: f32 = 44.0;
const CLOCK_H: f32 = 84.0;
const CARD_PAD: f32 = 18.0;
const TITLE_H: f32 = 46.0;

const WELL_PAD: f32 = 8.0;
const FILE_H: f32 = 36.0;
const ROW_H: f32 = 30.0;
const ROW_GAP: f32 = 8.0;
/// Four lines of small text, which is what the longest pair of notes takes, in either language.
const NOTES_H: f32 = 62.0;
const ACTION_H: f32 = 36.0;

/// How much of the screen card isn't the preview's well.
const SCREEN_REST: f32 =
    TITLE_H + ROW_GAP + FILE_H + ROW_GAP + ROW_H + ROW_GAP + NOTES_H + ROW_GAP + ACTION_H + 14.0;

const EFFECT_COLUMNS: usize = 4;
const EFFECT_H: f32 = 28.0;
const EFFECT_GAP: f32 = 5.0;
/// Off and the most effects any model has.
const EFFECT_ROWS: usize = 18usize.div_ceil(EFFECT_COLUMNS);
const SECTION_GAP: f32 = 11.0;
const SLIDERS_H: f32 = 44.0;
const COLORS_H: f32 = 52.0;
const SLEEP_H: f32 = 48.0;
/// Between the lights' two sliders.
const SLIDER_GAP: f32 = 24.0;
/// How far in from its part of the row a slider's thumb runs.
#[cfg_attr(not(windows), allow(dead_code))]
const SLIDER_INSET: f32 = 8.0;
const CHIP_GAP: f32 = 6.0;
/// The chips for how a picture fits, which leave the rest of their row for the time per frame.
const FIT_CHIPS_W: f32 = 234.0;
/// What the sleep chips leave above them for their name.
const SLEEP_CHIPS_TOP: f32 = 20.0;

const LIGHTS_H: f32 = TITLE_H + EFFECT_ROWS as f32 * (EFFECT_H + EFFECT_GAP) - EFFECT_GAP
    + SECTION_GAP
    + SLIDERS_H
    + SECTION_GAP
    + COLORS_H
    + SECTION_GAP
    + SLEEP_H
    + 16.0;

/// The most height the two columns ever need, with no preview taller than the screen card allows.
const COLUMNS_MIN_H: f32 = LIGHTS_H + GAP + CLOCK_H;

/// Below this the small text, at 11 DIPs, would be under 9 pixels and hard to read.
pub const MIN_DPI: f32 = 80.0;
const _: () = assert!(11.0 * MIN_DPI / 96.0 >= 9.0);

/// A rectangle of pixels on the desktop: left, top, right, bottom.
pub type PixelRect = (i32, i32, i32, i32);

/// The DPI to draw at, so that the window fits the screen. That's the monitor's, unless a window of
/// `client` DIPs plus its `frame` of pixels would be larger than the `work` area of pixels. Then
/// everything gets drawn smaller, down to [`MIN_DPI`] and never larger than the monitor's.
pub fn effective_dpi(dpi: f32, client: (f32, f32), frame: (i32, i32), work: (i32, i32)) -> f32 {
    let fitting = |work: i32, frame: i32, dips: f32| (work - frame).max(1) as f32 * 96.0 / dips;
    let fits = dpi
        .min(fitting(work.0, frame.0, client.0))
        .min(fitting(work.1, frame.1, client.1));
    if fits < dpi {
        // Whole numbers, so that rounding the window's size up can't push it past the edge.
        fits.floor().max(MIN_DPI.min(dpi))
    } else {
        dpi
    }
}

/// How many pixels some DIPs take at a DPI.
pub fn pixels(dips: f32, dpi: f32) -> i32 {
    (dips * dpi / 96.0).ceil() as i32
}

/// Where a window of this size goes to stay in the work area. If it's too large for that, its title
/// bar is what stays on screen, top left.
pub fn keep_in_work_area(x: i32, y: i32, size: (i32, i32), work: PixelRect) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    (
        x.min(right - size.0).max(left),
        y.min(bottom - size.1).max(top),
    )
}

/// Where a window of this size goes to be in the middle of the work area. On Linux that's up to the
/// window manager.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn centered_in_work_area(size: (i32, i32), work: PixelRect) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    keep_in_work_area(
        left + (right - left - size.0) / 2,
        top + (bottom - top - size.1) / 2,
        size,
        work,
    )
}

/// Where each part of the window goes. The window keeps its size whatever happens, except that a
/// banner adds to its height.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub banners: Vec<Rect>,
    pub screen: Rect,
    pub lights: Rect,
    pub clock: Rect,
    /// The screen and lights cards' place together, for when there's no keyboard to show them for.
    pub empty: Rect,
    pub footer: Rect,
    pub height: f32,
}

impl Frame {
    /// `screen_height` is the tallest thing the preview has to show, in pixels.
    pub fn new(banners: usize, screen_height: u16) -> Frame {
        let mut y = HEADER_H;
        let banners = (0..banners)
            .map(|_| {
                let banner = (LEFT, y, RIGHT, y + BANNER_H);
                y += BANNER_H + BANNER_GAP;
                banner
            })
            .collect();
        let needed = SCREEN_REST + 2.0 * WELL_PAD + f32::from(screen_height);
        let height = needed.max(COLUMNS_MIN_H);
        let bottom = y + height;
        let split = LEFT + SCREEN_CARD_W;
        let lights_bottom = bottom - CLOCK_H - GAP;
        let footer_top = bottom + GAP - 4.0;
        let footer = (LEFT, footer_top, RIGHT, footer_top + FOOTER_H);
        Frame {
            banners,
            screen: (LEFT, y, split, bottom),
            lights: (split + GAP, y, RIGHT, lights_bottom),
            clock: (split + GAP, lights_bottom + GAP, RIGHT, bottom),
            empty: (LEFT, y, RIGHT, bottom),
            footer,
            height: footer.3 + 12.0,
        }
    }
}

/// The rows of the screen card, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRows {
    /// Where the preview sits, taking whatever the other rows leave.
    pub well: Rect,
    pub file: Rect,
    pub fit: Rect,
    pub notes: Rect,
    pub action: Rect,
}

pub fn screen_rows(card: Rect) -> ScreenRows {
    let (x0, y0, x1, y1) = card;
    let (x0, x1) = (x0 + CARD_PAD, x1 - CARD_PAD);
    let action = (x0, y1 - 14.0 - ACTION_H, x1, y1 - 14.0);
    let notes = (x0, action.1 - ROW_GAP - NOTES_H, x1, action.1 - ROW_GAP);
    let fit = (x0, notes.1 - ROW_GAP - ROW_H, x1, notes.1 - ROW_GAP);
    let file = (x0, fit.1 - ROW_GAP - FILE_H, x1, fit.1 - ROW_GAP);
    let well = (x0, y0 + TITLE_H, x1, file.1 - ROW_GAP);
    ScreenRows {
        well,
        file,
        fit,
        notes,
        action,
    }
}

/// The rows of the lights card, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightRows {
    pub effects: Rect,
    pub sliders: Rect,
    pub colors: Rect,
    pub sleep: Rect,
}

pub fn light_rows(card: Rect) -> LightRows {
    let (x0, y0, x1, _) = card;
    let (x0, x1) = (x0 + CARD_PAD, x1 - CARD_PAD);
    let top = y0 + TITLE_H;
    let effects_h = EFFECT_ROWS as f32 * (EFFECT_H + EFFECT_GAP) - EFFECT_GAP;
    let effects = (x0, top, x1, top + effects_h);
    let sliders_top = effects.3 + SECTION_GAP;
    let sliders = (x0, sliders_top, x1, sliders_top + SLIDERS_H);
    let colors_top = sliders.3 + SECTION_GAP;
    let colors = (x0, colors_top, x1, colors_top + COLORS_H);
    let sleep_top = colors.3 + SECTION_GAP;
    let sleep = (x0, sleep_top, x1, sleep_top + SLEEP_H);
    LightRows {
        effects,
        sliders,
        colors,
        sleep,
    }
}

/// Where the lights' two sliders go: brightness, then speed.
pub fn slider_parts(sliders: Rect) -> [Rect; 2] {
    let parts = split_row(sliders, 2, SLIDER_GAP);
    [parts[0], parts[1]]
}

/// What a slider's thumb runs along, in its part of the row. The Linux interface's markup works out
/// its own, where it's drawn.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn slider_track(part: Rect) -> (f32, f32) {
    (part.0 + SLIDER_INSET, part.2 - SLIDER_INSET)
}

/// Where the chips for how a picture fits go, in their row.
pub fn fit_chips(row: Rect, count: usize) -> Vec<Rect> {
    let chips = (row.0, row.1, row.0 + FIT_CHIPS_W, row.3);
    split_row(chips, count, CHIP_GAP)
}

/// Where the sleep chips go, in the sleep row.
pub fn sleep_chips(row: Rect, count: usize) -> Vec<Rect> {
    let chips = (row.0, row.1 + SLEEP_CHIPS_TOP, row.2, row.3);
    split_row(chips, count, CHIP_GAP)
}

/// Where each effect's chip goes: left to right, then top to bottom.
pub fn effect_chips(area: Rect, count: usize) -> Vec<Rect> {
    let width =
        (area.2 - area.0 - EFFECT_GAP * (EFFECT_COLUMNS as f32 - 1.0)) / EFFECT_COLUMNS as f32;
    (0..count)
        .map(|i| {
            let x = area.0 + (i % EFFECT_COLUMNS) as f32 * (width + EFFECT_GAP);
            let y = area.1 + (i / EFFECT_COLUMNS) as f32 * (EFFECT_H + EFFECT_GAP);
            (x, y, x + width, y + EFFECT_H)
        })
        .collect()
}

/// Equal parts of a row.
pub fn split_row(area: Rect, count: usize, gap: f32) -> Vec<Rect> {
    let n = count.max(1) as f32;
    let width = (area.2 - area.0 - gap * (n - 1.0)) / n;
    (0..count)
        .map(|i| {
            let x = area.0 + i as f32 * (width + gap);
            (x, area.1, x + width, area.3)
        })
        .collect()
}

/// A rectangle of this size in the middle of another, on whole DIPs, so that a preview at 100 % has
/// one screen pixel for each pixel of the image.
pub fn centered(area: Rect, width: f32, height: f32) -> Rect {
    let x = ((area.0 + area.2 - width) / 2.0).floor();
    let y = ((area.1 + area.3 - height) / 2.0).floor();
    (x, y, x + width, y + height)
}

/// The step of `steps` nearest to `x` on a slider running from `x0` to `x1`.
pub fn step_at(x: f32, x0: f32, x1: f32, steps: usize) -> usize {
    if steps < 2 || x1 <= x0 {
        return 0;
    }
    let share = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    (share * (steps - 1) as f32).round() as usize
}

/// Where a step is on a slider running from `x0` to `x1`. The Linux interface's markup works it out
/// where it's drawn.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn step_x(step: usize, x0: f32, x1: f32, steps: usize) -> f32 {
    if steps < 2 {
        return x0;
    }
    x0 + (x1 - x0) * step.min(steps - 1) as f32 / (steps - 1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::model;

    fn inside(inner: Rect, outer: Rect) -> bool {
        inner.0 >= outer.0 && inner.1 >= outer.1 && inner.2 <= outer.2 && inner.3 <= outer.3
    }

    fn apart(a: Rect, b: Rect) -> bool {
        a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1
    }

    fn positive(r: Rect) -> bool {
        r.2 > r.0 && r.3 > r.1
    }

    /// Every screen size any model has, and none.
    fn heights() -> Vec<u16> {
        let mut heights: Vec<u16> = model::supported()
            .map(|m| m.screen.unwrap().height)
            .collect();
        heights.extend([0, 172]);
        heights
    }

    #[test]
    fn the_cards_stay_inside_the_window_and_apart() {
        for height in heights() {
            for banners in 0..3 {
                let f = Frame::new(banners, height);
                let window = (0.0, 0.0, WIDTH, f.height);
                let mut parts = vec![f.screen, f.lights, f.clock, f.footer];
                parts.extend(&f.banners);
                for (i, part) in parts.iter().enumerate() {
                    assert!(positive(*part));
                    assert!(inside(*part, window), "{part:?} in {window:?}");
                    for other in &parts[i + 1..] {
                        assert!(apart(*part, *other), "{part:?} and {other:?}");
                    }
                }
                assert!(inside(f.screen, f.empty) && inside(f.lights, f.empty));
                assert!(apart(f.empty, f.footer));
            }
        }
    }

    #[test]
    fn the_preview_of_every_model_fits_its_well() {
        for model in model::supported() {
            let screen = model.screen.unwrap();
            let frame = Frame::new(1, screen.height);
            let well = screen_rows(frame.screen).well;
            let preview = centered(well, f32::from(screen.width), f32::from(screen.height));
            assert!(inside(preview, well), "{}: {preview:?}", model.name);
        }
    }

    #[test]
    fn the_rows_of_the_screen_card_stay_inside_it_and_apart() {
        for height in heights() {
            let card = Frame::new(0, height).screen;
            let r = screen_rows(card);
            let rows = [r.well, r.file, r.fit, r.notes, r.action];
            for (i, row) in rows.iter().enumerate() {
                assert!(positive(*row), "{row:?}");
                assert!(inside(*row, card), "{row:?} in {card:?}");
                for other in &rows[i + 1..] {
                    assert!(apart(*row, *other), "{row:?} and {other:?}");
                }
            }
        }
    }

    #[test]
    fn the_rows_of_the_lights_card_stay_inside_it_and_apart() {
        for height in heights() {
            let card = Frame::new(0, height).lights;
            let r = light_rows(card);
            let rows = [r.effects, r.sliders, r.colors, r.sleep];
            for (i, row) in rows.iter().enumerate() {
                assert!(positive(*row), "{row:?}");
                assert!(inside(*row, card), "{row:?} in {card:?}");
                for other in &rows[i + 1..] {
                    assert!(apart(*row, *other), "{row:?} and {other:?}");
                }
            }
        }
    }

    #[test]
    fn every_models_effects_and_off_have_a_chip_inside_the_area() {
        let card = Frame::new(0, 172).lights;
        let area = light_rows(card).effects;
        for model in model::supported() {
            let count = model.lighting.effects.len() + 1;
            let chips = effect_chips(area, count);
            assert_eq!(chips.len(), count);
            for (i, chip) in chips.iter().enumerate() {
                assert!(inside(*chip, area), "{}: {chip:?}", model.name);
                for other in &chips[i + 1..] {
                    assert!(apart(*chip, *other));
                }
            }
        }
    }

    #[test]
    fn a_banner_makes_the_window_taller_and_moves_the_cards_down() {
        let plain = Frame::new(0, 172);
        let with = Frame::new(1, 172);
        let moved = BANNER_H + BANNER_GAP;
        assert_eq!(with.banners.len(), 1);
        assert_eq!(with.height, plain.height + moved);
        assert_eq!(with.screen.1, plain.screen.1 + moved);
        assert_eq!(with.banners[0].3, with.screen.1 - BANNER_GAP);
    }

    #[test]
    fn the_window_keeps_its_size_for_wide_screens() {
        let wide = Frame::new(0, 172).height;
        assert_eq!(Frame::new(0, 135).height, wide);
        assert_eq!(Frame::new(0, 128).height, wide);
    }

    /// A window's frame at a DPI, as measured: 18 by 47 pixels at 125 %.
    fn frame_at(dpi: f32) -> (i32, i32) {
        (
            (14.4 * dpi / 96.0).ceil() as i32,
            (37.6 * dpi / 96.0).ceil() as i32,
        )
    }

    /// A screen's work area at a DPI, with the usual 48 DIPs of taskbar.
    fn work_of(screen: (i32, i32), dpi: f32) -> (i32, i32) {
        (screen.0, screen.1 - (48.0 * dpi / 96.0).ceil() as i32)
    }

    /// Common screens and scales.
    const SCREENS: [((i32, i32), f32); 10] = [
        ((1280, 720), 96.0),
        ((1366, 768), 96.0),
        ((1366, 768), 120.0),
        ((1536, 864), 120.0),
        ((1920, 1080), 96.0),
        ((1920, 1080), 120.0),
        ((1920, 1080), 144.0),
        ((1920, 1080), 168.0),
        ((2560, 1440), 192.0),
        ((3840, 2160), 288.0),
    ];

    #[test]
    fn the_window_fits_common_screens_at_every_scale_with_or_without_banners() {
        for (screen, dpi) in SCREENS {
            for banners in 0..3 {
                for height in heights() {
                    let needed = Frame::new(banners, height).height;
                    let frame = frame_at(dpi);
                    let work = work_of(screen, dpi);
                    let drawn = effective_dpi(dpi, (WIDTH, needed), frame, work);
                    assert!(drawn >= MIN_DPI, "{screen:?} at {dpi}: {drawn}");
                    assert!(drawn <= dpi, "{screen:?} at {dpi}: {drawn}");
                    assert!(
                        pixels(needed, drawn) + frame.1 <= work.1,
                        "{screen:?} at {dpi} with {banners} banners, {height} tall: {drawn}"
                    );
                    assert!(
                        pixels(WIDTH, drawn) + frame.0 <= work.0,
                        "{screen:?} at {dpi}: {drawn}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_screen_with_room_gets_the_monitors_own_scale() {
        let needed = Frame::new(0, 172).height;
        for (screen, dpi) in [
            ((1920, 1080), 96.0),
            ((1920, 1080), 120.0),
            ((2560, 1440), 144.0),
        ] {
            let drawn = effective_dpi(dpi, (WIDTH, needed), frame_at(dpi), work_of(screen, dpi));
            assert_eq!(drawn, dpi, "{screen:?}");
        }
    }

    #[test]
    fn a_small_laptop_at_125_percent_draws_smaller_to_fit() {
        let needed = Frame::new(0, 172).height;
        let work = work_of((1366, 768), 120.0);
        let drawn = effective_dpi(120.0, (WIDTH, needed), frame_at(120.0), work);
        assert!(drawn < 120.0, "{drawn}");
        assert!(drawn > 96.0, "{drawn}: still larger than at 100 %");
    }

    #[test]
    fn a_narrow_screen_also_draws_smaller() {
        let work = (1024, 2000);
        let drawn = effective_dpi(120.0, (WIDTH, 640.0), (18, 47), work);
        assert!(pixels(WIDTH, drawn) + 18 <= 1024, "{drawn}");
    }

    #[test]
    fn nothing_gets_drawn_too_small_to_read() {
        // Too small for anything: the text stays readable, and the window hangs off the bottom.
        let drawn = effective_dpi(96.0, (WIDTH, 640.0), (14, 38), (1024, 400));
        assert_eq!(drawn, MIN_DPI);
        // A monitor already below that keeps its own.
        assert_eq!(
            effective_dpi(72.0, (WIDTH, 640.0), (14, 38), (800, 400)),
            72.0
        );
    }

    #[test]
    fn a_window_stays_in_the_work_area() {
        let work = (100, 50, 1100, 750);
        assert_eq!(keep_in_work_area(200, 100, (300, 200), work), (200, 100));
        assert_eq!(keep_in_work_area(-500, -500, (300, 200), work), (100, 50));
        assert_eq!(keep_in_work_area(5000, 5000, (300, 200), work), (800, 550));
    }

    #[test]
    fn a_window_too_large_for_the_work_area_keeps_its_title_bar_on_screen() {
        let work = (0, 0, 1366, 708);
        assert_eq!(keep_in_work_area(10, 10, (1500, 900), work), (0, 0));
        assert_eq!(centered_in_work_area((1500, 900), work), (0, 0));
    }

    #[test]
    fn a_window_opens_in_the_middle_of_the_work_area_of_any_monitor() {
        assert_eq!(
            centered_in_work_area((100, 50), (0, 0, 1000, 500)),
            (450, 225)
        );
        // A monitor to the left of the main one.
        assert_eq!(
            centered_in_work_area((100, 50), (-1000, 0, 0, 500)),
            (-550, 225)
        );
    }

    #[test]
    fn split_rows_fill_the_area_with_equal_parts() {
        let parts = split_row((10.0, 0.0, 110.0, 20.0), 4, 4.0);
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].0, 10.0);
        assert_eq!(parts[3].2, 110.0);
        let width = parts[0].2 - parts[0].0;
        assert_eq!(parts[2].2 - parts[2].0, width);
        assert_eq!(parts[1].0, parts[0].2 + 4.0);
    }

    #[test]
    fn the_sliders_share_their_row_with_a_gap_between_them() {
        let [brightness, speed] = slider_parts((10.0, 5.0, 134.0, 49.0));
        assert_eq!(brightness, (10.0, 5.0, 60.0, 49.0));
        assert_eq!(speed, (84.0, 5.0, 134.0, 49.0));
        assert_eq!(slider_track(speed), (92.0, 126.0));
    }

    #[test]
    fn the_fit_chips_leave_the_rest_of_their_row_alone() {
        let row = (18.0, 400.0, 382.0, 430.0);
        let chips = fit_chips(row, 3);
        assert_eq!(chips.len(), 3);
        assert_eq!(chips[0].0, row.0);
        assert_eq!(chips[2].2, row.0 + 234.0);
        assert_eq!(chips[1].0, chips[0].2 + 6.0);
        assert!(chips.iter().all(|chip| chip.1 == row.1 && chip.3 == row.3));
    }

    #[test]
    fn the_sleep_chips_go_under_their_name() {
        let chips = sleep_chips((0.0, 100.0, 90.0, 148.0), 4);
        assert_eq!(chips.len(), 4);
        assert!(chips.iter().all(|chip| chip.1 == 120.0 && chip.3 == 148.0));
        assert_eq!(chips[3].2, 90.0);
    }

    #[test]
    fn something_centered_sits_on_whole_dips() {
        let c = centered((0.0, 0.0, 101.0, 51.0), 40.0, 20.0);
        assert_eq!(c, (30.0, 15.0, 70.0, 35.0));
    }

    #[test]
    fn slider_positions_round_to_the_nearest_step() {
        assert_eq!(step_at(0.0, 0.0, 100.0, 6), 0);
        assert_eq!(step_at(100.0, 0.0, 100.0, 6), 5);
        assert_eq!(step_at(49.0, 0.0, 100.0, 6), 2);
        assert_eq!(step_at(51.0, 0.0, 100.0, 6), 3);
        // Past the ends, and sliders with nothing to choose.
        assert_eq!(step_at(-40.0, 0.0, 100.0, 6), 0);
        assert_eq!(step_at(400.0, 0.0, 100.0, 6), 5);
        assert_eq!(step_at(50.0, 0.0, 100.0, 1), 0);
        assert_eq!(step_at(50.0, 0.0, 100.0, 0), 0);
        assert_eq!(step_at(5.0, 5.0, 5.0, 6), 0);
    }

    #[test]
    fn a_step_is_where_it_reads_back_from() {
        for steps in 2..8 {
            for step in 0..steps {
                let x = step_x(step, 20.0, 180.0, steps);
                assert_eq!(step_at(x, 20.0, 180.0, steps), step);
            }
        }
        assert_eq!(step_x(9, 20.0, 180.0, 6), 180.0);
        assert_eq!(step_x(0, 20.0, 180.0, 1), 20.0);
    }
}
