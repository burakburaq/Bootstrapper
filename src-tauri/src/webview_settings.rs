//! Installer-only WebView2 settings, applied on the owning webview thread.
use tauri::{WebviewWindow, webview::PlatformWebview};

fn configure(webview: PlatformWebview) -> windows_core::Result<()> {
    // SAFETY: Tauri supplies live, owned COM interfaces on their owning thread.
    // This callback only changes the default context menu setting.
    unsafe {
        webview
            .controller()
            .CoreWebView2()?
            .Settings()?
            .SetAreDefaultContextMenusEnabled(false)?;
    }
    Ok(())
}

pub(crate) fn initialize(window: &WebviewWindow) -> tauri::Result<()> {
    let handle = window.clone();
    window.with_webview(move |webview| {
        if let Err(error) = configure(webview) {
            eprintln!("Installer WebView2 ayarları uygulanamadı: {error}");
            let _ = handle.close();
        }
    })
}
