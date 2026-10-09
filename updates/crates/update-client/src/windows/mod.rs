//! Windows adapter. Privileged paths and arguments never come from WebView/IPC.
mod native;
pub mod service;
pub use native::application_guard;
pub use native::uninstall_product;
pub use native::{
    WindowsInstaller, WindowsPolicy, bootstrap_context, read_product_version, verify_authenticode,
};
