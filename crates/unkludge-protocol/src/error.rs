//! Everything that can go wrong talking to a keyboard.

/// Errors from this crate.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("no supported keyboard found")]
    NotFound,
    #[error("keyboard {0:04X} isn't supported")]
    Unsupported(u16),
    /// A supported keyboard is there, and none of its HID collections is one Unkludge can use.
    #[error(
        "{} is connected, but not with the HID collections Unkludge can use. `unkludge report` lists \
         them",
        keyboard_name(*.0)
    )]
    NoUsableCollection(u16),
    #[error("USB error: {0}")]
    Hid(String),
    /// The system says this keyboard is on another bus than USB. Nothing is sent to it.
    #[error(
        "{} isn't connected by USB, and Unkludge only works by USB cable. Connect it by cable",
        keyboard_name(*.0)
    )]
    NotOnUsb(u16),
    /// Another Unkludge program has the keyboard. See `unkludge_shared::device_lock`.
    #[error(
        "another Unkludge program is using the keyboard. If the Unkludge app is running, quit it first \
         (right-click its tray icon, Quit). If a unkludge command is running, wait for it to end"
    )]
    DeviceBusy,
    /// Nothing can tell whether another Unkludge program has the keyboard, so it stays closed.
    #[error("the lock that keeps Unkludge programs off the keyboard at the same time failed: {0}")]
    LockFailed(String),
    #[error("the keyboard didn't acknowledge the start of the upload")]
    StartNoAck,
    #[error("the keyboard refused the start of the upload")]
    StartRejected,
    /// A chunk of an upload. An abort follows, as far as the keyboard still takes one.
    #[error("the keyboard didn't acknowledge chunk {idx} of frame {frame}. An abort was sent")]
    NoAck { frame: u8, idx: u8 },
    /// A chunk of an upload. An abort follows, as far as the keyboard still takes one.
    #[error("the keyboard refused chunk {idx} of frame {frame}. An abort was sent")]
    Rejected { frame: u8, idx: u8 },
    #[error("{0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Image(String),
    /// A setting of the current effect was changed with no effect showing.
    #[error("the lights are off: choose an effect first")]
    LightsOff,
    /// A setting of the current effect was changed in the custom mode, which this version can't
    /// change, only leave.
    #[error("the keyboard is in the custom mode: choose an effect first")]
    CustomMode,
    /// `got` may be less than the whole count, as decoding stops soon after the limit.
    #[error("too many frames: at least {got}, this keyboard's limit is {max}")]
    TooManyFrames { got: usize, max: usize },
}

/// A keyboard's name and product ID, or the ID alone if it isn't in the table.
fn keyboard_name(pid: u16) -> String {
    match crate::model::find(pid) {
        Some(model) => format!("{} ({pid:04X})", model.name),
        None => format!("Keyboard {pid:04X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_carry_the_details() {
        assert_eq!(
            Error::Unsupported(0x01FA).to_string(),
            "keyboard 01FA isn't supported"
        );
        assert_eq!(
            Error::NoUsableCollection(0x0265).to_string(),
            "RK S85 ES (0265) is connected, but not with the HID collections Unkludge can use. \
             `unkludge report` lists them"
        );
        assert!(
            Error::NoUsableCollection(0xFFFE)
                .to_string()
                .starts_with("Keyboard FFFE is connected")
        );
        assert_eq!(
            Error::NotOnUsb(0x0265).to_string(),
            "RK S85 ES (0265) isn't connected by USB, and Unkludge only works by USB cable. Connect \
             it by cable"
        );
        let busy = Error::DeviceBusy.to_string();
        assert!(busy.contains("quit it first"), "{busy}");
        assert_eq!(
            Error::LockFailed("no folder".to_string()).to_string(),
            "the lock that keeps Unkludge programs off the keyboard at the same time failed: no folder"
        );
        assert_eq!(
            Error::NoAck { frame: 2, idx: 7 }.to_string(),
            "the keyboard didn't acknowledge chunk 7 of frame 2. An abort was sent"
        );
        assert_eq!(
            Error::Rejected { frame: 2, idx: 7 }.to_string(),
            "the keyboard refused chunk 7 of frame 2. An abort was sent"
        );
        assert_eq!(
            Error::StartNoAck.to_string(),
            "the keyboard didn't acknowledge the start of the upload"
        );
        assert_eq!(
            Error::StartRejected.to_string(),
            "the keyboard refused the start of the upload"
        );
        assert_eq!(
            Error::Invalid("brightness 5 isn't one of [0, 4]".to_string()).to_string(),
            "brightness 5 isn't one of [0, 4]"
        );
        assert_eq!(
            Error::Image("not a known image format".to_string()).to_string(),
            "not a known image format"
        );
        assert_eq!(
            Error::LightsOff.to_string(),
            "the lights are off: choose an effect first"
        );
        assert_eq!(
            Error::CustomMode.to_string(),
            "the keyboard is in the custom mode: choose an effect first"
        );
        assert_eq!(
            Error::TooManyFrames { got: 61, max: 60 }.to_string(),
            "too many frames: at least 61, this keyboard's limit is 60"
        );
    }
}
