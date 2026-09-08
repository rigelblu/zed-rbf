use crate::{ButtonSize, DynamicSpacing};
use gpui::{App, Pixels, Window, px};

pub const MACOS_SDK_26_OR_LATER: bool = cfg!(macos_sdk_26_or_later);

// Use pixels here instead of a rem-based size because the macOS traffic
// lights are a static size, and don't scale with the rest of the UI.
//
// Magic number: There is one extra pixel of padding on the left side due to
// the 1px border around the window on macOS apps.
pub const TRAFFIC_LIGHT_PADDING: f32 = if MACOS_SDK_26_OR_LATER { 78. } else { 71. };

/// Returns the platform-appropriate title bar height.
///
/// On Windows, this returns a fixed height of 32px.
/// On other platforms, it scales with the window's rem size (1.75x) with a minimum of 34px.
#[cfg(not(target_os = "windows"))]
pub fn platform_title_bar_height(window: &Window) -> Pixels {
    (1.75 * window.rem_size()).max(px(34.))
}

#[cfg(target_os = "windows")]
pub fn platform_title_bar_height(_window: &Window) -> Pixels {
    // todo(windows) instead of hard coded size report the actual size to the Windows platform API
    px(32.)
}

/// The status bar's height.
///
/// The bar sizes itself from its content — `Base04` padding around a small icon
/// button — so until now nothing else could ask how tall it is. The sidebar slot spans
/// the full window height while the status bar stops at the slot's edge, so an
/// occupant that wants the chrome to run unbroken across its foot has to paint its own
/// band, and it needs this to size it.
///
/// **Derived, and checked against the real bar.** The status bar sizes itself from its
/// content, so this reproduces that content's box — a default-size control plus the
/// bar's padding — rather than the bar reading a height from here. Setting the bar's
/// height from this instead was tried and reverted: it shrank the chrome for everyone
/// to serve a band.
///
/// `test_status_bar_height_matches_the_bar_the_workspace_renders` renders the real bar
/// and compares. Without it a wrong derivation is invisible, because the only other
/// reader is the band being checked against it — which is how the first version shipped
/// 3.25px short.
pub fn status_bar_height(window: &mut Window, cx: &mut App) -> Pixels {
    ButtonSize::Default.rems().to_pixels(window.rem_size()) + DynamicSpacing::Base04.px(cx) * 2.
}
