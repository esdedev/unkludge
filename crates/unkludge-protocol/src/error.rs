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
    #[error("the keyboard didn't acknowledge command {cmd:#04x}, chunk {idx}")]
    NoAck { cmd: u8, idx: u8 },
    #[error("the keyboard refused command {cmd:#04x}, chunk {idx}")]
    Rejected { cmd: u8, idx: u8 },
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
    #[error("image: {0}")]
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
            Error::NoAck { cmd: 0x0C, idx: 7 }.to_string(),
            "the keyboard didn't acknowledge command 0x0c, chunk 7"
        );
        assert_eq!(
            Error::Rejected { cmd: 0x0C, idx: 7 }.to_string(),
            "the keyboard refused command 0x0c, chunk 7"
        );
        assert_eq!(
            Error::TooManyFrames { got: 61, max: 60 }.to_string(),
            "too many frames: at least 61, this keyboard's limit is 60"
        );
    }
}
