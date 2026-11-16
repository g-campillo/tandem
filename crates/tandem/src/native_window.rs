//! The two things Tandem asks macOS for that GPUI has no call for: the opacity of the
//! window's content and the strength of the blur behind the window. All unsafe code and
//! every foreign call of this crate is in this file; the functions are safe to call from the
//! main thread, which is where GPUI runs views.
//!
//! Opacity is the alpha of GPUI's own view, the layer everything Tandem draws ends up in,
//! so text, panels and popups fade together and the blur behind the window shows through
//! them. (The window's own `alphaValue` fades the whole window as one, its blur included.)
//!
//! Blur strength is the window server's background blur radius, set with the private
//! `CGSSetWindowBackgroundBlurRadius`, as iTerm2, kitty and Alacritty do. The symbol is
//! looked up when it is needed, so a system without it is reported, not a crash.
use gpui_kit::Window;

/// Sets the opacity of everything Tandem draws in `window`: 1.0 is opaque. Values outside
/// 0.0..=1.0 are brought inside. Returns whether the system took the value.
pub fn set_content_opacity(window: &Window, alpha: f32) -> bool {
    imp::set_content_opacity(window, alpha.clamp(0., 1.))
}

/// Sets the radius of the blur the window server draws behind `window`; 0 turns it off.
/// The blur shows wherever the window is not opaque. Returns `false` when this system has
/// no such call or refused it, and the caller should fall back to GPUI's built-in blur.
pub fn set_blur_radius(window: &Window, radius: i32) -> bool {
    imp::set_blur_radius(window, radius.max(0))
}

#[cfg(target_os = "macos")]
mod imp {
    use gpui_kit::Window;
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::{CStr, c_void};
    use std::ptr::NonNull;

    /// GPUI's view in the window: the `NSView` whose layer GPUI renders into.
    fn view(window: &Window) -> Option<NonNull<AnyObject>> {
        // `Window` also has a method of its own named `window_handle`.
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::AppKit(handle) => Some(handle.ns_view.cast()),
            _ => None,
        }
    }

    /// The `NSWindow` that holds `view`, if it is in one.
    fn ns_window(view: NonNull<AnyObject>) -> Option<NonNull<AnyObject>> {
        // SAFETY: `view` is a live NSView (GPUI keeps it for as long as the `Window`
        // exists), `-[NSView window]` takes no arguments and returns an object or nil,
        // and this runs on the main thread.
        let window: *mut AnyObject = unsafe { msg_send![view.as_ptr(), window] };
        NonNull::new(window)
    }

    pub fn set_content_opacity(window: &Window, alpha: f32) -> bool {
        let Some(view) = view(window) else {
            return false;
        };
        // SAFETY: `view` is a live NSView on the main thread; `setAlphaValue:` takes a
        // CGFloat, which is an f64 on every Mac Tandem runs on.
        unsafe {
            let _: () = msg_send![view.as_ptr(), setAlphaValue: f64::from(alpha)];
        }
        if let Some(ns_window) = ns_window(view) {
            // The window's shadow follows the shape of its content, which just changed.
            // SAFETY: `ns_window` is a live NSWindow on the main thread.
            unsafe {
                let _: () = msg_send![ns_window.as_ptr(), invalidateShadow];
            }
        }
        true
    }

    type MainConnection = unsafe extern "C" fn() -> i32;
    type SetBlurRadius = unsafe extern "C" fn(i32, i32, i32) -> i32;

    /// The address of a function in a library the app has loaded, or null.
    fn symbol(name: &CStr) -> *mut c_void {
        // SAFETY: `name` is a valid C string and RTLD_DEFAULT searches the images that
        // are already loaded; nothing is loaded or run.
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }
    }

    pub fn set_blur_radius(window: &Window, radius: i32) -> bool {
        let Some(ns_window) = view(window).and_then(ns_window) else {
            return false;
        };
        let connection = symbol(c"CGSMainConnectionID");
        let set_radius = symbol(c"CGSSetWindowBackgroundBlurRadius");
        if connection.is_null() || set_radius.is_null() {
            return false;
        }
        // SAFETY: `ns_window` is a live NSWindow on the main thread and `windowNumber`
        // returns an NSInteger.
        let number: isize = unsafe { msg_send![ns_window.as_ptr(), windowNumber] };
        let Ok(number) = i32::try_from(number) else {
            return false;
        };
        if number <= 0 {
            // The window is not known to the window server yet.
            return false;
        }
        // SAFETY: both addresses were just resolved, and these are the C signatures of the
        // two CoreGraphics functions: `int CGSMainConnectionID(void)` and
        // `CGError CGSSetWindowBackgroundBlurRadius(int connection, int window, int radius)`.
        // The call only changes how the window server draws this app's own window.
        unsafe {
            let connection: MainConnection = std::mem::transmute(connection);
            let set_radius: SetBlurRadius = std::mem::transmute(set_radius);
            set_radius(connection(), number, radius) == 0
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use gpui_kit::Window;

    pub fn set_content_opacity(_: &Window, _: f32) -> bool {
        false
    }

    pub fn set_blur_radius(_: &Window, _: i32) -> bool {
        false
    }
}
