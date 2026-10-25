#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod preflight;
mod product;
mod update_bridge;

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_native;

#[cfg(windows)]
#[allow(unsafe_code)]
mod elevation_native;

#[cfg(windows)]
mod desktop;

#[cfg(windows)]
mod service_bootstrap;

#[cfg(windows)]
mod window_layout;

#[cfg(windows)]
#[allow(unsafe_code)]
mod webview_settings;

#[cfg(windows)]
#[allow(unsafe_code)]
mod window_region;

fn main() {
    #[cfg(windows)]
    desktop::run();
    #[cfg(not(windows))]
    {
        eprintln!("Bu installer temeli yalnız Windows içindir.");
        std::process::exit(1);
    }
}
