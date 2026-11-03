//! Protocol library for Royal Kludge keyboards.

mod error;
pub mod model;
// The crate-private packet builders are used by the send paths and feature modules.
#[allow(dead_code)]
pub mod packet;

pub use error::Error;
