//! Verified update download and installer-independent integration API.
pub mod delta;
pub mod installation;
pub mod inventory;
pub mod journal;
pub mod package;
pub mod progress;
mod transport;
#[cfg(windows)]
pub mod windows;
pub use journal::*;
pub use package::*;
pub use transport::*;
pub use update_protocol;
