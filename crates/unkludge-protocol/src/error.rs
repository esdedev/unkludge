//! Everything that can go wrong talking to a keyboard.

/// Errors from this crate.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("no supported keyboard found")]
    NotFound,
    #[error("keyboard {0:04X} isn't supported")]
    Unsupported(u16),
    #[error("USB error: {0}")]
    Hid(String),
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
    /// `got` may be less than the whole count, as decoding stops soon after the limit.
    #[error("too many frames: at least {got}, this keyboard's limit is {max}")]
    TooManyFrames { got: usize, max: usize },
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
            Error::TooManyFrames { got: 61, max: 60 }.to_string(),
            "too many frames: at least 61, this keyboard's limit is 60"
        );
    }
}
