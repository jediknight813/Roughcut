//! Icons (Lucide, ISC licence — see assets/icons/LICENSE), embedded so the
//! app needs nothing from disk or network. Anything else falls through to
//! GPUI Kit's built-in component icons.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal,)*) => {
        &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*]
    };
}

static ICONS: &[(&str, &[u8])] = icons!(
    "bold",
    "check",
    "chevron-left",
    "clock",
    "download",
    "eraser",
    "file-clock",
    "file-plus",
    "file-text",
    "folder-open",
    "heading-1",
    "heading-2",
    "highlighter",
    "image",
    "italic",
    "layers",
    "list-ordered",
    "list",
    "mouse-pointer-2",
    "music",
    "pause",
    "pen-line",
    "pilcrow",
    "play",
    "plus",
    "redo-2",
    "rotate-ccw",
    "scan-text",
    "search",
    "share",
    "skip-back",
    "skip-forward",
    "sparkles",
    "square",
    "sticky-note",
    "strikethrough",
    "text-quote",
    "type",
    "underline",
    "undo-2",
    "x",
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ICONS.iter().find(|(p, _)| *p == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::Assets.list(path)?;
        all.extend(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)));
        Ok(all)
    }
}

/// An icon by Lucide name, tinted with the current text colour.
pub fn icon(name: &'static str) -> gpui_kit::Svg {
    gpui_kit::svg().path(SharedString::from(format!("icons/{name}.svg")))
}
