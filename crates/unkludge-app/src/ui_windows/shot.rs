//! The demo's pictures: a PNG of what the window draws inside, drawn again into a bitmap of its own.
//! It never looks at the desktop, so it can't have anything else in it, it works with the session
//! locked, and the window needn't even show.

use std::io;
use std::path::Path;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use super::App;
use super::gfx::Gfx;
use crate::winres;

impl App {
    /// Writes what the window has inside to a PNG file.
    pub fn save_shot(&mut self, path: &Path) -> io::Result<()> {
        let (width, height, mut pixels) = self.capture()?;
        // Nothing here has any alpha to speak of.
        let (bgra, _) = pixels.as_chunks_mut::<4>();
        for pixel in bgra {
            pixel[3] = 255;
        }
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(path, winres::png(width, height, &pixels))
    }

    fn capture(&mut self) -> io::Result<(usize, usize, Vec<u8>)> {
        let failed = |what: &str| io::Error::other(format!("{what} failed"));
        let mut client = RECT::default();
        // SAFETY: the rectangle is written by the call.
        unsafe { GetClientRect(self.hwnd, &mut client) }
            .map_err(|_| failed("reading the window's size"))?;
        let (width, height) = (client.right, client.bottom);
        if width <= 0 || height <= 0 {
            return Err(failed("reading the window's size"));
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // Top row first.
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the usual steps for drawing into a bitmap, each object going away after what
        // depends on it. The copy reads exactly the bitmap's pixels, while it's still there.
        unsafe {
            let target = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(target), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                .map_err(|_| failed("making the bitmap"))?;
            let previous = SelectObject(target, bitmap.into());
            let drawn = Gfx::for_dc(&self.d2d, target, &client)
                .map_err(|error| io::Error::other(error.to_string()))
                .map(|gfx| self.paint_to(gfx));
            let bytes = width as usize * height as usize * 4;
            let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), bytes).to_vec();
            SelectObject(target, previous);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(target);
            drawn?;
            Ok((width as usize, height as usize, pixels))
        }
    }
}
