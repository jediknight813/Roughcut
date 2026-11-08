//! Canvas text editing uses the same text layout for painting, selection and IME.
//! No form frame or padding is introduced around the text.
use crate::editor::*;
use crate::page::{PageLayout, canvas_layout};
use crate::theme;
use gpui_kit::prelude::*;
use gpui_kit::*;
use quill_doc::{Document, Pos};
use quill_store::BoardTextStyle;
use std::ops::Range;
use std::time::{Duration, Instant};

pub enum BoardInputEvent {
    Change,
    Blur,
}
#[derive(Clone)]
struct Snapshot {
    doc: Document,
    anchor: Pos,
    head: Pos,
}

pub struct BoardInput {
    focus: FocusHandle,
    doc: Document,
    anchor: Pos,
    head: Pos,
    marked: Option<(Pos, Pos)>,
    selecting: bool,
    style: BoardTextStyle,
    layout: Option<PageLayout>,
    cached: Option<(Document, Pixels, f32, PageLayout)>,
    scale: f32,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_typing: Option<Instant>,
    _blur: Subscription,
}
impl EventEmitter<BoardInputEvent> for BoardInput {}

impl BoardInput {
    pub fn new(text: String, style: BoardTextStyle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut doc = Document::new("");
        doc.insert_text(Pos::default(), &text);
        let end = doc.end();
        let focus = cx.focus_handle();
        let blur = cx.on_blur(&focus, window, |_, _, cx| cx.emit(BoardInputEvent::Blur));
        let mut input = Self {
            focus,
            doc,
            anchor: end,
            head: end,
            marked: None,
            selecting: false,
            style: style.clone(),
            layout: None,
            cached: None,
            scale: 1.,
            undo: Vec::new(),
            redo: Vec::new(),
            last_typing: None,
            _blur: blur,
        };
        input.set_style(style, cx);
        input
    }
    pub fn value(&self) -> String {
        self.doc.plain_text()
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }
    pub fn set_style(&mut self, style: BoardTextStyle, cx: &mut Context<Self>) {
        if self.style != style {
            self.style = style;
        }
        for block in &mut self.doc.blocks {
            block.style = self.style.typography.clone();
            for span in &mut block.spans {
                span.marks = self.style.marks;
            }
        }
        cx.notify();
    }
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }
    fn selection(&self) -> (Pos, Pos) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot { doc: self.doc.clone(), anchor: self.anchor, head: self.head }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.set_style(self.style.clone(), cx);
        cx.emit(BoardInputEvent::Change);
        cx.notify();
    }
    fn replace(&mut self, text: &str, cx: &mut Context<Self>) {
        let typing = !text.is_empty() && !text.contains('\n');
        let join = typing
            && self.anchor == self.head
            && self.last_typing.is_some_and(|at| at.elapsed() < Duration::from_millis(800));
        if !join {
            self.undo.push(self.snapshot());
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.last_typing = typing.then(Instant::now);
        self.redo.clear();
        let (start, end) = self.selection();
        let at = self.doc.delete(start, end);
        self.head = self.doc.insert_text(at, text);
        self.anchor = self.head;
        self.changed(cx);
    }
    pub fn history(&mut self, redo: bool, cx: &mut Context<Self>) {
        self.last_typing = None;
        let next = if redo { self.redo.pop() } else { self.undo.pop() };
        if let Some(next) = next {
            let current = self.snapshot();
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            self.doc = next.doc;
            self.head = next.head;
            self.anchor = next.anchor;
            self.marked = None;
            self.changed(cx);
        }
    }
    fn move_to(&mut self, pos: Pos, extend: bool, cx: &mut Context<Self>) {
        self.last_typing = None;
        self.head = pos;
        if !extend {
            self.anchor = pos;
        }
        self.marked = None;
        cx.notify();
    }
    fn horizontal(&mut self, right: bool, extend: bool, cx: &mut Context<Self>) {
        let (a, b) = self.selection();
        let at = if !extend && a != b {
            if right { b } else { a }
        } else if right {
            self.doc.next_pos(self.head)
        } else {
            self.doc.prev_pos(self.head)
        };
        self.move_to(at, extend, cx);
    }
    fn vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        if let Some(layout) = &self.layout
            && let Some(at) = layout.point_for_pos(self.head)
        {
            self.move_to(layout.adjacent_row(self.head, down, at.x), extend, cx);
        }
    }

    fn delete(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.anchor == self.head {
            self.anchor = if forward { self.doc.next_pos(self.head) } else { self.doc.prev_pos(self.head) };
        }
        if self.anchor != self.head {
            self.replace("", cx);
        }
    }
    fn copy(&self, cx: &mut Context<Self>) {
        let (a, b) = self.selection();
        if a != b {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.value()[self.doc.flat_offset(a)..self.doc.flat_offset(b)].to_owned(),
            ));
        }
    }
    fn to_utf16(&self, pos: Pos) -> usize {
        self.value()[..self.doc.flat_offset(pos)].encode_utf16().count()
    }
    fn pos_from_utf16(&self, offset: usize) -> Pos {
        let text = self.value();
        let mut n = 0;
        for (byte, ch) in text.char_indices() {
            if n + ch.len_utf16() > offset {
                return self.doc.pos_at_flat(byte);
            }
            n += ch.len_utf16();
        }
        self.doc.end()
    }
    fn down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.prevent_default();
        cx.stop_propagation();
        self.focus(window, cx);
        if let Some(layout) = &self.layout {
            let pos = layout.pos_for_point(event.position);
            self.move_to(pos, event.modifiers.shift, cx);
            self.selecting = true;
            if event.click_count >= 2 {
                let text = self.doc.blocks[pos.block].text();
                let start = text[..pos.offset]
                    .char_indices()
                    .rev()
                    .find(|(_, c)| c.is_whitespace())
                    .map_or(0, |(i, c)| i + c.len_utf8());
                let end = text[pos.offset..].find(char::is_whitespace).map_or(text.len(), |i| pos.offset + i);
                self.anchor = Pos::new(pos.block, start);
                self.head = Pos::new(pos.block, end);
            }
        }
    }
}

impl Render for BoardInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prepare = cx.entity();
        let paint = cx.entity();
        div()
            .size_full()
            .key_context("Editor")
            .track_focus(&self.focus)
            .cursor_text()
            .on_action(cx.listener(|s, _: &Backspace, _, cx| s.delete(false, cx)))
            .on_action(cx.listener(|s, _: &Delete, _, cx| s.delete(true, cx)))
            .on_action(cx.listener(|s, _: &Enter, _, cx| s.replace("\n", cx)))
            .on_action(cx.listener(|s, _: &Left, _, cx| s.horizontal(false, false, cx)))
            .on_action(cx.listener(|s, _: &Right, _, cx| s.horizontal(true, false, cx)))
            .on_action(cx.listener(|s, _: &SelectLeft, _, cx| s.horizontal(false, true, cx)))
            .on_action(cx.listener(|s, _: &SelectRight, _, cx| s.horizontal(true, true, cx)))
            .on_action(cx.listener(|s, _: &Up, _, cx| s.vertical(false, false, cx)))
            .on_action(cx.listener(|s, _: &Down, _, cx| s.vertical(true, false, cx)))
            .on_action(cx.listener(|s, _: &SelectUp, _, cx| s.vertical(false, true, cx)))
            .on_action(cx.listener(|s, _: &SelectDown, _, cx| s.vertical(true, true, cx)))
            .on_action(cx.listener(|s, _: &LineStart, _, cx| s.move_to(Pos::new(s.head.block, 0), false, cx)))
            .on_action(cx.listener(|s, _: &LineEnd, _, cx| {
                s.move_to(Pos::new(s.head.block, s.doc.blocks[s.head.block].len()), false, cx)
            }))
            .on_action(cx.listener(|s, _: &DocStart, _, cx| s.move_to(Pos::default(), false, cx)))
            .on_action(cx.listener(|s, _: &DocEnd, _, cx| s.move_to(s.doc.end(), false, cx)))
            .on_action(cx.listener(|s, _: &SelectAll, _, cx| {
                s.anchor = Pos::default();
                s.head = s.doc.end();
                cx.notify();
            }))
            .on_action(cx.listener(|s, _: &Undo, _, cx| s.history(false, cx)))
            .on_action(cx.listener(|s, _: &Redo, _, cx| s.history(true, cx)))
            .on_action(cx.listener(|s, _: &Copy, _, cx| s.copy(cx)))
            .on_action(cx.listener(|s, _: &Cut, _, cx| {
                s.copy(cx);
                s.replace("", cx);
            }))
            .on_action(cx.listener(|s, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    s.replace(&text, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::down))
            .on_mouse_move(cx.listener(|s, event: &MouseMoveEvent, _, cx| {
                if s.selecting
                    && let Some(layout) = &s.layout
                {
                    s.move_to(layout.pos_for_point(event.position), true, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|s, _, _, _| s.selecting = false))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|s, _, _, _| s.selecting = false))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        prepare.update(cx, |s, _| {
                            if s.cached.as_ref().is_none_or(|(doc, width, scale, _)| {
                                *doc != s.doc || *width != bounds.size.width || *scale != s.scale
                            }) {
                                let layout = canvas_layout(&s.doc, bounds.size.width, s.scale, window);
                                s.cached = Some((s.doc.clone(), bounds.size.width, s.scale, layout));
                            }
                            let mut layout = s.cached.as_ref().unwrap().3.clone();
                            layout.translate(bounds.origin);
                            s.layout = Some(layout.clone());
                            layout
                        })
                    },
                    move |bounds, layout, window, cx| {
                        let s = paint.read(cx);
                        let (start, end) = s.selection();
                        for quad in layout.selection_quads(start, end) {
                            window.paint_quad(quad);
                        }
                        layout.paint_text(window, cx);
                        let s = paint.read(cx);
                        if s.focus.is_focused(window)
                            && let Some(at) = layout.point_for_pos(s.head)
                        {
                            window.paint_quad(fill(
                                Bounds::new(at, size(px(1.5), layout.line_height_at(s.head.block))),
                                theme::accent(),
                            ));
                        }
                        window.handle_input(&s.focus, ElementInputHandler::new(bounds, paint.clone()), cx);
                    },
                )
                .size_full(),
            )
    }
}

impl EntityInputHandler for BoardInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (a, b) = (self.pos_from_utf16(range.start), self.pos_from_utf16(range.end));
        *actual = Some(self.to_utf16(a)..self.to_utf16(b));
        Some(self.value()[self.doc.flat_offset(a)..self.doc.flat_offset(b)].to_owned())
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let (a, b) = self.selection();
        Some(UTF16Selection { range: self.to_utf16(a)..self.to_utf16(b), reversed: self.head < self.anchor })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.map(|(a, b)| self.to_utf16(a)..self.to_utf16(b))
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((a, b)) = range.map(|r| (self.pos_from_utf16(r.start), self.pos_from_utf16(r.end))).or(self.marked)
        {
            self.anchor = a;
            self.head = b;
        }
        self.marked = None;
        self.replace(text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((a, b)) = range.map(|r| (self.pos_from_utf16(r.start), self.pos_from_utf16(r.end))).or(self.marked)
        {
            self.anchor = a;
            self.head = b;
        }
        let start = self.selection().0;
        let offset = self.to_utf16(start);
        self.replace(text, cx);
        self.marked = (!text.is_empty()).then_some((start, self.head));
        if let Some(sel) = selection {
            let len = text.encode_utf16().count();
            self.anchor = self.pos_from_utf16(offset + sel.start.min(len));
            self.head = self.pos_from_utf16(offset + sel.end.min(len));
        }
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let start = self.pos_from_utf16(range.start);
        let at = self.layout.as_ref()?.point_for_pos(start)?;
        Some(Bounds::new(at, size(px(2.), self.layout.as_ref()?.line_height_at(start.block))))
    }
    fn character_index_for_point(&mut self, at: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.to_utf16(self.layout.as_ref()?.pos_for_point(at)))
    }
    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        true
    }
}
