//! Protocol library for Royal Kludge keyboards.

// The crate-private send paths and packet builders are for the feature modules.
#[allow(dead_code)]
pub mod device;
mod error;
pub mod model;
#[allow(dead_code)]
pub mod packet;
pub mod transport;

pub use device::Device;
pub use error::Error;
