//! Where things go in the window, in DIPs (1/96 inch). Pure numbers, so that they're tested without
//! a window. The Windows interface draws to these; the rest of what it draws is its own business.

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

/// Where a step is on a slider running from `x0` to `x1`.
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

    #[test]
    fn the_window_fits_a_small_laptop_at_100_percent() {
        for height in heights() {
            assert!(Frame::new(0, height).height <= 680.0);
        }
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
