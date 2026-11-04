use super::*;

#[test]
fn a_rect_knows_what_it_holds() {
    let r = R::new(10.0, 20.0, 30.0, 60.0);
    assert!(r.contains(10.0, 20.0));
    assert!(r.contains(30.0, 60.0));
    assert!(!r.contains(9.0, 20.0));
    assert!(!r.contains(10.0, 61.0));
    assert_eq!((r.w(), r.h(), r.mid_y()), (20.0, 40.0, 40.0));
    assert_eq!(r.inset(1.0), R::new(11.0, 21.0, 29.0, 59.0));
}

#[test]
fn colours_mix_from_one_to_the_other() {
    let mid = lerp(alpha(BG, 0.0), alpha(BG, 1.0), 0.5);
    assert_eq!(mid.a, 0.5);
    assert!((lerp(ACCENT, GREEN, 0.0).r - ACCENT.r).abs() < 0.001);
    assert!((lerp(ACCENT, GREEN, 1.0).g - GREEN.g).abs() < 0.001);
}

/// How a colour stands out against another, as the accessibility guidelines measure it.
fn contrast(a: D2D1_COLOR_F, b: D2D1_COLOR_F) -> f32 {
    let channel = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let light =
        |c: D2D1_COLOR_F| 0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b);
    let (a, b) = (light(a), light(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn contrast_is_measured_as_the_guidelines_do() {
    let (white, black) = (rgb(0xFFFFFF, 1.0), rgb(0x000000, 1.0));
    assert!((contrast(white, black) - 21.0).abs() < 0.01);
    assert!((contrast(white, white) - 1.0).abs() < 0.01);
}

#[test]
fn text_on_the_accent_colour_can_be_read() {
    assert!(contrast(TEXT, ACCENT) >= 4.5, "{}", contrast(TEXT, ACCENT));
}

#[test]
fn text_on_cards_can_be_read() {
    for color in [TEXT, SILVER, MUTED, GREEN, AMBER, RED, ACCENT_HI, CYAN] {
        assert!(contrast(color, CARD) >= 4.5, "{}", contrast(color, CARD));
    }
}

#[test]
fn sliders_and_toggles_can_be_seen_against_a_card() {
    assert!(contrast(TRACK, CARD) >= 3.0, "{}", contrast(TRACK, CARD));
    assert!(contrast(ACCENT, CARD) >= 3.0, "{}", contrast(ACCENT, CARD));
}

#[test]
fn a_chosen_chip_stands_out_while_disabled() {
    assert!(contrast(SELECTED_DISABLED, SURFACE) >= 1.5);
    assert!(contrast(TEXT, SELECTED_DISABLED) >= 4.5);
    assert!(contrast(TEXT, SELECTED_DISABLED) > contrast(DIM, SURFACE));
}

#[test]
fn the_logo_uses_the_icons_colours() {
    let [r, g, b] = crate::icon::TOP;
    assert_eq!(ACCENT.r, r / 255.0);
    assert_eq!(ACCENT.g, g / 255.0);
    assert_eq!(ACCENT.b, b / 255.0);
}
