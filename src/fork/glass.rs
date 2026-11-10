//! Fork (Windows): glass themes. The main window is created see-through
//! with Windows 11's acrylic blur behind it; opaque themes paint every pixel
//! and look as before, themes whose colours have alpha show the blur.

/// Puts Windows 11's acrylic blur behind the main window, which is
/// created see-through on Windows. An opaque palette paints every pixel, so
/// the blur shows only where a theme's colours have alpha (a glass theme).
/// Windows before 11 22H2 ignores the request and shows the desktop unblurred.
pub fn frosted_backdrop(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;

    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    let dark: i32 = 1;
    let backdrop = DWMSBT_TRANSIENTWINDOW;
    // The whole client area is the backdrop's, not only a frame strip.
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        DwmExtendFrameIntoClientArea(hwnd, &margins);
        // Dark acrylic; the themes that use it are dark.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of::<i32>() as u32,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&raw const backdrop).cast(),
            size_of_val(&backdrop) as u32,
        );
    }
    keep_glass(hwnd, fastframe_titlebar::enabled());
}

/// A subclass of the main window that keeps the glass right.
///
/// - Windows dims the acrylic when the window loses focus, because DWM
///   follows the frame's active state. `WM_NCACTIVATE` is passed on as active
///   so the frame, and the blur, stay as they are; keyboard focus still moves
///   (winit follows `WM_SETFOCUS`/`WM_KILLFOCUS`).
/// - With Spotifast's own title bar, Windows must not draw its caption buttons
///   in the frame `frosted_backdrop` extends over the window. They need
///   `WS_SYSMENU`, which winit keeps on undecorated windows (with
///   `WS_CAPTION`, for Aero Snap) and sets again whenever the window's state
///   changes, so `WM_STYLECHANGING` keeps it off. Snap, resizing and the
///   taskbar use other styles.
fn keep_glass(hwnd: windows_sys::Win32::Foundation::HWND, custom_titlebar: bool) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WM_NCACTIVATE, WM_STYLECHANGING,
        WS_SYSMENU,
    };

    /// `custom_titlebar` rides in the subclass's reference data.
    unsafe extern "system" fn subclass(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _: usize,
        custom_titlebar: usize,
    ) -> LRESULT {
        if message == WM_NCACTIVATE {
            return unsafe { DefSubclassProc(hwnd, message, 1, lparam) };
        }
        if custom_titlebar != 0 && message == WM_STYLECHANGING && wparam as i32 == GWL_STYLE {
            // SAFETY: for WM_STYLECHANGING, lparam points at the new styles.
            unsafe { (*(lparam as *mut STYLESTRUCT)).styleNew &= !WS_SYSMENU };
        }
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }

    // SAFETY: hwnd is the app's live main window, on its own thread.
    unsafe {
        SetWindowSubclass(hwnd, Some(subclass), 1, usize::from(custom_titlebar));
        if custom_titlebar {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_SYSMENU as isize));
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}
