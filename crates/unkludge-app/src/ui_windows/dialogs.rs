//! The system's dialogs: choosing a file, choosing a colour and the first custom colour's warning,
//! and the file from a drag and drop.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use unkludge_protocol::lighting::Rgb;
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragFinish, DragQueryFileW, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FileOpenDialog, HDROP,
    IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
use windows::core::{HSTRING, PCWSTR, Result, w};

use crate::i18n::t;

/// Asks for a file. `None` if the user changes their mind.
pub fn choose_image(owner: HWND) -> Option<PathBuf> {
    match open_dialog(owner) {
        Ok(path) => path,
        Err(error) => {
            crate::log::error(&format!("the file dialog can't be shown: {error}"));
            None
        }
    }
}

fn open_dialog(owner: HWND) -> Result<Option<PathBuf>> {
    let title = HSTRING::from(t().choose_title);
    let filter_name = HSTRING::from(t().image_files);
    let filters = [COMDLG_FILTERSPEC {
        pszName: PCWSTR(filter_name.as_ptr()),
        pszSpec: w!("*.gif;*.png;*.jpg;*.jpeg;*.bmp;*.webp"),
    }];
    // SAFETY: COM is set up on this thread by `run`, and the strings outlive these calls.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetTitle(&title)?;
        dialog.SetFileTypes(&filters)?;
        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)?;
        // Cancelling comes back as an error too.
        if dialog.Show(Some(owner)).is_err() {
            return Ok(None);
        }
        let item = dialog.GetResult()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = OsString::from_wide(name.as_wide());
        CoTaskMemFree(Some(name.0.cast()));
        Ok(Some(PathBuf::from(path)))
    }
}

/// Asks for a colour, starting from this one.
pub fn choose_color(owner: HWND, initial: Rgb) -> Option<Rgb> {
    let mut custom = [COLORREF(0x00FF_FFFF); 16];
    let mut choice = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: COLORREF(colorref(initial)),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    // SAFETY: the structure and the custom colours live through the call, which is all that uses them.
    let chosen = unsafe { ChooseColorW(&mut choice) };
    chosen.as_bool().then(|| rgb_of(choice.rgbResult.0))
}

/// A colour as the system has them: 0x00BBGGRR.
fn colorref(color: Rgb) -> u32 {
    u32::from(color.b) << 16 | u32::from(color.g) << 8 | u32::from(color.r)
}

fn rgb_of(value: u32) -> Rgb {
    Rgb::new(value as u8, (value >> 8) as u8, (value >> 16) as u8)
}

/// The warning for a model's first custom colour. Says whether to go on.
pub fn warn_first_custom_color(owner: HWND) -> bool {
    let text = HSTRING::from(t().first_custom_warning);
    let title = HSTRING::from(t().first_custom_title);
    // SAFETY: the strings outlive the call.
    let answer = unsafe { MessageBoxW(Some(owner), &text, &title, MB_OKCANCEL | MB_ICONWARNING) };
    answer == IDOK
}

/// The first file of a drop, which is then finished with.
pub fn dropped_file(drop: HDROP) -> Option<PathBuf> {
    // SAFETY: `drop` is the system's for this message, and is only let go at the end. The first call
    // gives the length without the NUL, and the buffer has room for both.
    unsafe {
        let len = DragQueryFileW(drop, 0, None) as usize;
        let path = (len > 0).then(|| {
            let mut buffer = vec![0u16; len + 1];
            let written = DragQueryFileW(drop, 0, Some(&mut buffer)) as usize;
            PathBuf::from(OsString::from_wide(&buffer[..written.min(len)]))
        });
        DragFinish(drop);
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::UI::Shell::DROPFILES;

    /// What the system hands over for a drop: a header, then wide names, each NUL-terminated, with
    /// another NUL at the end.
    fn drop_of(names: &[&str]) -> HDROP {
        let mut wide: Vec<u16> = Vec::new();
        for name in names {
            wide.extend(name.encode_utf16());
            wide.push(0);
        }
        wide.push(0);
        let header = size_of::<DROPFILES>();
        // SAFETY: the block is large enough for what's written, and is the drop's from then on.
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, header + wide.len() * 2).unwrap();
            let start = GlobalLock(memory);
            start.cast::<DROPFILES>().write(DROPFILES {
                pFiles: header as u32,
                fWide: true.into(),
                ..Default::default()
            });
            let names_at = start.cast::<u8>().add(header).cast::<u16>();
            std::ptr::copy_nonoverlapping(wide.as_ptr(), names_at, wide.len());
            let _ = GlobalUnlock(memory);
            HDROP(memory.0)
        }
    }

    #[test]
    fn the_first_file_of_a_drop_is_the_one_taken() {
        let path = dropped_file(drop_of(&[r"C:\pics\gato ñ.gif", r"C:\pics\other.png"]));
        assert_eq!(path, Some(PathBuf::from(r"C:\pics\gato ñ.gif")));
    }

    #[test]
    fn a_drop_with_no_files_gives_nothing() {
        assert_eq!(dropped_file(drop_of(&[])), None);
    }

    #[test]
    fn colours_round_trip_through_the_systems_order() {
        let color = Rgb::new(0x12, 0x34, 0x56);
        assert_eq!(colorref(color), 0x0056_3412);
        assert_eq!(rgb_of(colorref(color)), color);
    }
}
