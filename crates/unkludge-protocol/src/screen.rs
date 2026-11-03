//! Images and animations for the keyboard's screen.
//!
//! Frames are `width * height * 2` bytes of RGB565, big-endian (`RRRRRGGG GGGBBBBB`), row by row
//! from the top left, with no header or padding. Every screen size works the same way.
//!
//! The files may come from anywhere, so decoding is limited: image size, memory, file size and, for
//! GIFs, the frame count and the total number of pixels. Past a limit it stops with an error rather
//! than carrying on.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use image::codecs::gif::GifDecoder;
use image::imageops::{self, FilterType};
use image::{
    AnimationDecoder, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits, Rgba, RgbaImage,
};

use crate::Error;
use crate::model::ScreenSpec;
use crate::packet::MAX_DATA;

/// The delay of animations whose file says 0, and of still images, in milliseconds. Browsers also
/// show GIFs with no delay at this speed.
pub const DEFAULT_DELAY_MS: u16 = 100;

/// The largest file that gets read.
const MAX_FILE_BYTES: u64 = 256 << 20;
/// The largest still image: enough for a 100-megapixel photo, but not to fill memory.
const MAX_IMAGE_SIDE: u32 = 16384;
const MAX_IMAGE_ALLOC: u64 = 512 << 20;

const GIF_LIMITS: GifLimits = GifLimits {
    max_side: 4096,
    // The decoder holds up to three images of the canvas size at once.
    max_alloc: 192 << 20,
    // Measured at about 4 s per 256 megapixels of noise. A long GIF of 800x800 needs 128.
    max_pixels: 1 << 29,
};

const RESIZE_FILTER: FilterType = FilterType::Triangle;

/// What a GIF is allowed to cost to decode. Frame count times canvas size is the work done, and a
/// tiny file can ask for a lot of it.
struct GifLimits {
    /// Largest width and height of the canvas.
    max_side: u32,
    /// Most memory the decoder may use for a frame, which may be larger than the canvas.
    max_alloc: u64,
    /// Most canvas pixels decoded over all frames.
    max_pixels: u64,
}

/// How an image is made to fit a screen of another shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// Fill the screen and crop what's left over, equally on both sides.
    #[default]
    Cover,
    /// Show the whole image, with black bars.
    Contain,
    /// Fill the screen, changing the image's shape.
    Stretch,
}

/// How an animation is loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadOptions {
    pub fit: Fit,
    /// Milliseconds per frame, instead of what the file says. 0 is refused.
    pub delay_ms: Option<u16>,
    /// Keep the frames that fit the model's limit, instead of [`Error::TooManyFrames`].
    pub truncate: bool,
}

/// An image or animation, ready to send to a screen of its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Animation {
    frames: Vec<Vec<u8>>,
    delay_ms: u16,
    width: u16,
    height: u16,
    truncated: bool,
}

impl Animation {
    /// Loads a file. See [`Animation::from_bytes`].
    pub fn from_file(
        path: &Path,
        screen: &ScreenSpec,
        options: LoadOptions,
    ) -> Result<Animation, Error> {
        let file = File::open(path).map_err(|error| Error::Image(error.to_string()))?;
        Animation::from_bytes(&read_limited(file, MAX_FILE_BYTES)?, screen, options)
    }

    /// Decodes a GIF, PNG, JPEG, BMP or WebP file and fits it to the screen. GIFs are animated, at
    /// the delay of their first frame. Anything else gives one frame.
    ///
    /// Decoding stops at the first frame past `screen.max_frames`. Without
    /// [`LoadOptions::truncate`] that's [`Error::TooManyFrames`], whose `got` is therefore
    /// `max_frames + 1` however long the file really is.
    pub fn from_bytes(
        bytes: &[u8],
        screen: &ScreenSpec,
        options: LoadOptions,
    ) -> Result<Animation, Error> {
        if options.delay_ms == Some(0) {
            return Err(Error::Invalid("a delay can't be 0".to_string()));
        }
        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| Error::Image(error.to_string()))?;
        match reader.format() {
            Some(ImageFormat::Gif) => decode_gif(bytes, screen, options, &GIF_LIMITS),
            Some(_) => decode_still(reader, screen, options),
            None => Err(Error::Image("not a known image format".to_string())),
        }
    }

    /// The frames, as RGB565.
    pub fn frames(&self) -> &[Vec<u8>] {
        &self.frames
    }

    /// Milliseconds per frame.
    pub fn delay_ms(&self) -> u16 {
        self.delay_ms
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Whether the source had frames that were left out.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Keeps the first `max` frames, and always at least one.
    pub fn truncate_to(&mut self, max: usize) {
        let max = max.max(1);
        self.truncated |= self.frames.len() > max;
        self.frames.truncate(max);
    }
}

/// How many reports one frame takes on a screen of this size. The last may be partly filled.
pub fn chunks_per_frame(width: u16, height: u16) -> usize {
    (usize::from(width) * usize::from(height) * 2).div_ceil(MAX_DATA)
}

fn image_error(error: ImageError) -> Error {
    Error::Image(error.to_string())
}

/// Reads all of `source`, unless it holds more than `limit` bytes.
fn read_limited(source: impl Read, limit: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    source
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::Image(error.to_string()))?;
    if bytes.len() as u64 > limit {
        return Err(Error::Image(format!(
            "the file is larger than {limit} bytes"
        )));
    }
    Ok(bytes)
}

fn decode_still(
    mut reader: ImageReader<Cursor<&[u8]>>,
    screen: &ScreenSpec,
    options: LoadOptions,
) -> Result<Animation, Error> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    let image = reader.decode().map_err(image_error)?.into_rgba8();

    Ok(Animation {
        frames: vec![prepare_frame(&image, screen, options.fit)?],
        delay_ms: options.delay_ms.unwrap_or(DEFAULT_DELAY_MS),
        width: screen.width,
        height: screen.height,
        truncated: false,
    })
}

fn decode_gif(
    bytes: &[u8],
    screen: &ScreenSpec,
    options: LoadOptions,
    gif_limits: &GifLimits,
) -> Result<Animation, Error> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(gif_limits.max_side);
    limits.max_image_height = Some(gif_limits.max_side);
    limits.max_alloc = Some(gif_limits.max_alloc);

    let mut decoder = GifDecoder::new(Cursor::new(bytes)).map_err(image_error)?;
    decoder.set_limits(limits).map_err(image_error)?;
    let (canvas_width, canvas_height) = decoder.dimensions();
    let canvas = u64::from(canvas_width) * u64::from(canvas_height);

    let max_frames = usize::from(screen.max_frames);
    let mut frames = Vec::new();
    let mut file_delay = None;
    let mut truncated = false;
    let mut pixels = 0;
    for frame in decoder.into_frames() {
        // A frame past the limit counts even if it's broken: nothing more is needed from the file.
        if frames.len() == max_frames {
            if !options.truncate {
                return Err(Error::TooManyFrames {
                    got: max_frames + 1,
                    max: max_frames,
                });
            }
            truncated = true;
            break;
        }
        pixels += canvas;
        if pixels > gif_limits.max_pixels {
            return Err(Error::Image(
                "the animation is too much work to decode".to_string(),
            ));
        }
        let frame = frame.map_err(image_error)?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        file_delay.get_or_insert(numerator / denominator.max(1));
        frames.push(prepare_frame(frame.buffer(), screen, options.fit)?);
    }
    let file_delay = file_delay.ok_or_else(|| Error::Image("no frames in the GIF".to_string()))?;

    Ok(Animation {
        frames,
        delay_ms: options.delay_ms.unwrap_or_else(|| gif_delay(file_delay)),
        width: screen.width,
        height: screen.height,
        truncated,
    })
}

/// A GIF's delay in the 16 bits of milliseconds there are for it.
fn gif_delay(milliseconds: u32) -> u16 {
    match milliseconds {
        0 => DEFAULT_DELAY_MS,
        other => u16::try_from(other).unwrap_or(u16::MAX),
    }
}

fn prepare_frame(image: &RgbaImage, screen: &ScreenSpec, mode: Fit) -> Result<Vec<u8>, Error> {
    if image.width() == 0 || image.height() == 0 {
        return Err(Error::Image("the image is empty".to_string()));
    }
    let fitted = fit(
        image,
        u32::from(screen.width),
        u32::from(screen.height),
        mode,
    );
    Ok(to_rgb565(&fitted))
}

/// Makes an image exactly `width` by `height`. Neither crop nor scale needs an intermediate image
/// bigger than the source and the result, however extreme the source's shape.
fn fit(image: &RgbaImage, width: u32, height: u32, mode: Fit) -> RgbaImage {
    match mode {
        Fit::Stretch => imageops::resize(image, width, height, RESIZE_FILTER),
        Fit::Cover => {
            let (x, y, crop_width, crop_height) = cover_crop(image.dimensions(), width, height);
            let cropped = imageops::crop_imm(image, x, y, crop_width, crop_height);
            imageops::resize(&*cropped, width, height, RESIZE_FILTER)
        }
        Fit::Contain => {
            let (inner_width, inner_height) = contain_size(image.dimensions(), width, height);
            let inner = imageops::resize(image, inner_width, inner_height, RESIZE_FILTER);
            let mut canvas = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255]));
            let x = i64::from((width - inner_width) / 2);
            let y = i64::from((height - inner_height) / 2);
            imageops::replace(&mut canvas, &inner, x, y);
            canvas
        }
    }
}

/// `a * b / c`, rounded, and at least 1.
fn scale(a: u32, b: u32, c: u32) -> u32 {
    let (a, b, c) = (u64::from(a), u64::from(b), u64::from(c));
    (((a * b + c / 2) / c).max(1)) as u32
}

/// The middle part of the source with the screen's shape: `(x, y, width, height)`.
fn cover_crop((source_w, source_h): (u32, u32), width: u32, height: u32) -> (u32, u32, u32, u32) {
    if u64::from(source_w) * u64::from(height) > u64::from(width) * u64::from(source_h) {
        let crop = scale(source_h, width, height).min(source_w);
        ((source_w - crop) / 2, 0, crop, source_h)
    } else {
        let crop = scale(source_w, height, width).min(source_h);
        (0, (source_h - crop) / 2, source_w, crop)
    }
}

/// The largest size with the source's shape that fits the screen.
fn contain_size((source_w, source_h): (u32, u32), width: u32, height: u32) -> (u32, u32) {
    if u64::from(source_w) * u64::from(height) > u64::from(width) * u64::from(source_h) {
        (width, scale(source_h, width, source_w).min(height))
    } else {
        (scale(source_w, height, source_h).min(width), height)
    }
}

/// Converts to the screen's pixel format. The screen can't show transparency, so what's transparent
/// lets black through.
fn to_rgb565(image: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.as_raw().len() / 2);
    for &Rgba([r, g, b, a]) in image.pixels() {
        let over_black = |channel: u8| (u16::from(channel) * u16::from(a) / 255) as u8;
        let (r, g, b) = (over_black(r), over_black(g), over_black(b));
        out.push((r & 0xF8) | (g >> 5));
        out.push(((g << 3) & 0xE0) | (b >> 3));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, ImageFormat, Rgba};
    use std::io::Cursor;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    const SIZES: [(u32, u32); 4] = [(320, 172), (240, 135), (240, 240), (128, 128)];
    const FITS: [Fit; 3] = [Fit::Cover, Fit::Contain, Fit::Stretch];

    fn spec(width: u16, height: u16, max_frames: u16) -> ScreenSpec {
        ScreenSpec {
            width,
            height,
            max_frames,
            kind: 1,
            crc: false,
            draw_frame_time: 0,
        }
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    /// Top left red, top right green, bottom left blue, bottom right white.
    fn quadrants(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba(match (x < width / 2, y < height / 2) {
                (true, true) => RED,
                (false, true) => GREEN,
                (true, false) => BLUE,
                (false, false) => WHITE,
            })
        })
    }

    fn encode(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        if format == ImageFormat::Jpeg {
            image::DynamicImage::ImageRgba8(image.clone())
                .into_rgb8()
                .write_to(&mut Cursor::new(&mut bytes), format)
                .unwrap();
        } else {
            image
                .write_to(&mut Cursor::new(&mut bytes), format)
                .unwrap();
        }
        bytes
    }

    /// A GIF with one solid colour per frame, all showing for `delay_ms`.
    fn gif(width: u32, height: u32, colors: &[[u8; 4]], delay_ms: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            for &color in colors {
                let delay = Delay::from_numer_denom_ms(delay_ms, 1);
                let frame = Frame::from_parts(solid(width, height, color), 0, 0, delay);
                encoder.encode_frame(frame).unwrap();
            }
        }
        bytes
    }

    fn colors_of(count: usize) -> Vec<[u8; 4]> {
        (0..count).map(|i| [i as u8, 0, 0, 255]).collect()
    }

    fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 2] {
        let at = ((y * width + x) * 2) as usize;
        [frame[at], frame[at + 1]]
    }

    // RGB565.

    #[test]
    fn rgb565_is_big_endian_for_pure_colours() {
        assert_eq!(to_rgb565(&solid(1, 1, RED)), [0xF8, 0x00]);
        assert_eq!(to_rgb565(&solid(1, 1, GREEN)), [0x07, 0xE0]);
        assert_eq!(to_rgb565(&solid(1, 1, BLUE)), [0x00, 0x1F]);
        assert_eq!(to_rgb565(&solid(1, 1, WHITE)), [0xFF, 0xFF]);
        assert_eq!(to_rgb565(&solid(1, 1, BLACK)), [0x00, 0x00]);
    }

    #[test]
    fn rgb565_keeps_the_top_bits_of_each_channel() {
        assert_eq!(
            to_rgb565(&solid(1, 1, [0b0000_1111, 0b0000_0111, 0b0000_1111, 255])),
            [0x08, 0x21]
        );
    }

    #[test]
    fn rgb565_runs_left_to_right_from_the_top() {
        let image = RgbaImage::from_fn(2, 2, |x, y| {
            Rgba(match (x, y) {
                (0, 0) => RED,
                (1, 0) => GREEN,
                (0, 1) => BLUE,
                _ => WHITE,
            })
        });
        assert_eq!(
            to_rgb565(&image),
            [0xF8, 0x00, 0x07, 0xE0, 0x00, 0x1F, 0xFF, 0xFF]
        );
    }

    #[test]
    fn transparency_shows_black() {
        assert_eq!(to_rgb565(&solid(1, 1, [255, 255, 255, 0])), [0x00, 0x00]);
        // White at half strength: each channel is 127.
        assert_eq!(to_rgb565(&solid(1, 1, [255, 255, 255, 127])), [0x7B, 0xEF]);
    }

    // Fitting.

    #[test]
    fn every_fit_gives_exactly_the_screen_size() {
        for (width, height) in SIZES {
            for mode in FITS {
                for (from_w, from_h) in [(64, 48), (48, 64), (1000, 10), (10, 1000), (1, 1)] {
                    let out = fit(&quadrants(from_w, from_h), width, height, mode);
                    assert_eq!(
                        out.dimensions(),
                        (width, height),
                        "{mode:?} {from_w}x{from_h}"
                    );
                }
            }
        }
    }

    /// The colour near each corner of a rectangle.
    fn corners(image: &RgbaImage, x: u32, y: u32, width: u32, height: u32) -> [[u8; 4]; 4] {
        let inset = 3;
        let (right, bottom) = (x + width - 1 - inset, y + height - 1 - inset);
        [
            image.get_pixel(x + inset, y + inset).0,
            image.get_pixel(right, y + inset).0,
            image.get_pixel(x + inset, bottom).0,
            image.get_pixel(right, bottom).0,
        ]
    }

    #[test]
    fn cover_and_stretch_keep_the_corners_in_place() {
        for (width, height) in SIZES {
            for mode in [Fit::Cover, Fit::Stretch] {
                let out = fit(&quadrants(64, 48), width, height, mode);
                assert_eq!(
                    corners(&out, 0, 0, width, height),
                    [RED, GREEN, BLUE, WHITE],
                    "{mode:?} {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn contain_keeps_the_corners_and_adds_black_bars() {
        // Wider than the square screen: 240x180 with bars of 30 above and below.
        let out = fit(&quadrants(64, 48), 240, 240, Fit::Contain);
        assert_eq!(corners(&out, 0, 30, 240, 180), [RED, GREEN, BLUE, WHITE]);
        assert_eq!(corners(&out, 0, 0, 240, 28), [BLACK; 4]);
        assert_eq!(corners(&out, 0, 212, 240, 28), [BLACK; 4]);

        // Taller than the wide screen: 129x172 with bars of 95 or 96 left and right.
        let out = fit(&quadrants(48, 64), 320, 172, Fit::Contain);
        assert_eq!(corners(&out, 95, 0, 130, 172), [RED, GREEN, BLUE, WHITE]);
        assert_eq!(corners(&out, 0, 0, 90, 172), [BLACK; 4]);
        assert_eq!(corners(&out, 230, 0, 90, 172), [BLACK; 4]);
    }

    #[test]
    fn cover_crops_the_long_side_equally() {
        // A 4:3 image on a square screen loses the sides: a red strip on the far left is cut off.
        let image = RgbaImage::from_fn(400, 300, |x, _| Rgba(if x < 40 { RED } else { GREEN }));
        let out = fit(&image, 240, 240, Fit::Cover);
        assert_eq!(out.get_pixel(0, 120).0, GREEN);
    }

    #[test]
    fn cover_of_an_extreme_shape_uses_no_big_intermediate_image() {
        // Scaling this up to cover the screen first would need gigabytes.
        let out = fit(&solid(16000, 1, RED), 320, 172, Fit::Cover);
        assert_eq!(out.dimensions(), (320, 172));
        assert_eq!(out.get_pixel(160, 86).0, RED);
    }

    // Chunks.

    #[test]
    fn chunks_per_frame_follow_the_screen_size() {
        assert_eq!(chunks_per_frame(320, 172), 215);
        assert_eq!(chunks_per_frame(240, 135), 127);
        assert_eq!(chunks_per_frame(240, 240), 225);
        assert_eq!(chunks_per_frame(128, 128), 64);
    }

    // Loading GIFs.

    #[test]
    fn a_gif_gives_its_frames_and_delay() {
        let bytes = gif(10, 10, &[RED, BLUE], 200);
        let anim = Animation::from_bytes(&bytes, &spec(4, 2, 60), LoadOptions::default()).unwrap();
        assert_eq!(anim.frames().len(), 2);
        assert_eq!(anim.delay_ms(), 200);
        assert_eq!((anim.width(), anim.height()), (4, 2));
        assert!(!anim.truncated());
        assert_eq!(anim.frames()[0], [0xF8, 0x00].repeat(8));
        assert_eq!(anim.frames()[1], [0x00, 0x1F].repeat(8));
    }

    #[test]
    fn a_delay_given_by_the_caller_wins() {
        let bytes = gif(10, 10, &[RED, BLUE], 200);
        let options = LoadOptions {
            delay_ms: Some(70),
            ..LoadOptions::default()
        };
        let anim = Animation::from_bytes(&bytes, &spec(4, 2, 60), options).unwrap();
        assert_eq!(anim.delay_ms(), 70);
    }

    #[test]
    fn a_delay_of_0_from_the_caller_is_refused() {
        let bytes = gif(10, 10, &[RED], 200);
        let options = LoadOptions {
            delay_ms: Some(0),
            ..LoadOptions::default()
        };
        let result = Animation::from_bytes(&bytes, &spec(4, 2, 60), options);
        assert!(matches!(result, Err(Error::Invalid(_))));
    }

    #[test]
    fn a_delay_of_0_in_a_gif_means_100_ms() {
        let bytes = gif(10, 10, &[RED, BLUE], 0);
        let anim = Animation::from_bytes(&bytes, &spec(4, 2, 60), LoadOptions::default()).unwrap();
        assert_eq!(anim.delay_ms(), 100);
    }

    #[test]
    fn a_long_delay_in_a_gif_is_capped() {
        // The longest a GIF can say is 655.35 s, which doesn't fit the 16 bits of milliseconds.
        let bytes = gif(10, 10, &[RED], 655_350);
        let anim = Animation::from_bytes(&bytes, &spec(4, 2, 60), LoadOptions::default()).unwrap();
        assert_eq!(anim.delay_ms(), u16::MAX);
    }

    #[test]
    fn the_fit_is_applied_to_gif_frames() {
        let bytes = gif(4, 4, &[RED], 100);
        let options = LoadOptions {
            fit: Fit::Contain,
            ..LoadOptions::default()
        };
        let anim = Animation::from_bytes(&bytes, &spec(8, 4, 60), options).unwrap();
        let frame = &anim.frames()[0];
        assert_eq!(pixel(frame, 8, 0, 0), [0, 0]);
        assert_eq!(pixel(frame, 8, 4, 2), [0xF8, 0x00]);
        assert_eq!(pixel(frame, 8, 7, 3), [0, 0]);
    }

    #[test]
    fn as_many_frames_as_the_limit_are_fine() {
        let bytes = gif(2, 2, &colors_of(3), 100);
        let anim = Animation::from_bytes(&bytes, &spec(2, 2, 3), LoadOptions::default()).unwrap();
        assert_eq!(anim.frames().len(), 3);
    }

    #[test]
    fn more_frames_than_the_limit_are_refused() {
        let bytes = gif(2, 2, &colors_of(10), 100);
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 3), LoadOptions::default());
        // Decoding stopped at the fourth frame, so that's the count it can report.
        assert_eq!(result.err(), Some(Error::TooManyFrames { got: 4, max: 3 }));
    }

    #[test]
    fn truncating_keeps_the_first_frames() {
        let bytes = gif(2, 2, &[RED, GREEN, BLUE, WHITE], 100);
        let options = LoadOptions {
            truncate: true,
            ..LoadOptions::default()
        };
        let anim = Animation::from_bytes(&bytes, &spec(2, 2, 2), options).unwrap();
        assert_eq!(anim.frames().len(), 2);
        assert!(anim.truncated());
        assert_eq!(anim.frames()[0], [0xF8, 0x00].repeat(4));
        assert_eq!(anim.frames()[1], [0x07, 0xE0].repeat(4));
    }

    #[test]
    fn truncating_a_gif_within_the_limit_changes_nothing() {
        let bytes = gif(2, 2, &[RED, GREEN], 100);
        let options = LoadOptions {
            truncate: true,
            ..LoadOptions::default()
        };
        let anim = Animation::from_bytes(&bytes, &spec(2, 2, 2), options).unwrap();
        assert_eq!(anim.frames().len(), 2);
        assert!(!anim.truncated());
    }

    #[test]
    fn truncate_to_keeps_the_first_frames_and_at_least_one() {
        let bytes = gif(2, 2, &[RED, GREEN, BLUE], 100);
        let load = LoadOptions::default();
        let mut anim = Animation::from_bytes(&bytes, &spec(2, 2, 60), load).unwrap();
        anim.truncate_to(5);
        assert_eq!(anim.frames().len(), 3);
        assert!(!anim.truncated());
        anim.truncate_to(2);
        assert_eq!(anim.frames().len(), 2);
        assert_eq!(anim.frames()[1], [0x07, 0xE0].repeat(4));
        assert!(anim.truncated());
        anim.truncate_to(0);
        assert_eq!(anim.frames().len(), 1);
    }

    // Hostile files.

    /// Cuts the last frame of a GIF in half, so that decoding it fails.
    fn with_corrupt_last_frame(colors: &[[u8; 4]]) -> Vec<u8> {
        let mut bytes = gif(16, 16, colors, 100);
        let last_frame = bytes.iter().rposition(|&b| b == 0x2C).unwrap();
        bytes.truncate(last_frame + 12);
        let frames = image::codecs::gif::GifDecoder::new(Cursor::new(&bytes))
            .unwrap()
            .into_frames()
            .collect_frames();
        assert!(frames.is_err(), "this test needs the end to be corrupt");
        bytes
    }

    #[test]
    fn decoding_stops_past_the_limit_without_reading_the_rest() {
        // 5 frames and a limit of 3: if it looked at the fifth, this would be an image error.
        let colors = colors_of(5);
        let bytes = with_corrupt_last_frame(&colors);
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 3), LoadOptions::default());
        assert_eq!(result.err(), Some(Error::TooManyFrames { got: 4, max: 3 }));

        let options = LoadOptions {
            truncate: true,
            ..LoadOptions::default()
        };
        let anim = Animation::from_bytes(&bytes, &spec(2, 2, 3), options).unwrap();
        assert_eq!(anim.frames().len(), 3);
    }

    #[test]
    fn a_broken_frame_within_the_limit_is_an_image_error() {
        let bytes = with_corrupt_last_frame(&colors_of(3));
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 60), LoadOptions::default());
        assert!(matches!(result, Err(Error::Image(_))));
    }

    #[test]
    fn a_gif_with_a_huge_canvas_is_refused_before_any_allocation() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend([0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0x3B]);
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 60), LoadOptions::default());
        assert!(matches!(result, Err(Error::Image(_))), "{result:?}");
    }

    #[test]
    fn a_small_gif_with_a_huge_frame_is_refused() {
        let mut bytes = b"GIF89a".to_vec();
        // A 1x1 canvas with no colour table.
        bytes.extend([1, 0, 1, 0, 0, 0, 0]);
        // An image of 65535x65535 at 0,0 with a 2-colour local table, and hardly any data.
        bytes.extend([0x2C, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0x80]);
        bytes.extend([0, 0, 0, 255, 255, 255]);
        bytes.extend([2, 2, 0x44, 0x01, 0, 0x3B]);
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 60), LoadOptions::default());
        assert!(matches!(result, Err(Error::Image(_))), "{result:?}");
    }

    #[test]
    fn a_gif_with_no_frames_is_an_image_error() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend([1, 0, 1, 0, 0, 0, 0, 0x3B]);
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 60), LoadOptions::default());
        assert!(matches!(result, Err(Error::Image(_))), "{result:?}");
    }

    #[test]
    fn a_gif_with_an_empty_canvas_is_an_image_error() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend([0, 0, 0, 0, 0, 0, 0]);
        bytes.extend([0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0x80, 0, 0, 0, 255, 255, 255]);
        bytes.extend([2, 2, 0x44, 0x01, 0, 0x3B]);
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 60), LoadOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn the_total_decoding_work_is_limited() {
        let bytes = gif(2, 2, &colors_of(3), 100);
        let limits = GifLimits {
            max_pixels: 8,
            ..GIF_LIMITS
        };
        let screen = spec(2, 2, 60);
        let load = LoadOptions::default();
        let result = decode_gif(&bytes, &screen, load, &limits);
        assert!(matches!(result, Err(Error::Image(_))), "{result:?}");

        let limits = GifLimits {
            max_pixels: 12,
            ..GIF_LIMITS
        };
        assert!(decode_gif(&bytes, &screen, load, &limits).is_ok());
    }

    #[test]
    fn a_bmp_with_huge_dimensions_is_refused_before_any_allocation() {
        let mut bytes = b"BM".to_vec();
        bytes.extend(1000u32.to_le_bytes());
        bytes.extend([0; 4]);
        bytes.extend(54u32.to_le_bytes());
        bytes.extend(40u32.to_le_bytes());
        bytes.extend(30000i32.to_le_bytes());
        bytes.extend(30000i32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(24u16.to_le_bytes());
        bytes.extend([0; 24]);
        let result = Animation::from_bytes(&bytes, &spec(2, 2, 60), LoadOptions::default());
        assert!(matches!(result, Err(Error::Image(_))), "{result:?}");
    }

    #[test]
    fn garbage_is_an_image_error() {
        for bytes in [&b""[..], b"not an image", b"GIF89a", b"\x89PNG\r\n\x1a\n"] {
            let result = Animation::from_bytes(bytes, &spec(2, 2, 60), LoadOptions::default());
            assert!(matches!(result, Err(Error::Image(_))), "{bytes:?}");
        }
    }

    #[test]
    fn reading_stops_past_the_size_limit() {
        let data = [7; 10];
        assert_eq!(read_limited(&data[..], 10).unwrap(), data);
        assert!(matches!(read_limited(&data[..], 9), Err(Error::Image(_))));
    }

    #[test]
    fn a_missing_file_is_an_image_error() {
        let result = Animation::from_file(
            std::path::Path::new("this file does not exist.gif"),
            &spec(2, 2, 60),
            LoadOptions::default(),
        );
        assert!(matches!(result, Err(Error::Image(_))));
    }

    #[test]
    fn a_file_loads_like_its_bytes() {
        let path = std::env::temp_dir().join(format!("unkludge-test-{}.gif", std::process::id()));
        std::fs::write(&path, gif(2, 2, &[RED, BLUE], 100)).unwrap();
        let result = Animation::from_file(&path, &spec(2, 2, 60), LoadOptions::default());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(result.unwrap().frames().len(), 2);
    }

    // Still images.

    #[test]
    fn still_images_are_one_frame() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Bmp,
            ImageFormat::WebP,
        ] {
            let bytes = encode(&solid(8, 8, RED), format);
            let anim =
                Animation::from_bytes(&bytes, &spec(4, 2, 60), LoadOptions::default()).unwrap();
            assert_eq!(anim.frames().len(), 1, "{format:?}");
            assert_eq!(anim.frames()[0].len(), 4 * 2 * 2);
            assert_eq!(anim.delay_ms(), 100, "{format:?}");
        }
    }

    #[test]
    fn a_still_image_keeps_its_pixels_and_takes_a_delay() {
        let bytes = encode(&quadrants(8, 8), ImageFormat::Png);
        let options = LoadOptions {
            delay_ms: Some(250),
            ..LoadOptions::default()
        };
        let anim = Animation::from_bytes(&bytes, &spec(8, 8, 60), options).unwrap();
        assert_eq!(anim.delay_ms(), 250);
        let frame = &anim.frames()[0];
        assert_eq!(pixel(frame, 8, 0, 0), [0xF8, 0x00]);
        assert_eq!(pixel(frame, 8, 7, 0), [0x07, 0xE0]);
        assert_eq!(pixel(frame, 8, 0, 7), [0x00, 0x1F]);
        assert_eq!(pixel(frame, 8, 7, 7), [0xFF, 0xFF]);
    }

    #[test]
    fn every_supported_model_gets_frames_of_its_size() {
        let bytes = encode(&quadrants(64, 48), ImageFormat::Png);
        for model in crate::model::supported() {
            let screen = model.screen.unwrap();
            let anim = Animation::from_bytes(&bytes, &screen, LoadOptions::default()).unwrap();
            let expected = usize::from(screen.width) * usize::from(screen.height) * 2;
            assert_eq!(anim.frames()[0].len(), expected, "{:04X}", model.pid);
        }
    }
}
