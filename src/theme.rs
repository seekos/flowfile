use crate::models::ThemePreference;
use gpui::{Hsla, Pixels, WindowAppearance, px, rgb};
use std::sync::atomic::{AtomicBool, Ordering};

static DARK_MODE: AtomicBool = AtomicBool::new(false);
const UI_TEXT_SCALE: f32 = 1.30;
const UI_ICON_SCALE: f32 = 1.20;

/// Keeps the application typography on one coherent scale while matching
/// native macOS desktop density across panes and sidebars.
pub fn font(base_size: f32) -> Pixels {
    px(base_size * UI_TEXT_SCALE)
}

/// Scales monochrome UI icons together with the global typography scale so
/// toolbar buttons, navigation controls, and sidebar badges stay balanced.
pub fn icon_size(base_size: f32) -> Pixels {
    px(base_size * UI_ICON_SCALE)
}

pub fn apply(preference: ThemePreference, system: WindowAppearance) {
    let dark = match preference {
        ThemePreference::Auto => {
            matches!(
                system,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            )
        }
        ThemePreference::Light => false,
        ThemePreference::Dark => true,
    };
    DARK_MODE.store(dark, Ordering::Release);
}

fn dark() -> bool {
    DARK_MODE.load(Ordering::Acquire)
}

pub fn canvas() -> Hsla {
    rgb(if dark() { 0x161719 } else { 0xf0f0f3 }).into()
}

pub fn surface() -> Hsla {
    rgb(if dark() { 0x1e1f22 } else { 0xffffff }).into()
}

pub fn surface_subtle() -> Hsla {
    rgb(if dark() { 0x26272b } else { 0xf5f5f7 }).into()
}

pub fn sidebar() -> Hsla {
    rgb(if dark() { 0x252629 } else { 0xededef }).into()
}

pub fn sidebar_active() -> Hsla {
    rgb(if dark() { 0x38393e } else { 0xdcdce0 }).into()
}

pub fn toolbar() -> Hsla {
    rgb(if dark() { 0x1e1f22 } else { 0xffffff }).into()
}

pub fn control_track() -> Hsla {
    rgb(if dark() { 0x2c2d31 } else { 0xf0f0f2 }).into()
}

pub fn hover_subtle() -> Hsla {
    rgb(if dark() { 0x313237 } else { 0xe6e6e9 }).into()
}

pub fn inactive_selection() -> Hsla {
    rgb(if dark() { 0x34353a } else { 0xe4e4e7 }).into()
}

pub fn border() -> Hsla {
    rgb(if dark() { 0x343539 } else { 0xe5e5e7 }).into()
}

pub fn border_strong() -> Hsla {
    rgb(if dark() { 0x46484e } else { 0xcfd0d4 }).into()
}

pub fn text_primary() -> Hsla {
    rgb(if dark() { 0xf5f5f7 } else { 0x1d1d1f }).into()
}

pub fn text_secondary() -> Hsla {
    rgb(if dark() { 0xa1a1a6 } else { 0x6e6e73 }).into()
}

pub fn text_tertiary() -> Hsla {
    rgb(if dark() { 0x78787e } else { 0x8e8e93 }).into()
}

pub fn accent() -> Hsla {
    rgb(if dark() { 0x0a84ff } else { 0x007aff }).into()
}

pub fn accent_soft() -> Hsla {
    rgb(if dark() { 0x1f3a5c } else { 0xe3f0ff }).into()
}

pub fn danger() -> Hsla {
    rgb(0xc43e4b).into()
}

pub fn danger_soft() -> Hsla {
    rgb(if dark() { 0x4a252d } else { 0xffecee }).into()
}

pub fn folder() -> Hsla {
    rgb(0x4eb7f5).into()
}

pub fn folder_tab() -> Hsla {
    rgb(0x369ee0).into()
}

pub fn file_blue() -> Hsla {
    rgb(0x5d83b9).into()
}

pub fn file_green() -> Hsla {
    rgb(0x4d9870).into()
}

pub fn file_purple() -> Hsla {
    rgb(0x8268b0).into()
}

pub fn terminal_background() -> Hsla {
    rgb(if dark() { 0x0e1319 } else { 0x202833 }).into()
}

pub fn terminal_foreground() -> Hsla {
    rgb(0x78d69f).into()
}
