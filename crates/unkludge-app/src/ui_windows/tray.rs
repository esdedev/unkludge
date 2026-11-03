//! The tray icon and its menu, and the icon as the system wants it.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyMenu, GetCursorPos, HICON, ICONINFO,
    MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostMessageW, RegisterWindowMessageW,
    SM_CXICON, SM_CXSMICON, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    WM_APP, WM_NULL,
};
use windows::core::{HSTRING, w};

use super::App;
use crate::i18n::t;

pub const WM_TRAY: u32 = WM_APP + 1;

const MENU_OPEN: usize = 1;
const MENU_SYNC_CLOCK: usize = 2;
const MENU_AUTOSTART: usize = 3;
const MENU_QUIT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    None,
    Open,
    SyncClock,
    Autostart,
    Quit,
}

/// Explorer sends this when it makes the taskbar, at sign-in or on restarting. The new one doesn't
/// know the icons there were, and if the app started first it never got its own.
pub fn wm_taskbar_created() -> u32 {
    static MESSAGE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    // SAFETY: the string is NUL-terminated and static.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// The size the system wants small icons, such as the tray's, at its scale.
pub fn small_icon_size() -> i32 {
    // SAFETY: plain numbers.
    unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }
}

/// The size it wants large ones, such as the window's.
pub fn large_icon_size() -> i32 {
    // SAFETY: plain numbers.
    unsafe { GetSystemMetricsForDpi(SM_CXICON, GetDpiForSystem()) }
}

/// The app's icon as the system wants it. Falls back to no icon rather than to no app.
pub fn make_icon(size: i32) -> HICON {
    let bitmap = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size, // Top row first.
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: the pointers are valid for the calls, the copy stays within the section of `size` by
    // `size` pixels of 4 bytes, and the bitmaps can go once the icon has been made from them.
    unsafe {
        let Ok(color) = CreateDIBSection(None, &bitmap, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HICON::default();
        };
        let pixels = crate::icon::pixels(size as usize);
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        let mask_bits = vec![0u8; (((size + 15) / 16) * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

impl App {
    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.tray_icon,
            ..Default::default()
        };
        let tip = if self.is_demo() {
            format!("Unkludge - {}", t().demo)
        } else {
            "Unkludge".to_string()
        };
        for (slot, unit) in data.szTip.iter_mut().zip(tip.encode_utf16()) {
            *slot = unit;
        }
        data
    }

    pub fn add_tray(&self) {
        // SAFETY: the structure is valid for the call, which copies what it needs.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.tray_data());
        }
    }

    /// A balloon from the tray icon, the one time it's needed.
    pub fn show_tray_note(&self) {
        let mut data = self.tray_data();
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        for (slot, unit) in data
            .szInfoTitle
            .iter_mut()
            .zip(t().tray_note_title.encode_utf16())
        {
            *slot = unit;
        }
        for (slot, unit) in data.szInfo.iter_mut().zip(t().tray_note.encode_utf16()) {
            *slot = unit;
        }
        // SAFETY: as in `add_tray`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    pub fn remove_tray(&self) {
        // SAFETY: as in `add_tray`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
        }
    }
}

/// Shows the tray's menu and waits for a choice. It has a message loop of its own.
pub fn menu(hwnd: HWND, autostart: bool) -> MenuChoice {
    // SAFETY: the handles are valid for these calls, and the strings outlive them.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return MenuChoice::None;
        };
        let open = HSTRING::from(t().tray_open);
        let sync = HSTRING::from(t().tray_sync_clock);
        let start = HSTRING::from(t().start_with_system);
        let quit = HSTRING::from(t().tray_quit);
        let checked = if autostart { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, &open);
        let _ = AppendMenuW(menu, MF_STRING, MENU_SYNC_CLOCK, &sync);
        let _ = AppendMenuW(menu, MF_STRING | checked, MENU_AUTOSTART, &start);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, &quit);
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // Without this, clicking elsewhere doesn't close the menu.
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        );
        // As the system's documentation asks, so that the menu goes away properly the next time.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match id.0 as usize {
            MENU_OPEN => MenuChoice::Open,
            MENU_SYNC_CLOCK => MenuChoice::SyncClock,
            MENU_AUTOSTART => MenuChoice::Autostart,
            MENU_QUIT => MenuChoice::Quit,
            _ => MenuChoice::None,
        }
    }
}
