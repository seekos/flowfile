use crate::models::ThemePreference;
use gpui::{Hsla, Pixels, WindowAppearance, px, rgb};
use std::sync::atomic::{AtomicBool, Ordering};

static DARK_MODE: AtomicBool = AtomicBool::new(false);
const UI_TEXT_SCALE: f32 = 1.125;

/// Keeps the application typography on one coherent scale while preserving
/// the compact density expected from a multi-pane file manager.
pub fn font(base_size: f32) -> Pixels {
    px(base_size * UI_TEXT_SCALE)
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
    rgb(if dark() { 0x11161d } else { 0xf3f5f8 }).into()
}

pub fn surface() -> Hsla {
    rgb(if dark() { 0x1b222c } else { 0xffffff }).into()
}

pub fn surface_subtle() -> Hsla {
    rgb(if dark() { 0x202934 } else { 0xf7f9fb }).into()
}

pub fn sidebar() -> Hsla {
    rgb(if dark() { 0x171e27 } else { 0xeef2f6 }).into()
}

pub fn border() -> Hsla {
    rgb(if dark() { 0x344150 } else { 0xd9dfe7 }).into()
}

pub fn border_strong() -> Hsla {
    rgb(if dark() { 0x46566a } else { 0xc5ced9 }).into()
}

pub fn text_primary() -> Hsla {
    rgb(if dark() { 0xe5edf5 } else { 0x18212d }).into()
}

pub fn text_secondary() -> Hsla {
    rgb(if dark() { 0xa4b2c1 } else { 0x667181 }).into()
}

pub fn text_tertiary() -> Hsla {
    rgb(if dark() { 0x788899 } else { 0x8c96a5 }).into()
}

pub fn accent() -> Hsla {
    rgb(if dark() { 0x5a9cff } else { 0x2475e8 }).into()
}

pub fn accent_soft() -> Hsla {
    rgb(if dark() { 0x203a5c } else { 0xe8f1ff }).into()
}

pub fn danger() -> Hsla {
    rgb(0xc43e4b).into()
}

pub fn danger_soft() -> Hsla {
    rgb(if dark() { 0x4a252d } else { 0xffecee }).into()
}

pub fn folder() -> Hsla {
    rgb(0xe8a62d).into()
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
