//! The document editor. Every edit goes into the library's permanent
//! history: consecutive typing (or deleting) coalesces into one revision,
//! committed when the run ends or after a short pause, so undo steps feel
//! like a word processor's while nothing is ever lost.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;
use quill_doc::{BlockKind, Document, Marks, Pos};
use quill_store::{DocId, Library};

use crate::page::{PageElement, PageLayout, TEXT_WIDTH};
use crate::theme;

const COMMIT_AFTER: Duration = Duration::from_millis(800);

actions!(
    editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        LineStart,
        LineEnd,
        DocStart,
        DocEnd,
        Enter,
        Undo,
        Redo,
        Bold,
        Italic,
        Underline,
        Strike,
        Highlight,
        Copy,
        Cut,
        Paste,
        CloseDocument,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Editor");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-left", LineStart, ctx),
        KeyBinding::new("cmd-right", LineEnd, ctx),
        KeyBinding::new("home", LineStart, ctx),
        KeyBinding::new("end", LineEnd, ctx),
        KeyBinding::new("cmd-up", DocStart, ctx),
        KeyBinding::new("cmd-down", DocEnd, ctx),
        KeyBinding::new("enter", Enter, ctx),
        KeyBinding::new("cmd-z", Undo, ctx),
        KeyBinding::new("cmd-shift-z", Redo, ctx),
        KeyBinding::new("cmd-b", Bold, ctx),
        KeyBinding::new("cmd-i", Italic, ctx),
        KeyBinding::new("cmd-u", Underline, ctx),
        KeyBinding::new("cmd-shift-x", Strike, ctx),
        KeyBinding::new("cmd-shift-h", Highlight, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-w", CloseDocument, ctx),
        KeyBinding::new("escape", CloseDocument, ctx),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

pub enum EditorEvent {
    Close,
}

pub struct EditorView {
    pub focus_handle: FocusHandle,
    library: Rc<RefCell<Library>>,
    id: DocId,
    pub doc: Document,
    /// Selection: `anchor` stays put while `head` moves. Equal = a caret.
    anchor: Pos,
    pub head: Pos,
    /// IME composition in progress.
    marked: Option<(Pos, Pos)>,
    /// Remembered x for repeated up/down through lines of different length.
    goal_x: Option<Pixels>,
    selecting: bool,
    pending: Option<EditKind>,
    commit_task: Option<Task<()>>,
    pub layout: Option<PageLayout>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EditorView {
    pub fn new(
        library: Rc<RefCell<Library>>,
        id: DocId,
        doc: Document,
        cx: &mut Context<Self>,
    ) -> EditorView {
        let end = doc.end();
        EditorView {
            focus_handle: cx.focus_handle(),
            library,
            id,
            doc,
            anchor: end,
            head: end,
            marked: None,
            goal_x: None,
            selecting: false,
            pending: None,
            commit_task: None,
            layout: None,
        }
    }

    /// The selection as an ordered `(start, end)` pair.
    pub fn selection(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Writes the current edit group to the permanent history.
    pub fn commit(&mut self) {
        self.commit_task = None;
        if self.pending.take().is_none() {
            return;
        }
        if let Err(err) =
            self.library
                .borrow_mut()
                .commit(self.id, &self.doc, self.head, quill_store::today())
        {
            eprintln!("quill: saving failed: {err}");
        }
    }

    fn edit(
        &mut self,
        kind: EditKind,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Document, Pos, Pos) -> Pos,
    ) {
        if self.pending.is_some_and(|k| k != kind) {
            self.commit();
        }
        let (start, end) = self.selection();
        let pos = f(&mut self.doc, start, end);
        self.anchor = pos;
        self.head = pos;
        self.goal_x = None;
        self.pending = Some(kind);
        if kind == EditKind::Other {
            self.commit();
        } else {
            self.commit_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(COMMIT_AFTER).await;
                this.update(cx, |this, cx| {
                    this.commit();
                    cx.notify();
                })
                .ok();
            }));
        }
        cx.notify();
    }

    fn replace_selection(&mut self, text: &str, kind: EditKind, cx: &mut Context<Self>) {
        self.edit(kind, cx, |doc, start, end| {
            let pos = doc.delete(start, end);
            doc.insert_text(pos, text)
        });
    }

    fn move_to(&mut self, pos: Pos, extend: bool, cx: &mut Context<Self>) {
        self.commit();
        self.head = pos;
        if !extend {
            self.anchor = pos;
        }
        cx.notify();
    }

    fn horizontal(&mut self, forward: bool, extend: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        let (start, end) = self.selection();
        let pos = match (extend, start == end, forward) {
            (false, false, false) => start,
            (false, false, true) => end,
            (_, _, false) => self.doc.prev_pos(self.head),
            (_, _, true) => self.doc.next_pos(self.head),
        };
        self.move_to(pos, extend, cx);
    }

    fn vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(layout) = &self.layout else { return };
        let Some(at) = layout.point_for_pos(self.head) else {
            return;
        };
        let x = *self.goal_x.get_or_insert(at.x);
        let lh = layout.line_height_at(self.head.block);
        let target = if down {
            point(x, at.y + lh + lh / 2.)
        } else {
            point(x, at.y - lh / 2.)
        };
        let pos = if !down && self.head.block == 0 && target.y < layout.blocks[0].origin.y {
            Pos::default()
        } else {
            layout.pos_for_point(target)
        };
        self.move_to(pos, extend, cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.anchor == self.head {
            let prev = self.doc.prev_pos(self.head);
            if prev == self.head {
                return;
            }
            self.anchor = prev;
        }
        self.edit(EditKind::Deleting, cx, |doc, start, end| {
            doc.delete(start, end)
        });
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.anchor == self.head {
            let next = self.doc.next_pos(self.head);
            if next == self.head {
                return;
            }
            self.anchor = next;
        }
        self.edit(EditKind::Deleting, cx, |doc, start, end| {
            doc.delete(start, end)
        });
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(EditKind::Typing, cx, |doc, start, end| {
            let pos = doc.delete(start, end);
            doc.split_block(pos)
        });
    }

    fn toggle(&mut self, mark: Marks, cx: &mut Context<Self>) {
        let (start, end) = self.selection();
        if start == end {
            return;
        }
        let (anchor, head) = (self.anchor, self.head);
        self.edit(EditKind::Other, cx, |doc, start, end| {
            doc.toggle_mark(start, end, mark);
            end
        });
        self.anchor = anchor;
        self.head = head;
    }

    fn set_block_kind(&mut self, kind: BlockKind, cx: &mut Context<Self>) {
        let (anchor, head) = (self.anchor, self.head);
        self.edit(EditKind::Other, cx, |doc, start, end| {
            for block in start.block..=end.block {
                let current = doc.blocks[block].kind;
                doc.set_block_kind(
                    block,
                    if current == kind {
                        BlockKind::Paragraph
                    } else {
                        kind
                    },
                );
            }
            end
        });
        self.anchor = anchor;
        self.head = head;
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.commit();
        let restored = self.library.borrow_mut().undo(self.id);
        self.apply_history(restored, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.commit();
        let restored = self.library.borrow_mut().redo(self.id);
        self.apply_history(restored, cx);
    }

    fn apply_history(
        &mut self,
        restored: quill_store::Result<Option<(Document, Pos)>>,
        cx: &mut Context<Self>,
    ) {
        match restored {
            Ok(Some((doc, cursor))) => {
                self.doc = doc;
                let cursor = self.clamp(cursor);
                self.anchor = cursor;
                self.head = cursor;
                self.marked = None;
                cx.notify();
            }
            Ok(None) => {}
            Err(err) => eprintln!("quill: history step failed: {err}"),
        }
    }

    fn clamp(&self, pos: Pos) -> Pos {
        let block = pos.block.min(self.doc.blocks.len() - 1);
        Pos::new(block, pos.offset.min(self.doc.blocks[block].len()))
    }

    fn selected_text(&self) -> String {
        let (start, end) = self.selection();
        let text = self.doc.plain_text();
        text[self.doc.flat_offset(start)..self.doc.flat_offset(end)].to_string()
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.anchor != self.head {
            cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text()));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.anchor != self.head {
            cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text()));
            self.edit(EditKind::Other, cx, |doc, start, end| {
                doc.delete(start, end)
            });
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text.replace("\r\n", "\n"), EditKind::Other, cx);
        }
    }

    fn close(&mut self, _: &CloseDocument, _: &mut Window, cx: &mut Context<Self>) {
        self.commit();
        cx.emit(EditorEvent::Close);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let Some(layout) = &self.layout else { return };
        let pos = layout.pos_for_point(event.position);
        self.selecting = true;
        self.goal_x = None;
        self.move_to(pos, event.modifiers.shift, cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if let Some(layout) = &self.layout {
            let pos = layout.pos_for_point(event.position);
            if pos != self.head {
                self.head = pos;
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    // Platform text input addresses the document as UTF-16 offsets into its
    // plain text (blocks joined by '\n').

    fn to_utf16(&self, pos: Pos) -> usize {
        let text = self.doc.plain_text();
        text[..self.doc.flat_offset(pos)].encode_utf16().count()
    }

    fn pos_from_utf16(&self, offset: usize) -> Pos {
        let text = self.doc.plain_text();
        let mut units = 0;
        let mut bytes = 0;
        for ch in text.chars() {
            if units >= offset {
                break;
            }
            units += ch.len_utf16();
            bytes += ch.len_utf8();
        }
        self.doc.pos_at_flat(bytes)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> (Pos, Pos) {
        (
            self.pos_from_utf16(range.start),
            self.pos_from_utf16(range.end),
        )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (start, _) = self.selection();
        let block_kind = self.doc.blocks.get(start.block).map(|b| b.kind);
        let mark_button =
            |id: &'static str, label: &'static str, mark: Marks, cx: &mut Context<Self>| {
                tool_button(id, label, false)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(mark, cx)))
            };
        let kind_button =
            |id: &'static str, label: &'static str, kind: BlockKind, cx: &mut Context<Self>| {
                tool_button(id, label, block_kind == Some(kind))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_block_kind(kind, cx)))
            };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .px(px(16.))
            .py(px(6.))
            .border_t_1()
            .border_color(theme::border())
            .child(
                tool_button("undo", "Undo", false)
                    .on_click(cx.listener(|this, _, w, cx| this.undo(&Undo, w, cx))),
            )
            .child(
                tool_button("redo", "Redo", false)
                    .on_click(cx.listener(|this, _, w, cx| this.redo(&Redo, w, cx))),
            )
            .child(separator())
            .child(kind_button("h1", "Title", BlockKind::Heading(1), cx))
            .child(kind_button("h2", "Heading", BlockKind::Heading(2), cx))
            .child(kind_button("bullet", "• List", BlockKind::Bullet, cx))
            .child(kind_button("numbered", "1. List", BlockKind::Numbered, cx))
            .child(kind_button("quote", "Quote", BlockKind::Quote, cx))
            .child(separator())
            .child(mark_button("bold", "B", Marks::BOLD, cx).font_weight(FontWeight::BOLD))
            .child(mark_button("italic", "I", Marks::ITALIC, cx).italic())
            .child(mark_button("underline", "U", Marks::UNDERLINE, cx).underline())
            .child(mark_button("strike", "S", Marks::STRIKE, cx).line_through())
            .child(mark_button("highlight", "Highlight", Marks::HIGHLIGHT, cx))
            .child(div().flex_1())
            .child(
                div()
                    .px(px(6.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(format!("{} words", self.doc.word_count())),
            )
    }
}

fn tool_button(id: &'static str, label: &'static str, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .min_w(px(36.))
        .h(px(32.))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .when(active, |d| d.bg(theme::selected_bg()))
        .hover(|s| s.bg(theme::raised()))
        .child(label)
}

fn separator() -> Div {
    div().w(px(1.)).h(px(22.)).mx(px(6.)).bg(theme::border())
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = if self.pending.is_some() {
            "Editing…"
        } else {
            "Saved"
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .text_color(theme::text())
            .font_family(theme::UI_FONT)
            .text_size(px(14.))
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(|this, _: &Left, _, cx| this.horizontal(false, false, cx)))
            .on_action(cx.listener(|this, _: &Right, _, cx| this.horizontal(true, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.horizontal(false, true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.horizontal(true, true, cx)))
            .on_action(cx.listener(|this, _: &Up, _, cx| this.vertical(false, false, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.vertical(true, false, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.vertical(false, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.vertical(true, true, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.anchor = Pos::default();
                this.move_to(this.doc.end(), true, cx);
            }))
            .on_action(cx.listener(|this, _: &LineStart, _, cx| {
                this.move_to(Pos::new(this.head.block, 0), false, cx)
            }))
            .on_action(cx.listener(|this, _: &LineEnd, _, cx| {
                let block = this.head.block;
                this.move_to(Pos::new(block, this.doc.blocks[block].len()), false, cx)
            }))
            .on_action(
                cx.listener(|this, _: &DocStart, _, cx| this.move_to(Pos::default(), false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DocEnd, _, cx| this.move_to(this.doc.end(), false, cx)),
            )
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|this, _: &Bold, _, cx| this.toggle(Marks::BOLD, cx)))
            .on_action(cx.listener(|this, _: &Italic, _, cx| this.toggle(Marks::ITALIC, cx)))
            .on_action(cx.listener(|this, _: &Underline, _, cx| this.toggle(Marks::UNDERLINE, cx)))
            .on_action(cx.listener(|this, _: &Strike, _, cx| this.toggle(Marks::STRIKE, cx)))
            .on_action(cx.listener(|this, _: &Highlight, _, cx| this.toggle(Marks::HIGHLIGHT, cx)))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::close))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .bg(theme::surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .px(px(16.))
                            .py(px(8.))
                            .child(tool_button("library", "‹ Library", false).on_click(
                                cx.listener(|this, _, w, cx| this.close(&CloseDocument, w, cx)),
                            ))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.doc.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme::muted())
                                    .child(status),
                            ),
                    )
                    .child(self.toolbar(cx)),
            )
            .child(
                div().id("page-scroll").flex_1().overflow_y_scroll().child(
                    div().flex().justify_center().py(px(40.)).child(
                        div()
                            .w(px(TEXT_WIDTH + 160.))
                            .min_h(px(900.))
                            .px(px(80.))
                            .py(px(72.))
                            .bg(theme::surface())
                            .rounded(px(4.))
                            .cursor(CursorStyle::IBeam)
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                            .on_mouse_move(cx.listener(Self::on_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                            .child(PageElement {
                                editor: cx.entity(),
                            }),
                    ),
                ),
            )
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (start, end) = self.range_from_utf16(&range);
        actual.replace(self.to_utf16(start)..self.to_utf16(end));
        let text = self.doc.plain_text();
        Some(text[self.doc.flat_offset(start)..self.doc.flat_offset(end)].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (start, end) = self.selection();
        Some(UTF16Selection {
            range: self.to_utf16(start)..self.to_utf16(end),
            reversed: self.head < self.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .map(|(start, end)| self.to_utf16(start)..self.to_utf16(end))
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
        if let Some((start, end)) = range.map(|r| self.range_from_utf16(&r)).or(self.marked) {
            self.anchor = start;
            self.head = end;
        }
        self.marked = None;
        self.replace_selection(text, EditKind::Typing, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((start, end)) = range.map(|r| self.range_from_utf16(&r)).or(self.marked) {
            self.anchor = start;
            self.head = end;
        }
        let (start, _) = self.selection();
        self.replace_selection(text, EditKind::Typing, cx);
        let start_utf16 = self.to_utf16(start);
        self.marked = (!text.is_empty()).then_some((start, self.head));
        if let Some(sel) = new_selection {
            let len = text.encode_utf16().count();
            self.anchor = self.pos_from_utf16(start_utf16 + sel.start.min(len));
            self.head = self.pos_from_utf16(start_utf16 + sel.end.min(len));
        }
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let (start, end) = self.range_from_utf16(&range);
        let a = layout.point_for_pos(start)?;
        let b = layout.point_for_pos(end)?;
        let lh = layout.line_height_at(end.block);
        Some(Bounds::from_corners(
            a,
            point(b.x.max(a.x + px(1.)), b.y + lh),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let pos = self.layout.as_ref()?.pos_for_point(point);
        Some(self.to_utf16(pos))
    }
}
