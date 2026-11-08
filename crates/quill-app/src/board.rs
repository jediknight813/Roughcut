//! The whiteboard around the page: rendering board items and the tools that
//! create and move them. Board coordinates are page pixels at writing size
//! with the origin at the page's top centre (see `quill_store::BoardItem`);
//! on screen they are multiplied by the editor's current zoom.

use std::sync::Arc;
use std::time::Duration;

use crate::board_input::{BoardInput, BoardInputEvent};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use quill_doc::Marks;
use quill_store::{BoardItem, ItemId, ItemKind};

use crate::anim;
use crate::assets::icon;
use crate::editor::{EditorView, Mode};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Pen,
    Highlighter,
    Shape,
    Text,
    Image,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 7] =
        [Tool::Select, Tool::Pen, Tool::Highlighter, Tool::Shape, Tool::Text, Tool::Image, Tool::Eraser];

    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Pen => "Pen",
            Tool::Highlighter => "Highlighter",
            Tool::Shape => "Shape",
            Tool::Text => "Text",
            Tool::Image => "Image",
            Tool::Eraser => "Eraser",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tool::Select => "mouse-pointer-2",
            Tool::Pen => "pen-line",
            Tool::Highlighter => "highlighter",
            Tool::Shape => "square",
            Tool::Text => "type",
            Tool::Image => "image",
            Tool::Eraser => "eraser",
        }
    }
}

pub const BOARD_COLORS: [u32; 4] = [0xfde68a, 0xfbcfe8, 0xbfdbfe, 0xbbf7d0];
const PEN_WIDTH: f32 = 2.5;
const HIGHLIGHTER_WIDTH: f32 = 14.;
const HIGHLIGHTER_COLOR: u32 = 0xfacc15;

pub enum Gesture {
    Drawing { points: Vec<Point<Pixels>>, highlighter: bool },
    Shaping { start: Point<Pixels>, end: Point<Pixels> },
    Dragging { id: ItemId, grab: Point<Pixels>, start: (f32, f32), edit_on_click: bool },
    Resizing { id: ItemId, grab: Point<Pixels>, original: (f32, f32, f32, f32), edges: (i8, i8) },
}

pub struct Editing {
    pub id: ItemId,
    pub state: Entity<BoardInput>,
    _subscription: Subscription,
}

impl EditorView {
    pub fn editing_text_input(&self) -> Option<Entity<BoardInput>> {
        self.editing.as_ref().map(|e| e.state.clone())
    }

    pub(crate) fn reload_board(&mut self) {
        match self.library.borrow().board(self.id) {
            Ok(items) => self.board = items,
            Err(err) => eprintln!("roughcut: loading the board failed: {err}"),
        }
    }

    /// Window position -> board coordinates (unscaled page pixels).
    fn to_board(&self, p: Point<Pixels>) -> Point<Pixels> {
        let z = self.page_scale().max(0.01);
        let d = p - self.board_origin;
        point(d.x / z, d.y / z)
    }

    pub(crate) fn board_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Page {
            self.set_mode(Mode::Board, window, cx);
            if event.click_count < 2 {
                return;
            }
        }
        if self.mode != Mode::Board {
            return;
        }
        // The board manages focus itself; stop the editor's container from
        // grabbing it afterwards (which would steal it from a new note).
        window.prevent_default();
        window.focus(&self.focus_handle, cx);
        self.stop_editing_item(cx);
        self.selected_item = None;
        let p = self.to_board(event.position);
        if event.click_count >= 2 {
            self.gesture = None;
            self.create_board_text(p, window, cx);
            cx.notify();
            return;
        }
        match self.tool {
            Tool::Pen | Tool::Highlighter => {
                self.gesture = Some(Gesture::Drawing { points: vec![p], highlighter: self.tool == Tool::Highlighter })
            }
            Tool::Shape => self.gesture = Some(Gesture::Shaping { start: p, end: p }),
            Tool::Text => self.create_board_text(p, window, cx),
            Tool::Image => self.insert_image(p, cx),
            Tool::Select | Tool::Eraser => {}
        }
        cx.notify();
    }

    fn create_board_text(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let kind = ItemKind::Text { text: String::new(), style: Default::default() };
        if let Some(id) = self.add_item(f32::from(at.x), f32::from(at.y) - 12., 260., 60., kind) {
            self.tool = Tool::Select;
            self.edit_item(id, window, cx);
        }
    }

    fn choose_board_color(&mut self, color: u32, cx: &mut Context<Self>) {
        self.board_color = color;
        if let Some(mut item) = self.board.iter().find(|i| Some(i.id) == self.selected_item).cloned()
            && let ItemKind::Shape { color: fill, .. } = &mut item.kind
        {
            *fill = color;
            match self.library.borrow_mut().update_item(&item) {
                Ok(()) => {
                    if let Some(current) = self.board.iter_mut().find(|i| i.id == item.id) {
                        *current = item;
                    }
                }
                Err(err) => eprintln!("roughcut: changing the shape color failed: {err}"),
            }
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
            Some(Gesture::Dragging { id, grab, start, .. }) => {
                let delta = p - *grab;
                let (id, start) = (*id, *start);
                if let Some(item) = self.board.iter_mut().find(|i| i.id == id) {
                    item.x = start.0 + f32::from(delta.x);
                    item.y = start.1 + f32::from(delta.y);
                    cx.notify();
                }
            }
            Some(Gesture::Resizing { id, grab, original: (x, y, w, h), edges: (ex, ey) }) => {
                let delta = p - *grab;
                if let Some(item) = self.board.iter_mut().find(|i| i.id == *id) {
                    let dx = f32::from(delta.x);
                    let dy = f32::from(delta.y);
                    if *ex < 0 {
                        item.w = (*w - dx).max(48.);
                        item.x = *x + *w - item.w;
                    }
                    if *ex > 0 {
                        item.w = (*w + dx).max(48.);
                    }
                    if *ey < 0 {
                        item.h = (*h - dy).max(40.);
                        item.y = *y + *h - item.h;
                    }
                    if *ey > 0 {
                        item.h = (*h + dy).max(40.);
                    }
                    cx.notify();
                }
            }
            None => {}
        }
    }

    pub(crate) fn board_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
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
                    let kind = ItemKind::Shape {
                        ellipse: false,
                        color: self.board_color,
                        text: String::new(),
                        style: quill_store::BoardTextStyle::shape(),
                    };
                    self.selected_item =
                        self.add_item(f32::from(min.x), f32::from(min.y), f32::from(size.x), f32::from(size.y), kind);
                    self.tool = Tool::Select;
                }
            }
            Some(Gesture::Dragging { id, grab, edit_on_click, .. }) => {
                if let Some(item) = self.board.iter().find(|i| i.id == id).cloned() {
                    self.save_item(&item);
                }
                if edit_on_click && (self.to_board(event.position) - grab).magnitude() < 3. {
                    self.edit_item(id, window, cx);
                }
            }
            Some(Gesture::Resizing { id, .. }) => {
                if let Some(item) = self.board.iter_mut().find(|i| i.id == id) {
                    grow_text_item(item, window);
                    let item = item.clone();
                    self.save_item(&item);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn item_mouse_down(&mut self, id: ItemId, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Page {
            self.set_mode(Mode::Board, window, cx);
            self.tool = Tool::Select;
            if self.board.iter().any(|i| i.id == id && matches!(i.kind, ItemKind::Shape { .. })) {
                cx.stop_propagation();
                window.prevent_default();
                self.edit_item(id, window, cx);
                return;
            }
        }
        if self.mode != Mode::Board {
            return;
        }
        match self.tool {
            Tool::Eraser => {
                cx.stop_propagation();
                self.remove_item(id, cx);
            }
            Tool::Pen | Tool::Highlighter => {
                // Let the board start a stroke over the item.
            }
            Tool::Select | Tool::Text | Tool::Shape | Tool::Image => {
                cx.stop_propagation();
                if self.editing.as_ref().is_some_and(|e| e.id == id) {
                    // Clicks inside the note being edited belong to its text box.
                    return;
                }
                window.prevent_default();
                window.focus(&self.focus_handle, cx);
                let editable =
                    self.board.iter().find(|i| i.id == id).is_some_and(|i| matches!(i.kind, ItemKind::Text { .. }));
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
                    if let ItemKind::Shape { color, .. } = &item.kind {
                        self.board_color = *color;
                    }
                    self.gesture = Some(Gesture::Dragging {
                        id,
                        grab: self.to_board(event.position),
                        start: (item.x, item.y),
                        edit_on_click: matches!(item.kind, ItemKind::Shape { .. }),
                    });
                }
                cx.notify();
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
            eprintln!("roughcut: restoring an item failed: {err}");
        }
        self.reload_board();
        cx.notify();
        true
    }

    fn remove_item(&mut self, id: ItemId, cx: &mut Context<Self>) {
        if let Err(err) = self.library.borrow_mut().remove_item(id) {
            eprintln!("roughcut: removing an item failed: {err}");
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
                eprintln!("roughcut: adding to the board failed: {err}");
                None
            }
        }
    }

    fn save_item(&self, item: &BoardItem) {
        if let Err(err) = self.library.borrow_mut().update_item(item) {
            eprintln!("roughcut: saving a board item failed: {err}");
        }
    }

    /// Pen strokes use a stronger version of the chosen board colour so they
    /// read on the dark board.
    fn pen_color(&self) -> u32 {
        match self.board_color {
            0xfde68a => 0xfacc15,
            0xfbcfe8 => 0xf472b6,
            0xbfdbfe => 0x7b9af0,
            _ => 0x4ade80,
        }
    }

    pub(crate) fn edit_item(&mut self, id: ItemId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, style)) =
            self.board.iter().find(|i| i.id == id).and_then(|i| i.kind.text()).map(|(t, s)| (t.to_owned(), s.clone()))
        else {
            return;
        };
        self.stop_editing_item(cx);
        let state = cx.new(|cx| BoardInput::new(text, style, window, cx));
        let subscription = cx.subscribe_in(&state, window, move |this, state, event, window, cx| match event {
            BoardInputEvent::Change => {
                let value = state.read(cx).value().to_string();
                if let Some(item) = this.board.iter_mut().find(|i| i.id == id) {
                    if let Some((text, _)) = item.kind.text_mut() {
                        *text = value;
                    }
                    grow_text_item(item, window);
                    let item = item.clone();
                    this.save_item(&item);
                    this.sync_board_edit_style(cx);
                    cx.notify();
                }
            }
            BoardInputEvent::Blur => this.stop_editing_item(cx),
        });
        state.update(cx, |s, cx| s.focus(window, cx));
        self.editing = Some(Editing { id, state, _subscription: subscription });
        self.selected_item = Some(id);
        self.sync_board_edit_style(cx);
        cx.notify();
    }

    pub(crate) fn sync_board_edit_style(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = &self.editing else {
            return;
        };
        let Some((_, style)) = self.board.iter().find(|i| i.id == editing.id).and_then(|i| i.kind.text()) else {
            return;
        };
        editing.state.update(cx, |s, cx| s.set_style(style.clone(), cx));
    }

    pub(crate) fn update_board_style(
        &mut self,
        change: impl FnOnce(&mut quill_store::BoardTextStyle),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.board.iter_mut().find(|i| Some(i.id) == self.selected_item) else {
            return;
        };
        let Some((_, style)) = item.kind.text_mut() else {
            return;
        };
        change(style);
        grow_text_item(item, window);
        let item = item.clone();
        self.save_item(&item);
        self.sync_board_edit_style(cx);
        cx.notify();
    }

    fn resize_item(
        &mut self,
        id: ItemId,
        edges: (i8, i8),
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.prevent_default();
        self.stop_editing_item(cx);
        window.focus(&self.focus_handle, cx);
        if let Some(item) = self.board.iter().find(|i| i.id == id) {
            self.gesture = Some(Gesture::Resizing {
                id,
                edges,
                grab: self.to_board(event.position),
                original: (item.x, item.y, item.w, item.h),
            });
        }
        cx.notify();
    }

    pub(crate) fn stop_editing_item(&mut self, cx: &mut Context<Self>) {
        if let Some(editing) = self.editing.take() {
            // An empty new note or text box is dropped rather than left behind.
            let empty = self.board.iter().find(|i| i.id == editing.id).is_some_and(|i| match &i.kind {
                ItemKind::Text { text, .. } => text.trim().is_empty(),
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
            this.update(cx, |this, cx| this.insert_image_from(&path, at, cx)).ok();
        })
        .detach();
    }

    /// Adds the image at `path` to the board, centred on `at` (board coordinates).
    pub fn insert_image_from(&mut self, path: &std::path::Path, at: Point<Pixels>, cx: &mut Context<Self>) {
        {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let mime = match ext.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => return,
            };
            let Ok(bytes) = std::fs::read(path) else { return };
            let (w, h) = image_size(&bytes).unwrap_or((240, 180));
            let scale = (280. / w as f32).min(1.);
            let asset = match self.library.borrow_mut().add_asset(mime, &bytes) {
                Ok(asset) => asset,
                Err(err) => {
                    eprintln!("roughcut: storing the image failed: {err}");
                    return;
                }
            };
            let (w, h) = (w as f32 * scale, h as f32 * scale);
            self.add_item(f32::from(at.x) - w / 2., f32::from(at.y) - h / 2., w, h, ItemKind::Image { asset });
            self.tool = Tool::Select;
            cx.notify();
        }
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
    /// every item absolutely positioned (and scaled) around it.
    pub(crate) fn render_board(
        &mut self,
        opacity: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let interactive = self.mode == Mode::Board;
        let z = self.page_scale();
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
                .left(px(item.x * z))
                .top(px(item.y * z))
                .w(px(item.w * z))
                .h(px(item.h * z))
                .when(self.mode != Mode::Focus, |d| {
                    d.cursor(if self.tool == Tool::Eraser {
                        CursorStyle::OperationNotAllowed
                    } else {
                        CursorStyle::OpenHand
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev, w, cx| this.item_mouse_down(id, ev, w, cx)),
                    )
                });
            let element = match &item.kind {
                ItemKind::Text { .. } | ItemKind::Shape { .. } => {
                    let (text, style) = item.kind.text().unwrap();
                    let shape = matches!(item.kind, ItemKind::Shape { .. });
                    let padding = if shape { 12. } else { 0. };
                    let text_height = text_height(text, style, (item.w - padding * 2.).max(16.), window);
                    let top = if shape { ((item.h - text_height) / 2.).max(padding) } else { 0. };
                    let size = style.typography.size.unwrap_or(16) as f32;
                    let color = style.typography.color.map(rgb).unwrap_or_else(theme::text_soft);
                    base.when_some(
                        match item.kind {
                            ItemKind::Shape { color, .. } => Some(color),
                            _ => None,
                        },
                        |d, color| d.bg(rgb(color)),
                    )
                    .when(shape, |d| d.rounded(px(8. * z)))
                    .when(shape && selected, |d| d.shadow(item_shadow(true)))
                    .child(
                        div()
                            .absolute()
                            .left(px(padding * z))
                            .top(px(top * z))
                            .w(px((item.w - padding * 2.).max(16.) * z))
                            .h(px((text_height + 2.) * z))
                            .font_family(crate::editor::formatting::font_family(style.typography.typeface))
                            .text_size(px(size * z))
                            .line_height(px(size * 1.5 * z))
                            .text_color(color)
                            .text_align(crate::editor::formatting::alignment(style.typography.alignment))
                            .when(style.marks.contains(Marks::BOLD), |d| d.font_weight(FontWeight::BOLD))
                            .when(style.marks.contains(Marks::ITALIC), |d| d.italic())
                            .child(match editing {
                                Some(state) => {
                                    state.update(cx, |s, _| s.set_scale(z));
                                    state.into_any_element()
                                }
                                None => StyledText::new(text.to_owned())
                                    .with_highlights([(0..text.len(), text_highlight(style))])
                                    .into_any_element(),
                            }),
                    )
                    .into_any_element()
                }
                ItemKind::Stroke { points, color, width } => {
                    let (points, color, width) = (points.clone(), *color, *width);
                    base.when(selected, |d| d.border_1().border_color(rgba(0x7b9af080)).rounded(px(4.)))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    paint_stroke(bounds.origin, &points, z, color, width, window)
                                },
                            )
                            .size_full(),
                        )
                        .into_any_element()
                }
                ItemKind::Image { asset } => match self.image(*asset) {
                    Some(image) => base
                        .rounded(px(8.))
                        .overflow_hidden()
                        .shadow(item_shadow(selected))
                        .when(selected, |d| d.border_2().border_color(theme::accent()))
                        .child(img(image).size_full())
                        .into_any_element(),
                    None => base.bg(theme::raised()).rounded(px(8.)).into_any_element(),
                },
            };
            let resizable = selected
                && matches!(item.kind, ItemKind::Text { .. } | ItemKind::Shape { .. } | ItemKind::Image { .. });
            let mut handles = Vec::new();
            if resizable {
                for (index, (ex, ey)) in
                    [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].into_iter().enumerate()
                {
                    let edge = (ex, ey);
                    let (left, top, width, height) = if ex == 0 {
                        (item.x * z + 6., (item.y + if ey < 0 { 0. } else { item.h }) * z - 4., item.w * z - 12., 8.)
                    } else if ey == 0 {
                        ((item.x + if ex < 0 { 0. } else { item.w }) * z - 4., item.y * z + 6., 8., item.h * z - 12.)
                    } else {
                        (
                            (item.x + if ex < 0 { 0. } else { item.w }) * z - 4.,
                            (item.y + if ey < 0 { 0. } else { item.h }) * z - 4.,
                            8.,
                            8.,
                        )
                    };
                    handles.push(
                        div()
                            .id(("resize", id as usize * 8 + index))
                            .absolute()
                            .left(px(left))
                            .top(px(top))
                            .w(px(width.max(8.)))
                            .h(px(height.max(8.)))
                            .cursor(if ex == 0 {
                                CursorStyle::ResizeUpDown
                            } else if ey == 0 {
                                CursorStyle::ResizeLeftRight
                            } else {
                                if ex == ey {
                                    CursorStyle::ResizeUpLeftDownRight
                                } else {
                                    CursorStyle::ResizeUpRightDownLeft
                                }
                            })
                            .when(ex != 0 && ey != 0, |d| d.bg(theme::accent()).rounded(px(2.)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.resize_item(id, edge, event, window, cx)
                                }),
                            )
                            .into_any_element(),
                    );
                }
            }
            // New items pop in; existing ones are already settled.
            children.push(
                div()
                    .absolute()
                    .child(element)
                    .with_animation(("appear", id as u64), Animation::new(Duration::from_millis(260)), |el, t| {
                        el.opacity(anim::ease_out(t))
                    })
                    .into_any_element(),
            );
            children.extend(handles);
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
                        move |bounds, _, window, _| paint_stroke(bounds.origin, &points, z, color, width, window),
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
                        .left(min.x * z)
                        .top(min.y * z)
                        .w((max.x - min.x) * z)
                        .h((max.y - min.y) * z)
                        .border_2()
                        .bg(rgb(self.board_color))
                        .border_color(theme::accent())
                        .rounded(px(8. * z))
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
            .opacity(opacity)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, cx| this.record_board_origin(bounds.origin, cx));
                    },
                    |_, _, _, _| {},
                )
                .size_0(),
            )
            .children(children)
    }

    /// A fixed tool rail beside the canvas, shared by Page and Board modes.
    pub(crate) fn tool_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let board = self.mode == Mode::Board;
        let divider = || div().flex_none().w(px(32.)).h(px(1.)).my(px(8.)).bg(theme::border());
        div()
            .id("board-tools")
            .block_mouse_except_scroll()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(76.))
            .h_full()
            .min_h_0()
            .flex_none()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .p(px(10.))
            .bg(theme::surface())
            .border_r_1()
            .border_color(theme::border())
            .children(Tool::ALL.into_iter().map(|tool| {
                let active = board && self.tool == tool;
                div()
                    .id(tool.label())
                    .size(px(38.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::selected_bg()))
                    .when(!active, |d| d.hover(|s| s.bg(theme::raised())))
                    .child(icon(tool.icon()).size(px(18.)).text_color(if active {
                        theme::accent()
                    } else {
                        theme::text_soft()
                    }))
                    .tooltip(move |window, cx| Tooltip::new(tool.label()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tool = tool;
                        this.set_mode(Mode::Board, window, cx);
                    }))
            }))
            .child(divider())
            .child(div().w(px(54.)).flex_none().flex().flex_wrap().gap(px(2.)).children(BOARD_COLORS.into_iter().map(
                |color| {
                    let active = self.board_color == color;
                    div()
                        .id(("color", color as u64))
                        .size(px(26.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .when(active, |d| d.border_2().border_color(theme::text_soft()))
                        .child(div().size(px(18.)).rounded_full().bg(rgb(color)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose_board_color(color, cx);
                        }))
                },
            )))
            .child(divider())
            .child(
                div()
                    .id("zoom-in")
                    .size(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(17.))
                    .text_color(theme::text_soft())
                    .hover(|s| s.bg(theme::raised()))
                    .child("+")
                    .tooltip(|window, cx| Tooltip::new("Zoom in (⌘=)").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_step(true, cx))),
            )
            .child(
                div()
                    .id("zoom-reset")
                    .w(px(46.))
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(theme::text_soft())
                    .hover(|s| s.bg(theme::raised()))
                    .child(format!("{}%", (self.zoom.target() * 100.).round() as i32))
                    .tooltip(|window, cx| Tooltip::new("Actual size (⌘0)").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_to(1., None, true, cx))),
            )
            .child(
                div()
                    .id("zoom-out")
                    .size(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(17.))
                    .text_color(theme::text_soft())
                    .hover(|s| s.bg(theme::raised()))
                    .child("−")
                    .tooltip(|window, cx| Tooltip::new("Zoom out (⌘−)").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_step(false, cx))),
            )
            .into_any_element()
    }
}

/// Read-only access and tool selection for integration tests, which drive
/// the board with real mouse and keyboard events.
#[doc(hidden)]
impl EditorView {
    pub fn board_items(&self) -> &[BoardItem] {
        &self.board
    }

    pub fn editing_item(&self) -> Option<ItemId> {
        self.editing.as_ref().map(|e| e.id)
    }

    pub fn selected(&self) -> Option<ItemId> {
        self.selected_item
    }

    pub fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        cx.notify();
    }

    pub fn active_tool(&self) -> Tool {
        self.tool
    }

    /// Where a board point is on screen, as of the last frame.
    pub fn board_to_window(&self, x: f32, y: f32) -> Point<Pixels> {
        let z = self.page_scale();
        self.board_origin + point(px(x * z), px(y * z))
    }

    pub fn zoom_level(&self) -> f32 {
        self.zoom.value()
    }
}

fn item_shadow(lifted: bool) -> Vec<BoxShadow> {
    let (y, blur, alpha) = if lifted { (14., 28., 0.55) } else { (6., 14., 0.45) };
    vec![BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }]
}

fn paint_stroke(origin: Point<Pixels>, points: &[(f32, f32)], zoom: f32, color: u32, width: f32, window: &mut Window) {
    let Some(&(x0, y0)) = points.first() else { return };
    let at = |x: f32, y: f32| origin + point(px(x * zoom), px(y * zoom));
    let mut path = PathBuilder::stroke(px(width * zoom));
    path.move_to(at(x0, y0));
    // Smooth through midpoints so freehand strokes don't look jagged.
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        path.curve_to(at((a.0 + b.0) / 2., (a.1 + b.1) / 2.), at(a.0, a.1));
    }
    if let Some(&(x, y)) = points.last() {
        path.line_to(at(x, y));
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

fn text_highlight(style: &quill_store::BoardTextStyle) -> HighlightStyle {
    HighlightStyle {
        color: style.typography.color.map(|c| rgb(c).into()),
        font_weight: style.marks.contains(Marks::BOLD).then_some(FontWeight::BOLD),
        font_style: style.marks.contains(Marks::ITALIC).then_some(FontStyle::Italic),
        underline: style.marks.contains(Marks::UNDERLINE).then_some(UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        }),
        strikethrough: style
            .marks
            .contains(Marks::STRIKE)
            .then_some(StrikethroughStyle { thickness: px(1.), color: None }),
        background_color: style.marks.contains(Marks::HIGHLIGHT).then(|| rgba(0xd9ad3655).into()),
        ..Default::default()
    }
}

fn text_height(text: &str, style: &quill_store::BoardTextStyle, width: f32, window: &mut Window) -> f32 {
    let size = style.typography.size.unwrap_or(16) as f32;
    let mut font = font(crate::editor::formatting::font_family(style.typography.typeface));
    if style.marks.contains(Marks::BOLD) {
        font.weight = FontWeight::BOLD;
    }
    if style.marks.contains(Marks::ITALIC) {
        font.style = FontStyle::Italic;
    }
    let run = TextRun {
        len: text.len(),
        font,
        color: theme::text().into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_text(text.to_owned().into(), px(size), &[run], Some(px(width.max(16.))), None)
        .map(|lines| lines.iter().map(|l| f32::from(l.size(px(size * 1.5)).height)).sum::<f32>().max(size * 1.5))
        .unwrap_or(size * 1.5)
}

fn grow_text_item(item: &mut BoardItem, window: &mut Window) {
    if let Some((text, style)) = item.kind.text() {
        let padding = if matches!(item.kind, ItemKind::Shape { .. }) { 24. } else { 0. };
        let height = text_height(text, style, item.w - padding, window) + padding + 2.;
        item.h = item.h.max(height);
    }
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
