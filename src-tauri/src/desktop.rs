use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::sync::Arc;
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::{
    elevation_native::ElevationSession,
    preflight::Inspection,
    update_bridge::{Settings, UpdateBridge, UpdateStatus},
    windows_native,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupResult {
    inspection: Inspection,
    updates: UpdateStatus,
}

#[tauri::command]
async fn startup_check(
    window: WebviewWindow,
    bridge: tauri::State<'_, Arc<UpdateBridge>>,
) -> Result<StartupResult, String> {
    if window.label() != "installer" {
        return Err("unsupported_window".into());
    }
    let inspection = tauri::async_runtime::spawn_blocking(|| {
        windows_native::inspect(crate::preflight::CONTRACT_VERSION)
    })
    .await
    .map_err(|_| "preflight_task_failed")??;
    if inspection.platform_supported
        && inspection.webview_version.is_some()
        && bridge.status()?.stage == "idle"
        && let Err(code) = bridge.refresh().await
    {
        bridge.block(&code)?;
    }
    Ok(StartupResult {
        inspection,
        updates: bridge.status()?,
    })
}

#[tauri::command]
fn update_status(
    window: WebviewWindow,
    bridge: tauri::State<'_, Arc<UpdateBridge>>,
) -> Result<UpdateStatus, String> {
    if window.label() != "installer" {
        return Err("unsupported_window".into());
    }
    bridge.status()
}

#[tauri::command]
async fn install_update(
    window: WebviewWindow,
    operation_id: String,
    bridge: tauri::State<'_, Arc<UpdateBridge>>,
) -> Result<UpdateStatus, String> {
    if window.label() != "installer" {
        return Err("unsupported_window".into());
    }
    // Consent belongs to the native session; the service independently verifies
    // caller signature, intent, nonce, authorization and package bytes.
    if !bridge.status()?.service_ready {
        return Err("elevation_required".into());
    }
    bridge.install(&operation_id).await
}

#[tauri::command]
async fn elevation_state(
    window: WebviewWindow,
    bridge: tauri::State<'_, Arc<UpdateBridge>>,
) -> Result<bool, String> {
    if window.label() != "installer" {
        return Err("unsupported_window".into());
    }
    Ok(bridge.service_ready().await)
}

#[tauri::command]
async fn request_elevation(
    window: WebviewWindow,
    session: tauri::State<'_, Arc<ElevationSession>>,
    bridge: tauri::State<'_, Arc<UpdateBridge>>,
) -> Result<bool, String> {
    if window.label() != "installer" {
        return Err("unsupported_window".into());
    }
    if !bridge.service_ready().await {
        let owner = window.hwnd().map_err(|_| "elevation_owner_failed")?.0 as isize;
        let session = Arc::clone(session.inner());
        tauri::async_runtime::spawn_blocking(move || session.request(owner))
            .await
            .map_err(|_| "elevation_task_failed")??;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while !bridge.service_ready().await {
            if tokio::time::Instant::now() >= deadline {
                return Err("service_bootstrap_failed".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    // Network/catalog failure does not revoke a successfully registered service
    // or misreport an approved UAC prompt as a permission failure.
    if let Err(code) = bridge.refresh().await {
        bridge.block(&code)?;
    }
    Ok(true)
}

#[tauri::command]
fn inspect(contract_version: u32) -> Result<Inspection, String> {
    let result = windows_native::inspect(contract_version);
    #[cfg(debug_assertions)]
    eprintln!(
        "installer web preflight: {}",
        if result.is_ok() { "ok" } else { "failed" }
    );
    result
}

#[tauri::command]
fn window_control(window: WebviewWindow, action: &str) -> Result<(), String> {
    match action {
        "minimize" => window.minimize(),
        "close" => window.close(),
        "drag" => window.start_dragging(),
        _ => return Err("unsupported_window_action".into()),
    }
    .map_err(|_| "window_action_failed".into())
}

#[tauri::command]
fn installer_screen(
    window: WebviewWindow,
    screen: &str,
    layout: tauri::State<'_, Arc<crate::window_layout::LayoutState>>,
) -> Result<(), String> {
    crate::window_layout::set_screen(&window, layout.inner(), screen)
}

#[tauri::command]
fn open_webview_download() -> Result<(), String> {
    windows_native::open_webview_download()
}

fn warning(description: &str) {
    MessageDialog::new()
        .set_title("Baykuş Lig Kurulum")
        .set_level(MessageLevel::Warning)
        .set_description(description)
        .set_buttons(MessageButtons::Ok)
        .show();
}

fn fallback() -> bool {
    loop {
        let action = MessageDialog::new()
            .set_title("Baykuş Lig — WebView2 kullanılamıyor")
            .set_level(MessageLevel::Warning)
            .set_description("Kurulum ekranı açılamadı. Microsoft Edge WebView2 Runtime eksik veya başlatılamıyor. Resmî sayfadan kurduktan sonra yeniden deneyebilirsin.")
            .set_buttons(MessageButtons::YesNoCancelCustom(
                "Resmî indirme sayfası".into(),
                "Yeniden dene".into(),
                "Kapat".into(),
            ))
            .show();
        match action {
            MessageDialogResult::Custom(value) if value == "Resmî indirme sayfası" => {
                if windows_native::open_webview_download().is_err() {
                    warning(
                        "Resmî sayfa açılamadı. Tarayıcından Microsoft WebView2 sayfasını açabilirsin.",
                    );
                }
            }
            MessageDialogResult::Custom(value) if value == "Yeniden dene" => return true,
            _ => return false,
        }
    }
}

pub fn run() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|value| value == "--elevation-worker")
    {
        let result = match arguments.as_slice() {
            [_, pid] => pid
                .parse::<u32>()
                .map_err(|_| "invalid_parent_pid".to_string())
                .and_then(crate::elevation_native::run_worker),
            _ => Err("invalid_worker_arguments".into()),
        };
        if result.is_err() {
            std::process::exit(1);
        }
        return;
    }
    if arguments.as_slice() == ["--inspect"] {
        match windows_native::inspect(crate::preflight::CONTRACT_VERSION) {
            Ok(value) => match serde_json::to_string_pretty(&value) {
                Ok(json) => println!("{json}"),
                Err(_) => std::process::exit(1),
            },
            Err(code) => {
                eprintln!("{code}");
                std::process::exit(1);
            }
        }
        return;
    }
    if !arguments.is_empty() {
        warning("Bu kurulum önkontrolü verilen parametreleri desteklemiyor.");
        return;
    }
    loop {
        if tauri::webview_version().is_ok() {
            break;
        }
        if !fallback() {
            return;
        }
    }
    let profile = match windows_native::installer_profile() {
        Ok(path) => path,
        Err(_) => {
            warning("Kurulum ekranı için kullanıcı klasörü belirlenemedi.");
            return;
        }
    };
    let bridge = match Settings::embedded().and_then(|settings| {
        UpdateBridge::new(
            settings,
            std::env::consts::ARCH,
            profile
                .parent()
                .ok_or("installer_profile_unavailable")?
                .join("Updates"),
        )
    }) {
        Ok(bridge) => Arc::new(bridge),
        Err(_) => {
            warning("Installer güncelleme kaynağı veya güvenilen anahtarları geçersiz.");
            return;
        }
    };
    let app = match tauri::Builder::default()
        .manage(Arc::new(ElevationSession::default()))
        .manage(bridge)
        .manage(Arc::new(crate::window_layout::LayoutState::default()))
        .invoke_handler(tauri::generate_handler![
            inspect,
            window_control,
            open_webview_download,
            elevation_state,
            request_elevation,
            installer_screen,
            startup_check,
            update_status,
            install_update
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(_) => {
            warning("Kurulum penceresi hazırlanamadı. Uygulamayı kapatıp yeniden deneyebilirsin.");
            return;
        }
    };
    loop {
        let created =
            WebviewWindowBuilder::new(&app, "installer", WebviewUrl::App("index.html".into()))
                .title("Baykuş Lig Kurulum")
                .inner_size(960.0, 480.0)
                .visible(false)
                .transparent(true)
                .shadow(false)
                .background_color(tauri::window::Color(0, 0, 0, 0))
                .decorations(false)
                .resizable(false)
                .maximizable(false)
                .data_directory(profile.clone())
                .disable_drag_drop_handler()
                .on_navigation(|url| {
                    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
                })
                .build();
        if let Ok(window) = created {
            if crate::webview_settings::initialize(&window).is_err() {
                let _ = window.close();
                warning("Kurulum ekranının WebView2 ayarları uygulanamadı.");
                return;
            }
            if crate::window_layout::initialize(&window).is_err() {
                let _ = window.close();
                warning("Kurulum penceresinin ekran ölçüleri ayarlanamadı.");
                return;
            }
            break;
        }
        if !fallback() {
            return;
        }
    }
    app.run(|handle, event| {
        if let tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::Destroyed,
            ..
        } = event
            && handle.webview_windows().is_empty()
        {
            handle.exit(0);
        }
    });
}
