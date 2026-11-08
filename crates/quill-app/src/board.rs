//! The whiteboard around the page: rendering board items and the tools that
//! create and move them. Board coordinates are page pixels with the origin
//! at the page's top centre (see `quill_store::BoardItem`).

use std::sync::Arc;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use quill_store::{BoardItem, ItemId, ItemKind};

use crate::editor::{EditorView, Mode};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Pen,
    Highlighter,
    Sticky,
    Shape,
    Text,
    Image,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 8] =
        [Tool::Select, Tool::Pen, Tool::Highlighter, Tool::Sticky, Tool::Shape, Tool::Text, Tool::Image, Tool::Eraser];

    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Pen => "Pen",
            Tool::Highlighter => "Highlighter",
            Tool::Sticky => "Sticky",
            Tool::Shape => "Shape",
            Tool::Text => "Text",
            Tool::Image => "Image",
            Tool::Eraser => "Eraser",
        }
    }
}

pub const NOTE_COLORS: [u32; 4] = [0xfde047, 0xf9a8d4, 0x93c5fd, 0x86efac];
const STICKY_SIZE: (f32, f32) = (200., 150.);
const PEN_WIDTH: f32 = 2.5;
const HIGHLIGHTER_WIDTH: f32 = 14.;
const HIGHLIGHTER_COLOR: u32 = 0xfacc15;

pub enum Gesture {
    Drawing { points: Vec<Point<Pixels>>, highlighter: bool },
    Shaping { start: Point<Pixels>, end: Point<Pixels> },
    Dragging { id: ItemId, grab: Point<Pixels>, start: (f32, f32) },
}

pub struct Editing {
    pub id: ItemId,
    pub state: Entity<TextareaState>,
    _subscription: Subscription,
}

impl EditorView {
    pub(crate) fn reload_board(&mut self) {
        match self.library.borrow().board(self.id) {
            Ok(items) => self.board = items,
            Err(err) => eprintln!("quill: loading the board failed: {err}"),
        }
    }

    /// Window position -> board coordinates.
    fn to_board(&self, p: Point<Pixels>) -> Point<Pixels> {
        p - self.board_origin
    }

    pub(crate) fn board_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Page {
            self.set_mode(Mode::Board, window, cx);
            return;
        }
        if self.mode != Mode::Board {
            return;
        }
        self.stop_editing_item(cx);
        self.selected_item = None;
        let p = self.to_board(event.position);
        match self.tool {
            Tool::Pen | Tool::Highlighter => {
                self.gesture = Some(Gesture::Drawing { points: vec![p], highlighter: self.tool == Tool::Highlighter })
            }
            Tool::Shape => self.gesture = Some(Gesture::Shaping { start: p, end: p }),
            Tool::Sticky => {
                let (w, h) = STICKY_SIZE;
                let kind = ItemKind::Sticky { text: String::new(), color: self.note_color };
                if let Some(id) = self.add_item(f32::from(p.x) - w / 2., f32::from(p.y) - 20., w, h, kind) {
                    self.edit_item(id, window, cx);
                }
            }
            Tool::Text => {
                if let Some(id) = self.add_item(
                    f32::from(p.x),
                    f32::from(p.y) - 12.,
                    260.,
                    60.,
                    ItemKind::Text { text: String::new() },
                ) {
                    self.edit_item(id, window, cx);
                }
            }
            Tool::Image => self.insert_image(p, cx),
            Tool::Select | Tool::Eraser => {}
        }
        cx.notify();
    }

    pub(crate) fn board_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.to_board(event.position);
        match &mut self.gesture {
            Some(Gesture::Drawing { points, .. }) => {
                if points.last().is_none_or(|last| (*last - p).magnitude() > 1.5) {
                    points.push(p);
                    cx.notify();
                }
            }
            Some(Gesture::Shaping { end, .. }) => {
                *end = p;
                cx.notify();
            }
            Some(Gesture::Dragging { id, grab, start }) => {
                let delta = p - *grab;
                let (id, start) = (*id, *start);
                if let Some(item) = self.board.iter_mut().find(|i| i.id == id) {
                    item.x = start.0 + f32::from(delta.x);
                    item.y = start.1 + f32::from(delta.y);
                    cx.notify();
                }
            }
            None => {}
        }
    }

    pub(crate) fn board_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.gesture.take() {
            Some(Gesture::Drawing { points, highlighter }) if points.len() > 1 => {
                let (min, max) = bounds_of(&points);
                let pad = if highlighter { HIGHLIGHTER_WIDTH } else { PEN_WIDTH };
                let origin = min - point(px(pad), px(pad));
                let rel = points.iter().map(|p| (f32::from(p.x - origin.x), f32::from(p.y - origin.y))).collect();
                let kind = ItemKind::Stroke {
                    points: rel,
                    color: if highlighter { HIGHLIGHTER_COLOR } else { self.pen_color() },
                    width: if highlighter { HIGHLIGHTER_WIDTH } else { PEN_WIDTH },
                };
                let size = max - min;
                self.add_item(
                    f32::from(origin.x),
                    f32::from(origin.y),
                    f32::from(size.x) + pad * 2.,
                    f32::from(size.y) + pad * 2.,
                    kind,
                );
            }
            Some(Gesture::Shaping { start, end }) => {
                let (min, max) = bounds_of(&[start, end]);
                let size = max - min;
                if size.x > px(8.) && size.y > px(8.) {
                    let kind = ItemKind::Shape { ellipse: false, color: self.pen_color() };
                    self.add_item(f32::from(min.x), f32::from(min.y), f32::from(size.x), f32::from(size.y), kind);
                }
            }
            Some(Gesture::Dragging { id, .. }) => {
                if let Some(item) = self.board.iter().find(|i| i.id == id).cloned() {
                    self.save_item(&item);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn item_mouse_down(&mut self, id: ItemId, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Board {
            return;
        }
        cx.stop_propagation();
        match self.tool {
            Tool::Eraser => self.remove_item(id, cx),
            Tool::Select | Tool::Sticky | Tool::Text | Tool::Shape | Tool::Image => {
                let editable = self
                    .board
                    .iter()
                    .find(|i| i.id == id)
                    .is_some_and(|i| matches!(i.kind, ItemKind::Sticky { .. } | ItemKind::Text { .. }));
                if event.click_count >= 2 && editable {
                    self.edit_item(id, window, cx);
                    return;
                }
                if self.editing.as_ref().is_some_and(|e| e.id == id) {
                    return;
                }
                self.stop_editing_item(cx);
                self.selected_item = Some(id);
                self.raise(id);
                if let Some(item) = self.board.iter().find(|i| i.id == id) {
                    self.gesture =
                        Some(Gesture::Dragging { id, grab: self.to_board(event.position), start: (item.x, item.y) });
                }
                cx.notify();
            }
            Tool::Pen | Tool::Highlighter => {
                // Draw over items too.
                cx.propagate();
            }
        }
    }

    /// Delete key on the board: removes the selected item. Returns whether
    /// it did anything.
    pub(crate) fn delete_selected_item(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selected_item.take() {
            Some(id) if self.mode == Mode::Board && self.editing.is_none() => {
                self.remove_item(id, cx);
                true
            }
            other => {
                self.selected_item = other;
                false
            }
        }
    }

    /// Undo on the board brings back the last removed item.
    pub(crate) fn undo_board(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.removed_items.pop() else { return false };
        if let Err(err) = self.library.borrow_mut().restore_item(id) {
            eprintln!("quill: restoring an item failed: {err}");
        }
        self.reload_board();
        cx.notify();
        true
    }

    fn remove_item(&mut self, id: ItemId, cx: &mut Context<Self>) {
        if let Err(err) = self.library.borrow_mut().remove_item(id) {
            eprintln!("quill: removing an item failed: {err}");
            return;
        }
        self.removed_items.push(id);
        self.board.retain(|i| i.id != id);
        cx.notify();
    }

    fn raise(&mut self, id: ItemId) {
        let top = self.board.iter().map(|i| i.z).max().unwrap_or(0);
        if let Some(item) = self.board.iter_mut().find(|i| i.id == id)
            && item.z < top
        {
            item.z = top + 1;
            let item = item.clone();
            self.board.sort_by_key(|i| (i.z, i.id));
            self.save_item(&item);
        }
    }

    fn add_item(&mut self, x: f32, y: f32, w: f32, h: f32, kind: ItemKind) -> Option<ItemId> {
        match self.library.borrow_mut().add_item(self.id, x, y, w, h, kind) {
            Ok(item) => {
                let id = item.id;
                self.board.push(item);
                Some(id)
            }
            Err(err) => {
                eprintln!("quill: adding to the board failed: {err}");
                None
            }
        }
    }

    fn save_item(&self, item: &BoardItem) {
        if let Err(err) = self.library.borrow_mut().update_item(item) {
            eprintln!("quill: saving a board item failed: {err}");
        }
    }

    fn pen_color(&self) -> u32 {
        // Pen strokes use a stronger version of the chosen note color so
        // they read on the dark board.
        match self.note_color {
            0xfde047 => 0xfacc15,
            0xf9a8d4 => 0xf472b6,
            0x93c5fd => 0x7b9af0,
            _ => 0x4ade80,
        }
    }

    pub(crate) fn edit_item(&mut self, id: ItemId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.board.iter().find(|i| i.id == id).and_then(|i| match &i.kind {
            ItemKind::Sticky { text, .. } | ItemKind::Text { text } => Some(text.clone()),
            _ => None,
        }) else {
            return;
        };
        self.stop_editing_item(cx);
        let state = cx.new(|cx| TextareaState::new(window, cx).default_value(text));
        let subscription = cx.subscribe_in(&state, window, move |this, state, event, _, cx| match event {
            InputEvent::Change => {
                let value = state.read(cx).value().to_string();
                if let Some(item) = this.board.iter_mut().find(|i| i.id == id) {
                    match &mut item.kind {
                        ItemKind::Sticky { text, .. } | ItemKind::Text { text } => *text = value,
                        _ => {}
                    }
                    let item = item.clone();
                    this.save_item(&item);
                }
            }
            InputEvent::Blur => this.stop_editing_item(cx),
            _ => {}
        });
        state.update(cx, |s, cx| s.focus(window, cx));
        self.editing = Some(Editing { id, state, _subscription: subscription });
        self.selected_item = Some(id);
        cx.notify();
    }

    pub(crate) fn stop_editing_item(&mut self, cx: &mut Context<Self>) {
        if let Some(editing) = self.editing.take() {
            // An empty new note or text box is dropped rather than left behind.
            let empty = self.board.iter().find(|i| i.id == editing.id).is_some_and(|i| match &i.kind {
                ItemKind::Sticky { text, .. } | ItemKind::Text { text } => text.trim().is_empty(),
                _ => false,
            });
            if empty {
                self.remove_item(editing.id, cx);
                self.removed_items.pop();
            }
            cx.notify();
        }
    }

    fn insert_image(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add image".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let mime = match ext.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => return,
            };
            let Ok(bytes) = std::fs::read(&path) else { return };
            let (w, h) = image_size(&bytes).unwrap_or((240, 180));
            let scale = (280. / w as f32).min(1.);
            this.update(cx, |this, cx| {
                let asset = match this.library.borrow_mut().add_asset(mime, &bytes) {
                    Ok(asset) => asset,
                    Err(err) => {
                        eprintln!("quill: storing the image failed: {err}");
                        return;
                    }
                };
                let (w, h) = (w as f32 * scale, h as f32 * scale);
                this.add_item(f32::from(at.x) - w / 2., f32::from(at.y) - h / 2., w, h, ItemKind::Image { asset });
                this.tool = Tool::Select;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn image(&mut self, asset: quill_store::AssetId) -> Option<Arc<Image>> {
        if let Some(image) = self.images.get(&asset) {
            return Some(image.clone());
        }
        let (mime, bytes) = self.library.borrow().asset(asset).ok().flatten()?;
        let image = Arc::new(Image::from_bytes(ImageFormat::from_mime_type(&mime)?, bytes));
        self.images.insert(asset, image.clone());
        Some(image)
    }

    /// The board layer: a zero-size anchor at the page's top centre with
    /// every item absolutely positioned around it.
    pub(crate) fn render_board(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let interactive = self.mode == Mode::Board;
        let entity = cx.entity();
        let items: Vec<BoardItem> = self.board.clone();
        let mut children: Vec<AnyElement> = Vec::new();
        for item in items {
            let selected = interactive && self.selected_item == Some(item.id);
            let editing = self.editing.as_ref().filter(|e| e.id == item.id).map(|e| e.state.clone());
            let id = item.id;
            let base = div()
                .id(("item", id as u64))
                .absolute()
                .left(px(item.x))
                .top(px(item.y))
                .w(px(item.w))
                .h(px(item.h))
                .when(selected, |d| d.border_2().border_color(theme::accent()))
                .when(interactive, |d| {
                    d.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev, w, cx| this.item_mouse_down(id, ev, w, cx)),
                    )
                });
            let element = match &item.kind {
                ItemKind::Sticky { text, color } => base
                    .p(px(14.))
                    .bg(rgb(*color))
                    .shadow_md()
                    .text_color(rgb(0x1f1d18))
                    .font_family("Noteworthy")
                    .text_size(px(18.))
                    .line_height(px(24.))
                    .overflow_hidden()
                    .child(match editing {
                        Some(state) => Textarea::new(&state).appearance(false).h_full().into_any_element(),
                        None => div().child(text.clone()).into_any_element(),
                    })
                    .into_any_element(),
                ItemKind::Text { text } => base
                    .p(px(4.))
                    .text_color(theme::text())
                    .font_family(theme::PAGE_FONT)
                    .text_size(px(16.))
                    .child(match editing {
                        Some(state) => Textarea::new(&state).appearance(false).h_full().into_any_element(),
                        None => div().child(text.clone()).into_any_element(),
                    })
                    .into_any_element(),
                ItemKind::Shape { ellipse, color } => base
                    .border_2()
                    .border_color(rgb(*color))
                    .when(*ellipse, |d| d.rounded_full())
                    .when(!*ellipse, |d| d.rounded(px(6.)))
                    .into_any_element(),
                ItemKind::Stroke { points, color, width } => {
                    let (points, color, width) = (points.clone(), *color, *width);
                    base.child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| paint_stroke(bounds.origin, &points, color, width, window),
                        )
                        .size_full(),
                    )
                    .into_any_element()
                }
                ItemKind::Image { asset } => match self.image(*asset) {
                    Some(image) => base.child(img(image).size_full().rounded(px(6.))).into_any_element(),
                    None => base.bg(theme::raised()).into_any_element(),
                },
            };
            children.push(element);
        }

        // The stroke or shape being drawn right now.
        match &self.gesture {
            Some(Gesture::Drawing { points, highlighter }) => {
                let points: Vec<(f32, f32)> = points.iter().map(|p| (f32::from(p.x), f32::from(p.y))).collect();
                let (color, width) =
                    if *highlighter { (HIGHLIGHTER_COLOR, HIGHLIGHTER_WIDTH) } else { (self.pen_color(), PEN_WIDTH) };
                children.push(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| paint_stroke(bounds.origin, &points, color, width, window),
                    )
                    .absolute()
                    .size_0()
                    .into_any_element(),
                );
            }
            Some(Gesture::Shaping { start, end }) => {
                let (min, max) = bounds_of(&[*start, *end]);
                children.push(
                    div()
                        .absolute()
                        .left(min.x)
                        .top(min.y)
                        .w(max.x - min.x)
                        .h(max.y - min.y)
                        .border_2()
                        .border_color(rgb(self.pen_color()))
                        .rounded(px(6.))
                        .into_any_element(),
                );
            }
            _ => {}
        }

        div()
            .absolute()
            .left(relative(0.5))
            .top(px(crate::editor::PAGE_TOP))
            .size_0()
            .when(self.mode == Mode::Page, |d| d.opacity(0.28))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| this.board_origin = bounds.origin);
                    },
                    |_, _, _, _| {},
                )
                .size_0(),
            )
            .children(children)
    }

    pub(crate) fn tool_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(6.))
            .rounded(px(14.))
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .shadow_lg()
            .children(Tool::ALL.into_iter().map(|tool| {
                let active = self.tool == tool;
                div()
                    .id(tool.label())
                    .px(px(10.))
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .rounded(px(10.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(active, |d| d.bg(theme::selected_bg()).text_color(theme::accent()))
                    .hover(|s| s.bg(theme::raised()))
                    .child(tool.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tool = tool;
                        this.set_mode(Mode::Board, window, cx);
                    }))
            }))
            .child(div().w(px(1.)).h(px(26.)).mx(px(6.)).bg(theme::border()))
            .children(NOTE_COLORS.into_iter().map(|color| {
                let active = self.note_color == color;
                div()
                    .id(("color", color as u64))
                    .size(px(22.))
                    .mx(px(3.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(rgb(color))
                    .when(active, |d| d.border_2().border_color(theme::text()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.note_color = color;
                        cx.notify();
                    }))
            }))
    }
}

fn paint_stroke(origin: Point<Pixels>, points: &[(f32, f32)], color: u32, width: f32, window: &mut Window) {
    let Some(&(x0, y0)) = points.first() else { return };
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(origin + point(px(x0), px(y0)));
    for &(x, y) in &points[1..] {
        path.line_to(origin + point(px(x), px(y)));
    }
    if let Ok(path) = path.build() {
        let alpha = if width >= HIGHLIGHTER_WIDTH { 0x66 } else { 0xff };
        window.paint_path(path, rgba((color << 8) | alpha));
    }
}

fn bounds_of(points: &[Point<Pixels>]) -> (Point<Pixels>, Point<Pixels>) {
    let mut min = points[0];
    let mut max = points[0];
    for p in points {
        min = point(min.x.min(p.x), min.y.min(p.y));
        max = point(max.x.max(p.x), max.y.max(p.y));
    }
    (min, max)
}

/// Width and height from a PNG, GIF or JPEG header, for placing images at
/// their aspect ratio without decoding them.
fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    if bytes.starts_with(b"\x89PNG") && bytes.len() >= 24 {
        return Some((be32(&bytes[16..20]), be32(&bytes[20..24])));
    }
    if bytes.starts_with(b"GIF") && bytes.len() >= 10 {
        return Some((
            u16::from_le_bytes([bytes[6], bytes[7]]) as u32,
            u16::from_le_bytes([bytes[8], bytes[9]]) as u32,
        ));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xff {
                return None;
            }
            let marker = bytes[i + 1];
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((w, h));
            }
            i += 2 + len;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_image_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(super::image_size(&png), Some((640, 480)));
        assert_eq!(super::image_size(b"GIF89a\x10\x00\x20\x00"), Some((16, 32)));
        let jpeg = [0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x01, 0x00, 0x02, 0x00, 0x03, 0, 0];
        assert_eq!(super::image_size(&jpeg), Some((512, 256)));
        assert_eq!(super::image_size(b"nope"), None);
    }
}
