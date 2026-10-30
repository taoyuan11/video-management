//! One embedded brand asset for navigation, native captions and the taskbar.
use iced::{widget::image, window};
use std::sync::OnceLock;

const LOGO: &[u8] = include_bytes!("../../assets/branding/frame-library-256.png");

pub fn logo() -> image::Handle {
    static HANDLE: OnceLock<image::Handle> = OnceLock::new();
    HANDLE
        .get_or_init(|| image::Handle::from_bytes(LOGO))
        .clone()
}

pub fn window_icon() -> window::Icon {
    static ICON: OnceLock<window::Icon> = OnceLock::new();
    ICON.get_or_init(|| {
        let rgba = ::image::load_from_memory_with_format(LOGO, ::image::ImageFormat::Png)
            .expect("Invalid embedded application icon")
            .into_rgba8();
        let (width, height) = rgba.dimensions();
        window::icon::from_rgba(rgba.into_raw(), width, height)
            .expect("Invalid embedded application icon dimensions")
    })
    .clone()
}

pub fn set_taskbar_icon(window: &dyn window::Window) -> Result<(), String> {
    use window::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            ICON_BIG, IMAGE_ICON, LR_SHARED, LoadImageW, SendMessageW, WM_SETICON,
        },
    };

    let handle = window.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Window is not backed by Win32".into());
    };
    // iced/winit sets only ICON_SMALL from window::Settings. Set ICON_BIG as
    // well for the taskbar and Alt+Tab, without changing vendored platform code.
    // SAFETY: window::run invokes this on the event-loop thread with a live
    // HWND. Resource 1 is linked by build.rs; LR_SHARED keeps its HICON alive
    // until process exit, so it must not be passed to DestroyIcon.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        if module.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let icon = LoadImageW(
            module,
            std::ptr::without_provenance(1), // MAKEINTRESOURCEW(1), not a string pointer
            IMAGE_ICON,
            256,
            256,
            LR_SHARED,
        );
        if icon.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        SendMessageW(
            handle.hwnd.get() as _,
            WM_SETICON,
            ICON_BIG as usize,
            icon as isize,
        );
    }
    Ok(())
}
