use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};

#[derive(Default)]
pub(crate) struct LayoutState {
    compact: AtomicBool,
    changing: AtomicBool,
}

fn installer_size(screen_height: u32) -> PhysicalSize<u32> {
    // 1080p is the 1:1 CSS reference; other scales are 2/3, 3/2 and 2.
    let width: u32 = if screen_height <= 768 {
        340
    } else if screen_height <= 1080 {
        510
    } else if screen_height <= 1440 {
        765
    } else {
        1020
    };
    PhysicalSize::new(width, width.div_ceil(2))
}

fn monitor_size(window: &WebviewWindow) -> tauri::Result<PhysicalSize<u32>> {
    let monitor = match window.current_monitor()? {
        Some(monitor) => Some(monitor),
        None => window.primary_monitor()?,
    };
    Ok(installer_size(
        monitor.map_or(1440, |monitor| monitor.size().height),
    ))
}

fn resize(window: &WebviewWindow, compact: bool) -> tauri::Result<()> {
    let size = monitor_size(window)?;
    let old_size = window.inner_size()?;
    let old_position = window.outer_position()?;
    let position = PhysicalPosition::new(
        old_position.x + (old_size.width as i32 - size.width as i32) / 2,
        old_position.y + (old_size.height as i32 - size.height as i32) / 2,
    );
    window.set_min_size(None::<tauri::Size>)?;
    window.set_max_size(None::<tauri::Size>)?;
    window.set_size(size)?;
    window.set_position(position)?;
    window.set_min_size(Some(size))?;
    window.set_max_size(Some(size))?;
    // Preserve physical dimensions regardless of Windows display scaling.
    window.set_zoom(1.0 / window.scale_factor()?)?;
    crate::window_region::apply(window, compact).map_err(Into::into)
}

pub(crate) fn set_screen(
    window: &WebviewWindow,
    state: &LayoutState,
    screen: &str,
) -> Result<(), String> {
    if window.label() != "installer" {
        return Err("unsupported_window".into());
    }
    let compact = match screen {
        "selection" => false,
        "details" => true,
        _ => return Err("unsupported_installer_screen".into()),
    };
    if state.changing.swap(true, Ordering::SeqCst) {
        return Err("window_layout_busy".into());
    }
    let previous = state.compact.swap(compact, Ordering::SeqCst);
    // Screen transitions never change native bounds or WebView zoom.
    let result = crate::window_region::apply(window, compact);
    if result.is_err() {
        state.compact.store(previous, Ordering::SeqCst);
        let _ = crate::window_region::apply(window, previous);
    }
    state.changing.store(false, Ordering::SeqCst);
    result.map_err(|_| "window_region_failed".into())
}

pub(crate) fn initialize(window: &WebviewWindow) -> tauri::Result<()> {
    resize(window, false)?;
    window.center()?;
    let handle = window.clone();
    let state = Arc::clone(window.state::<Arc<LayoutState>>().inner());
    window.on_window_event(move |event| {
        if state.changing.load(Ordering::SeqCst) {
            return;
        }
        let needs_resize = match event {
            WindowEvent::ScaleFactorChanged { .. } => true,
            WindowEvent::Moved(_) => monitor_size(&handle)
                .and_then(|size| handle.inner_size().map(|current| current != size))
                .unwrap_or(false),
            _ => false,
        };
        if needs_resize && !state.changing.swap(true, Ordering::SeqCst) {
            if let Err(error) = resize(&handle, state.compact.load(Ordering::SeqCst)) {
                eprintln!("Installer pencere ölçüsü güncellenemedi: {error}");
            }
            state.changing.store(false, Ordering::SeqCst);
        }
    });
    window.show()
}
