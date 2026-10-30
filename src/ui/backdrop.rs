//! Native Windows 11 material. Only the backing surface is translucent;
//! text, video, icons, and input remain fully opaque and interactive.
use iced::window;

pub fn configure(window: &dyn window::Window, dark: bool) -> Result<(), String> {
    use std::{ffi::c_void, mem::size_of};
    use window::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::{
        Graphics::Dwm::{
            DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmSetWindowAttribute,
        },
        UI::Controls::MARGINS,
    };

    let handle = window.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Window is not backed by Win32".into());
    };
    let hwnd = handle.hwnd.get() as *mut c_void;
    let set = |attribute: i32, value: i32| {
        // SAFETY: iced::window::run keeps this HWND alive and invokes us on
        // its event-loop thread. These DWM attributes all take a 32-bit value.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                (&value as *const i32).cast(),
                size_of::<i32>() as u32,
            )
        }
    };
    // Keep the native caption, resize borders, shadow, and Snap Layouts.
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(dark));
    if !iced_wgpu::window::Compositor::supports_transparency() {
        set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE);
        return Err("Renderer cannot preserve surface alpha".into());
    }
    let result = set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW);
    if result < 0 {
        return Err(format!(
            "Desktop Acrylic unavailable: HRESULT {result:#010x}"
        ));
    }
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // SAFETY: HWND is live; margins remains valid for the synchronous call.
    let result = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    if result < 0 {
        set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE);
        return Err(format!(
            "Cannot extend native material: HRESULT {result:#010x}"
        ));
    }
    let mut applied = DWMSBT_NONE;
    // SAFETY: applied is a writable 32-bit destination of the supplied size.
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&mut applied as *mut i32).cast(),
            size_of::<i32>() as u32,
        )
    };
    if result < 0 || applied != DWMSBT_TRANSIENTWINDOW {
        return Err(format!(
            "Native material verification failed: {result:#010x}, type={applied}"
        ));
    }
    Ok(())
}
