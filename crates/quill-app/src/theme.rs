//! The dark palette from the design, in one place.

use gpui_kit::{Hsla, Rgba, rgb, rgba};

pub fn app_bg() -> Rgba {
    rgb(0x111318)
}

pub fn surface() -> Rgba {
    rgb(0x1a1d23)
}

pub fn raised() -> Rgba {
    rgb(0x23272f)
}

pub fn border() -> Rgba {
    rgb(0x2a2e37)
}

pub fn text() -> Rgba {
    rgb(0xe6e8ec)
}

pub fn text_soft() -> Rgba {
    rgb(0xc4c9d2)
}

pub fn muted() -> Rgba {
    rgb(0x9aa1ae)
}

pub fn accent() -> Rgba {
    rgb(0x7b9af0)
}

pub fn on_accent() -> Rgba {
    rgb(0x0e1630)
}

pub fn selected_bg() -> Rgba {
    rgb(0x22305a)
}

pub fn selection() -> Hsla {
    rgba(0x7b9af04d).into()
}

pub fn highlight() -> Hsla {
    rgba(0xfacc1559).into()
}

pub const UI_FONT: &str = ".SystemUIFont";
pub const PAGE_FONT: &str = "Georgia";
