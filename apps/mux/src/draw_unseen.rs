//! A debug build's way to be looked at while nobody can see it.
//!
//! GPUI stops asking for frames when macOS says the window is covered, and a
//! locked screen covers every window. A window captured by id is still what
//! was last drawn, so a build under test stands still. With
//! `MUX_DEV_DRAW_UNSEEN` set, a debug build draws a frame a few times a
//! second while it is covered. Release builds have none of this.

// AppKit is asked through the Objective-C runtime.
#![allow(unsafe_code)]
// The `objc` macros test a `cfg` this crate has never heard of.
#![allow(unexpected_cfgs)]

use std::time::Duration;

use gpui::{App, Window};
use objc::runtime::Object;
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// `NSWindowOcclusionStateVisible`.
const VISIBLE: u64 = 1 << 1;
const EVERY: Duration = Duration::from_millis(120);

/// Keep `window` drawing while it is covered, if the environment asks.
pub(crate) fn keep_drawing(window: &Window, cx: &mut App) {
    if std::env::var_os("MUX_DEV_DRAW_UNSEEN").is_none() {
        return;
    }
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // The address only: the task below runs on the main thread, where the
    // view lives, and ends with the application.
    let view = handle.ns_view.as_ptr() as usize;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(EVERY).await;
            // Nothing of GPUI's is borrowed here, so its own frame callback
            // is free to run.
            unsafe {
                let view = view as *mut Object;
                let native: *mut Object = msg_send![view, window];
                if native.is_null() {
                    return;
                }
                let state: u64 = msg_send![native, occlusionState];
                if state & VISIBLE == 0 {
                    let layer: *mut Object = msg_send![view, layer];
                    let _: () = msg_send![view, displayLayer: layer];
                }
            }
        }
    })
    .detach();
}
