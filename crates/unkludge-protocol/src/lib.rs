//! Protocol library for Royal Kludge keyboards.

#[expect(dead_code, reason = "the feature modules will call the send paths")]
pub mod device;
mod error;
pub mod model;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the feature modules will build these reports")
)]
pub(crate) mod packet;
pub mod transport;

pub use device::Device;
pub use error::Error;
pub use packet::{REPORT_ID, REPORT_LEN};
