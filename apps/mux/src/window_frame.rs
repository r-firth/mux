//! The window opens where it was last left, at the size it was, as a Mac
//! app's window does: the frame is kept in the state directory as it
//! changes. A frame on a display that is gone, or too small to use, is
//! passed over for the usual centred window.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{App, Bounds, Context, Pixels, Window, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

use super::{MuxApp, WINDOW_HEIGHT, WINDOW_WIDTH};

const FILE: &str = "window.json";
/// The smallest window Mux opens at, as its minimum size allows.
pub(super) const MIN_WIDTH: f32 = 560.0;
pub(super) const MIN_HEIGHT: f32 = 360.0;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
struct SavedFrame {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    /// Zoomed to fill the screen; the frame is where it goes back to.
    #[serde(default)]
    maximized: bool,
}

impl SavedFrame {
    fn of(bounds: WindowBounds) -> Self {
        let frame = bounds.get_bounds();
        Self {
            x: f32::from(frame.origin.x),
            y: f32::from(frame.origin.y),
            width: f32::from(frame.size.width),
            height: f32::from(frame.size.height),
            maximized: matches!(bounds, WindowBounds::Maximized(_)),
        }
    }

    fn bounds(self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        )
    }

    /// Whether the frame can be opened at: big enough to use and showing
    /// on one of `displays`.
    fn fits(self, displays: &[Bounds<Pixels>]) -> bool {
        let usable = self.width.is_finite()
            && self.height.is_finite()
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width >= MIN_WIDTH
            && self.height >= MIN_HEIGHT;
        let frame = self.bounds();
        usable && displays.iter().any(|display| display.intersects(&frame))
    }
}

fn frame_path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

fn read_frame(state_dir: &Path) -> Option<SavedFrame> {
    serde_json::from_str(&std::fs::read_to_string(frame_path(state_dir)).ok()?).ok()
}

/// Where the window opens: where it was left when that is still on a
/// display, else centred at the usual size.
pub(super) fn opening_bounds(state_dir: Option<&Path>, cx: &App) -> WindowBounds {
    let displays = cx
        .displays()
        .iter()
        .map(|display| display.bounds())
        .collect::<Vec<_>>();
    match state_dir.and_then(read_frame) {
        Some(frame) if frame.fits(&displays) && frame.maximized => {
            WindowBounds::Maximized(frame.bounds())
        }
        Some(frame) if frame.fits(&displays) => WindowBounds::Windowed(frame.bounds()),
        _ => WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
            cx,
        )),
    }
}

impl MuxApp {
    /// Keep the window's frame as it moves and resizes, once it has
    /// settled for a moment.
    pub(super) fn keep_window_frame(window: &mut Window, cx: &mut Context<Self>) {
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some(state_dir) = this.state_dir.clone() else {
                return;
            };
            // Full screen is left the way it was entered, so the window
            // opens at the frame it will go back to.
            let frame = SavedFrame::of(window.window_bounds());
            this.window_frame_serial += 1;
            let serial = this.window_frame_serial;
            cx.spawn(async move |entity, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let current = entity
                    .update(cx, |this, _| this.window_frame_serial == serial)
                    .unwrap_or(false);
                if current {
                    save_frame(&state_dir, frame);
                }
            })
            .detach();
        })
        .detach();
    }
}

fn save_frame(state_dir: &Path, frame: SavedFrame) {
    let Ok(contents) = serde_json::to_string(&frame) else {
        return;
    };
    let path = frame_path(state_dir);
    let staged = path.with_extension("json.tmp");
    if std::fs::write(&staged, contents).is_ok() {
        let _ = std::fs::rename(&staged, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounds, SavedFrame, point, px, read_frame, save_frame, size};

    fn display(x: f32, y: f32, width: f32, height: f32) -> Bounds<gpui::Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn a_frame_opens_only_where_a_display_still_shows_it() {
        let frame = SavedFrame {
            x: 1800.0,
            y: 100.0,
            width: 1000.0,
            height: 700.0,
            maximized: false,
        };
        let laptop = display(0.0, 0.0, 1512.0, 982.0);
        let monitor = display(1512.0, 0.0, 2560.0, 1440.0);
        assert!(frame.fits(&[laptop, monitor]));
        // The monitor was unplugged: the frame is off every display.
        assert!(!frame.fits(&[laptop]));
        // Too small to use, or nonsense, is passed over.
        let tiny = SavedFrame {
            width: 100.0,
            ..frame
        };
        assert!(!tiny.fits(&[laptop, monitor]));
        let broken = SavedFrame {
            x: f32::NAN,
            ..frame
        };
        assert!(!broken.fits(&[laptop, monitor]));
    }

    #[test]
    fn a_saved_frame_reads_back() {
        let state = tempfile::tempdir().expect("state dir");
        let frame = SavedFrame {
            x: 40.0,
            y: 60.0,
            width: 1200.0,
            height: 800.0,
            maximized: true,
        };
        save_frame(state.path(), frame);
        assert_eq!(read_frame(state.path()), Some(frame));
        std::fs::write(state.path().join("window.json"), "{").expect("write");
        assert_eq!(read_frame(state.path()), None);
    }
}
