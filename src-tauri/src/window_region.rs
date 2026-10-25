//! Native input/drawing region; screen transitions leave the window bounds intact.
use std::{io, ptr};
use tauri::{PhysicalSize, WebviewWindow};
use windows_sys::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, HRGN, SetWindowRgn};

fn compact_region(size: PhysicalSize<u32>) -> io::Result<HRGN> {
    let width = i32::try_from(size.width).map_err(io::Error::other)?;
    let height = i32::try_from(size.height).map_err(io::Error::other)?;
    // Round outward so subpixel CSS boundaries aren't clipped.
    let top = (height * 85 - width * 16) / 170;
    let bottom = height - top;
    // SAFETY: These are bounded physical window coordinates. A successful call
    // creates a GDI region owned by the caller until SetWindowRgn accepts it.
    let region = unsafe { CreateRectRgn(0, top, width, bottom) };
    if region.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(region)
    }
}

pub(crate) fn apply(window: &WebviewWindow, compact: bool) -> io::Result<()> {
    let hwnd = window.hwnd().map_err(io::Error::other)?.0;
    let region = if compact {
        compact_region(window.inner_size().map_err(io::Error::other)?)?
    } else {
        ptr::null_mut()
    };
    // SAFETY: The live Tauri window owns hwnd. Success transfers ownership of
    // region to Windows; NULL removes the mask and restores the full window.
    if unsafe { SetWindowRgn(hwnd, region, 1) } == 0 {
        let error = io::Error::last_os_error();
        if !region.is_null() {
            // SAFETY: The failed SetWindowRgn did not take ownership; free once.
            unsafe { DeleteObject(region) };
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::PtInRegion;

    #[test]
    fn compact_mask_keeps_the_center_band_and_excludes_empty_margins() {
        for (width, height, top, bottom) in [
            (340, 170, 53, 117),
            (510, 255, 79, 176),
            (765, 383, 119, 264),
            (1020, 510, 159, 351),
        ] {
            let region = compact_region(PhysicalSize::new(width, height)).unwrap();
            // SAFETY: region is a live GDI object owned by this test. Query it
            // before deleting exactly once; no window takes ownership here.
            unsafe {
                let samples = [
                    PtInRegion(region, 0, top),
                    PtInRegion(region, width as i32 - 1, bottom - 1),
                    PtInRegion(region, width as i32 / 2, top - 1),
                    PtInRegion(region, width as i32 / 2, bottom),
                ];
                let deleted = DeleteObject(region);
                assert_eq!(samples.map(|value| value != 0), [true, true, false, false]);
                assert_ne!(deleted, 0);
            }
        }
    }
}
