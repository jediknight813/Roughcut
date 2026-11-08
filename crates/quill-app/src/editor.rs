//! The document editor. Every edit goes into the library's permanent
//! history: consecutive typing (or deleting) coalesces into one revision,
//! committed when the run ends or after a short pause, so undo steps feel
//! like a word processor's while nothing is ever lost.
//!
//! The page sits on a whiteboard (see `board.rs`). In Board mode the whole
//! canvas zooms out so notes and drawings sit around a smaller page; clicking
//! the page glides it back to full size for writing. Focus mode hides everything but the text and dims every
//! paragraph except the current one.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use quill_doc::{BlockKind, Document, Marks, Pos};
use quill_spell::Speller;
use quill_store::{AssetId, BoardItem, DocId, ItemId, Library, Revision};

use crate::anim::{self, Tween};
use crate::assets::icon;
use crate::board::{BOARD_COLORS, Editing, Gesture, Tool};
use crate::media::{Control, Media};
use crate::page::{LayoutCache, LayoutState, PageElement, PageLayout, TEXT_WIDTH};
use crate::theme;

#[path = "formatting.rs"]
pub(crate) mod formatting;

const COMMIT_AFTER: Duration = Duration::from_millis(800);
const MAX_UNSAVED: Duration = Duration::from_secs(5);
const BLINK: Duration = Duration::from_millis(530);
const SPELL_AFTER: Duration = Duration::from_millis(180);
/// Distance from the top of the board to the top of the page.
pub const PAGE_TOP: f32 = 48.;
const PAGE_PADDING: (f32, f32) = (80., 72.);
const PAGE_MIN_HEIGHT: f32 = 1000.;
/// How far the canvas zooms out in Board mode.
const BOARD_ZOOM: f32 = 0.78;
const HISTORY_WIDTH: f32 = 300.;
const HISTORY_SHOWN: usize = 400;
const DOT_SPACING: f32 = 24.;
/// Spelling menu geometry, shared with tests that click its rows.
pub const MENU_WIDTH: f32 = 220.;
pub const MENU_PADDING: f32 = 6.;
pub const MENU_ROW: f32 = 30.;
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 2.5;
/// Stops for ⌘= / ⌘− and the toolbar's zoom buttons.
const ZOOM_STEPS: [f32; 10] = [0.3, 0.5, 0.67, 0.78, 0.9, 1.0, 1.25, 1.5, 2.0, 2.5];

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
        Escape,
        ToggleFocus,
        ToggleBoard,
        ToggleHistory,
        ToggleSpelling,
        ZoomIn,
        ZoomOut,
        ZoomReset,
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
        KeyBinding::new("escape", Escape, ctx),
        KeyBinding::new("cmd-shift-f", ToggleFocus, ctx),
        KeyBinding::new("cmd-shift-b", ToggleBoard, ctx),
        KeyBinding::new("cmd-shift-y", ToggleHistory, ctx),
        KeyBinding::new("cmd-=", ZoomIn, ctx),
        KeyBinding::new("cmd-+", ZoomIn, ctx),
        KeyBinding::new("cmd--", ZoomOut, ctx),
        KeyBinding::new("cmd-0", ZoomReset, ctx),
        KeyBinding::new("cmd-alt-l", ToggleSpelling, ctx),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Board,
    Page,
    Focus,
}

/// The right-click menu on a misspelled word.
struct SpellMenu {
    block: usize,
    range: Range<usize>,
    word: String,
    suggestions: Vec<String>,
    loading: bool,
    at: Point<Pixels>,
}

pub enum EditorEvent {
    Close,
}

pub struct EditorView {
    pub focus_handle: FocusHandle,
    pub(crate) library: Rc<RefCell<Library>>,
    pub(crate) id: DocId,
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
    pending_since: Option<Instant>,
    typing_at: Option<Pos>,
    commit_task: Option<Task<()>>,
    save_error: Option<String>,
    layout_cache: LayoutCache,
    layout_revision: u64,
    block_texts: Vec<Arc<str>>,
    block_utf16: Vec<usize>,
    words: usize,
    reveal_caret: bool,
    center_page: bool,
    format_menu: Option<(formatting::FormatMenu, Point<Pixels>)>,
    pub layout: Option<PageLayout>,
    pub caret_on: bool,
    last_activity: Instant,
    _blink: Task<()>,
    _save_on_quit: Subscription,

    pub mode: Mode,
    pub(crate) zoom: Tween,
    render_scale: f32,
    /// The zoom Board mode returns to; pinching changes it.
    board_zoom: f32,
    scroll: ScrollHandle,
    focus_scroll: ScrollHandle,
    history_slide: Tween,
    pub(crate) board: Vec<BoardItem>,
    pub(crate) tool: Tool,
    pub(crate) board_color: u32,
    /// Window position of the board origin (page top centre), from the last paint.
    pub(crate) board_origin: Point<Pixels>,
    pub(crate) gesture: Option<Gesture>,
    pub(crate) selected_item: Option<ItemId>,
    pub(crate) editing: Option<Editing>,
    pub(crate) removed_items: Vec<ItemId>,
    pub(crate) images: HashMap<AssetId, Arc<Image>>,

    speller: Rc<RefCell<Speller>>,
    pub spellcheck: bool,
    /// Misspelled ranges by block text, so unchanged paragraphs aren't rechecked.
    spell_cache: HashMap<Arc<str>, Vec<Range<usize>>>,
    spell_task: Option<Task<()>>,
    spell_generation: u64,
    spell_ready: u64,
    suggestion_task: Option<Task<()>>,
    /// Words ignored for this session ("Ignore Spelling"), lowercased.
    ignored: HashSet<String>,
    spell_menu: Option<SpellMenu>,

    media: Entity<Media>,
    history: Option<Vec<Revision>>,
    session_start_words: usize,
    _observe_media: Subscription,
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
        media: Entity<Media>,
        speller: Rc<RefCell<Speller>>,
        cx: &mut Context<Self>,
    ) -> EditorView {
        let end = doc.end();
        let session_start_words = doc.word_count();
        let block_texts: Vec<Arc<str>> = doc.blocks.iter().map(|b| Arc::from(b.text())).collect();
        let block_utf16 = block_texts.iter().map(|t| t.encode_utf16().count()).collect();
        // Window teardown and application shutdown must flush the debounce buffer.
        cx.on_release(|this, _| {
            this.commit();
        })
        .detach();
        let save_on_quit = cx.on_app_quit(|this, _| {
            this.commit();
            async {}
        });
        let observe_media = cx.observe(&media, |_, _, cx| cx.notify());
        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let alive = this
                    .update(cx, |this, cx| {
                        let on = this.mode != Mode::Board
                            && this.anchor == this.head
                            && (this.last_activity.elapsed() < BLINK || !this.caret_on);
                        if on != this.caret_on {
                            this.caret_on = on;
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        let mut editor = EditorView {
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
            pending_since: None,
            typing_at: None,
            commit_task: None,
            save_error: None,
            layout_cache: LayoutCache::default(),
            layout_revision: 0,
            block_texts,
            block_utf16,
            words: session_start_words,
            reveal_caret: false,
            center_page: true,
            format_menu: None,
            layout: None,
            caret_on: true,
            last_activity: Instant::now(),
            _blink: blink,
            _save_on_quit: save_on_quit,
            mode: Mode::Page,
            zoom: Tween::new(1.),
            render_scale: 1.,
            board_zoom: BOARD_ZOOM,
            scroll: ScrollHandle::new(),
            focus_scroll: ScrollHandle::new(),
            history_slide: Tween::new(0.),
            board: Vec::new(),
            tool: Tool::Select,
            board_color: BOARD_COLORS[0],
            board_origin: Point::default(),
            gesture: None,
            selected_item: None,
            editing: None,
            removed_items: Vec::new(),
            images: HashMap::new(),
            speller,
            spellcheck: true,
            spell_cache: HashMap::new(),
            spell_task: None,
            spell_generation: 0,
            spell_ready: 0,
            suggestion_task: None,
            ignored: HashSet::new(),
            spell_menu: None,
            media,
            history: None,
            session_start_words,
            _observe_media: observe_media,
        };
        editor.reload_board();
        editor.schedule_spellcheck(Duration::ZERO, cx);
        editor
    }

    /// The selection as an ordered `(start, end)` pair.
    pub fn selection(&self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    /// Current zoom of the page and board, mid-animation included.
    pub fn page_scale(&self) -> f32 {
        self.render_scale
    }

    pub fn dim_except(&self) -> Option<usize> {
        (self.mode == Mode::Focus && self.anchor == self.head).then_some(self.head.block)
    }

    /// Misspelled byte ranges for each block. The word being typed isn't
    /// flagged until the caret leaves it, so words aren't marked mid-way.
    pub fn misspellings(&self) -> Vec<Vec<Range<usize>>> {
        if !self.spellcheck {
            return Vec::new();
        }
        let typing = self.typing_at == Some(self.head) && self.anchor == self.head;
        self.block_texts
            .iter()
            .enumerate()
            .map(|(i, text)| {
                self.spell_cache
                    .get(text)
                    .into_iter()
                    .flatten()
                    .filter(|r| {
                        !(typing && i == self.head.block && r.start <= self.head.offset && self.head.offset <= r.end)
                    })
                    .filter(|r| !self.ignored.contains(&text[(*r).clone()].to_lowercase()))
                    .cloned()
                    .collect()
            })
            .collect()
    }

    fn schedule_spellcheck(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.spell_generation += 1;
        self.spell_task = None;
        if !self.spellcheck {
            return;
        }
        let generation = self.spell_generation;
        self.spell_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let Ok((speller, texts)) = this.update(cx, |this, _| {
                // Keep only current paragraph versions; typing cannot grow this cache indefinitely.
                let current: HashSet<_> = this.block_texts.iter().cloned().collect();
                this.spell_cache.retain(|text, _| current.contains(text));
                let missing: Vec<_> = current.into_iter().filter(|t| !this.spell_cache.contains_key(t)).collect();
                if missing.is_empty() {
                    this.spell_ready = generation;
                }
                (this.speller.borrow().clone(), missing)
            }) else {
                return;
            };
            if texts.is_empty() {
                return;
            }
            let checked = cx
                .background_executor()
                .spawn(async move {
                    texts
                        .into_iter()
                        .map(|text| {
                            let ranges = speller.misspellings(&text);
                            (text, ranges)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                if generation != this.spell_generation {
                    return;
                }
                this.spell_cache.extend(checked);
                this.spell_ready = generation;
                this.layout_revision += 1;
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn shape_page(&mut self, window: &mut Window) -> PageLayout {
        let mut cache = std::mem::take(&mut self.layout_cache);
        let typing = self.typing_at.filter(|_| self.anchor == self.head);
        let page = cache.shape(
            &self.doc,
            LayoutState {
                revision: self.layout_revision,
                scale: self.page_scale(),
                dim_except: self.dim_except(),
                typing,
            },
            || self.misspellings(),
            window,
        );
        self.layout_cache = cache;
        page
    }

    pub(crate) fn reveal_caret(&mut self, layout: &PageLayout, cx: &mut Context<Self>) {
        if !self.reveal_caret || self.mode == Mode::Board {
            return;
        }
        self.reveal_caret = false;
        let Some(at) = layout.point_for_pos(self.head) else { return };
        let scroll = if self.mode == Mode::Focus { &self.focus_scroll } else { &self.scroll };
        let bounds = scroll.bounds();
        let margin = px(32.);
        let bottom = bounds.bottom() - margin - layout.line_height_at(self.head.block);
        let top = bounds.top() + margin;
        let delta = if at.y < top {
            top - at.y
        } else if at.y > bottom {
            bottom - at.y
        } else {
            px(0.)
        };
        if delta != px(0.) {
            let offset = scroll.offset();
            scroll.set_offset(point(offset.x, (offset.y + delta).min(px(0.))));
            cx.notify();
        }
    }

    fn on_right_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.spell_menu = None;
        self.typing_at = None;
        if self.mode == Mode::Board {
            return;
        }
        window.focus(&self.focus_handle, cx);
        let Some(layout) = &self.layout else { return };
        let pos = layout.pos_for_point(event.position);
        let hit = self
            .misspellings()
            .get(pos.block)
            .and_then(|ranges| ranges.iter().find(|r| r.start <= pos.offset && pos.offset <= r.end).cloned());
        if let Some(range) = hit {
            let word = self.doc.blocks[pos.block].text()[range.clone()].to_string();
            self.anchor = Pos::new(pos.block, range.start);
            self.head = Pos::new(pos.block, range.end);
            self.spell_menu = Some(SpellMenu {
                block: pos.block,
                range,
                word: word.clone(),
                suggestions: Vec::new(),
                loading: true,
                at: event.position,
            });
            let speller = self.speller.borrow().clone();
            self.suggestion_task = Some(cx.spawn(async move |this, cx| {
                let request = word.clone();
                let suggestions = cx.background_executor().spawn(async move { speller.suggestions(&request) }).await;
                this.update(cx, |this, cx| {
                    if let Some(menu) = &mut this.spell_menu
                        && menu.word == word
                        && menu.block == pos.block
                    {
                        menu.suggestions = suggestions;
                        menu.loading = false;
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        cx.notify();
    }

    fn apply_suggestion(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(menu) = self.spell_menu.take() else { return };
        let Some(word) = menu.suggestions.get(index).cloned() else { return };
        self.commit();
        self.anchor = Pos::new(menu.block, menu.range.start);
        self.head = Pos::new(menu.block, menu.range.end);
        self.replace_selection(&word, EditKind::Other, cx);
    }

    fn learn_spelling(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.spell_menu.take() else { return };
        self.speller.borrow_mut().learn(&menu.word);
        if let Err(err) = self.library.borrow_mut().learn_word(&menu.word) {
            eprintln!("roughcut: saving a learned word failed: {err}");
        }
        self.spell_cache.clear();
        self.layout_revision += 1;
        self.schedule_spellcheck(Duration::ZERO, cx);
        cx.notify();
    }

    fn ignore_spelling(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.spell_menu.take() else { return };
        self.ignored.insert(menu.word.to_lowercase());
        self.layout_revision += 1;
        cx.notify();
    }

    fn toggle_spelling(&mut self, cx: &mut Context<Self>) {
        self.spellcheck = !self.spellcheck;
        self.spell_menu = None;
        self.layout_revision += 1;
        self.schedule_spellcheck(Duration::ZERO, cx);
        cx.notify();
    }

    fn spell_menu_element(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.spell_menu.as_ref()?;
        let row = |id: (&'static str, usize), label: String, bold: bool| {
            div()
                .id(id)
                .h(px(MENU_ROW))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(13.))
                .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                .hover(|s| s.bg(theme::accent()).text_color(theme::on_accent()))
                .child(label)
        };
        let mut rows: Vec<AnyElement> = menu
            .suggestions
            .iter()
            .enumerate()
            .map(|(i, word)| {
                row(("suggestion", i), word.clone(), true)
                    .on_click(cx.listener(move |this, _, _, cx| this.apply_suggestion(i, cx)))
                    .into_any_element()
            })
            .collect();
        if rows.is_empty() {
            rows.push(
                div()
                    .h(px(MENU_ROW))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(if menu.loading { "Looking for suggestions…" } else { "No guesses found" })
                    .into_any_element(),
            );
        }
        rows.push(div().h(px(1.)).my(px(4.)).bg(theme::border()).into_any_element());
        rows.push(
            row(("spell-learn", 0), "Learn Spelling".into(), false)
                .on_click(cx.listener(|this, _, _, cx| this.learn_spelling(cx)))
                .into_any_element(),
        );
        rows.push(
            row(("spell-ignore", 0), "Ignore Spelling".into(), false)
                .on_click(cx.listener(|this, _, _, cx| this.ignore_spelling(cx)))
                .into_any_element(),
        );
        Some(
            deferred(
                anchored().position(menu.at).snap_to_window().child(
                    div()
                        .id("spell-menu")
                        .block_mouse_except_scroll()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(MENU_WIDTH))
                        .p(px(MENU_PADDING))
                        .rounded(px(10.))
                        .bg(rgba(0x23262df5))
                        .border_1()
                        .border_color(rgb(0x353a45))
                        .shadow(floating_shadow())
                        .text_color(theme::text())
                        .font_family(theme::UI_FONT)
                        .children(rows)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.spell_menu = None;
                            cx.notify();
                        }))
                        .with_animation(
                            "spell-menu-in",
                            Animation::new(anim::QUICK).with_easing(anim::ease_out),
                            |el, t| el.opacity(t),
                        ),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn touch(&mut self) {
        self.last_activity = Instant::now();
        self.caret_on = true;
    }

    pub(crate) fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.commit();
        self.center_page = false;
        self.gesture = None;
        if mode != Mode::Board {
            self.stop_editing_item(cx);
            self.selected_item = None;
        }
        self.mode = mode;
        self.reveal_caret = mode != Mode::Board;
        let zoom = match mode {
            Mode::Board => self.board_zoom,
            Mode::Page | Mode::Focus => 1.,
        };
        self.zoom.set(zoom, anim::STANDARD);
        self.touch();
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Zooms the canvas to `z`, keeping the board point under `anchor`
    /// (window coordinates) in place. Zooming the page out far enough moves
    /// to the board, the way pinching out of a document does in Freeform.
    pub(crate) fn zoom_to(&mut self, z: f32, anchor: Option<Point<Pixels>>, animate: bool, cx: &mut Context<Self>) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        let old = self.zoom.value();
        if self.mode == Mode::Focus {
            return;
        }
        self.center_page = false;
        if self.mode == Mode::Page && z < 0.97 {
            self.commit();
            self.mode = Mode::Board;
        }
        if self.mode == Mode::Board {
            self.board_zoom = z;
        }
        if let Some(anchor) = anchor {
            let b = anchor - self.board_origin;
            let (bx, by) = (f32::from(b.x) / old.max(0.01), f32::from(b.y) / old.max(0.01));
            let offset = self.scroll.offset();
            self.scroll.set_offset(point(offset.x - px(bx * (z - old)), offset.y - px(by * (z - old))));
        }
        self.zoom.set(z, if animate { anim::QUICK } else { Duration::ZERO });
        cx.notify();
    }

    pub(crate) fn zoom_step(&mut self, up: bool, cx: &mut Context<Self>) {
        let current = self.zoom.target();
        let next = if up {
            ZOOM_STEPS.iter().copied().find(|s| *s > current + 0.01).unwrap_or(MAX_ZOOM)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|s| *s < current - 0.01).unwrap_or(MIN_ZOOM)
        };
        self.zoom_to(next, None, true, cx);
    }

    fn on_pinch(&mut self, event: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        let z = self.zoom.target() * (1. + event.delta);
        self.zoom_to(z, Some(event.position), false, cx);
    }

    /// ⌘ or Ctrl + scroll zooms, like Maps and Freeform.
    fn on_scroll_wheel(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !(event.modifiers.platform || event.modifiers.control) {
            return;
        }
        cx.stop_propagation();
        let dy = f32::from(event.delta.pixel_delta(px(16.)).y);
        let z = self.zoom.target() * (1. + dy * 0.01).clamp(0.5, 1.5);
        self.zoom_to(z, Some(event.position), false, cx);
    }

    /// Writes the current edit group to the permanent history.
    pub fn commit(&mut self) -> bool {
        self.commit_task = None;
        if self.pending.is_none() {
            return true;
        }
        if let Err(err) = self.library.borrow_mut().commit(self.id, &self.doc, self.head, quill_store::today()) {
            eprintln!("roughcut: saving failed: {err}");
            self.save_error = Some(err.to_string());
            return false;
        }
        self.pending = None;
        self.pending_since = None;
        self.save_error = None;
        if self.history.is_some() {
            self.history = Some(self.load_history());
        }
        true
    }

    fn edit(&mut self, kind: EditKind, cx: &mut Context<Self>, f: impl FnOnce(&mut Document, Pos, Pos) -> Pos) {
        if self.pending.is_some_and(|k| k != kind) {
            self.commit();
        }
        let (start, end) = self.selection();
        let old_len = self.doc.blocks.len();
        let old_words: usize = self.doc.blocks[start.block..=end.block].iter().map(|b| b.word_count()).sum();
        let pos = f(&mut self.doc, start, end);
        let new_end = (end.block + self.doc.blocks.len()) - old_len;
        let changed = &self.doc.blocks[start.block..=new_end];
        self.words = self.words - old_words + changed.iter().map(|b| b.word_count()).sum::<usize>();
        self.block_texts.splice(start.block..=end.block, changed.iter().map(|b| Arc::from(b.text())));
        self.block_utf16.splice(
            start.block..=end.block,
            self.block_texts[start.block..=new_end].iter().map(|t| t.encode_utf16().count()),
        );
        self.layout_revision += 1;
        self.reveal_caret = true;
        self.spell_menu = None;
        self.suggestion_task = None;
        self.schedule_spellcheck(SPELL_AFTER, cx);
        self.anchor = pos;
        self.head = pos;
        self.goal_x = None;
        self.pending = Some(kind);
        self.typing_at = (kind == EditKind::Typing).then_some(pos);
        let elapsed = self.pending_since.get_or_insert_with(Instant::now).elapsed();
        self.touch();
        if kind == EditKind::Other {
            self.commit();
        } else {
            let delay = COMMIT_AFTER.min(MAX_UNSAVED.saturating_sub(elapsed));
            self.commit_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
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
        self.spell_menu = None;
        self.typing_at = None;
        self.suggestion_task = None;
        self.reveal_caret = true;
        self.head = pos;
        if !extend {
            self.anchor = pos;
        }
        self.touch();
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
        let Some(at) = layout.point_for_pos(self.head) else { return };
        let x = *self.goal_x.get_or_insert(at.x);
        let pos = layout.adjacent_row(self.head, down, x);
        self.move_to(pos, extend, cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.delete_selected_item(cx) || self.mode == Mode::Board {
            return;
        }
        if self.anchor == self.head {
            let prev = self.doc.prev_pos(self.head);
            if prev == self.head {
                return;
            }
            self.anchor = prev;
        }
        self.edit(EditKind::Deleting, cx, |doc, start, end| doc.delete(start, end));
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.delete_selected_item(cx) || self.mode == Mode::Board {
            return;
        }
        if self.anchor == self.head {
            let next = self.doc.next_pos(self.head);
            if next == self.head {
                return;
            }
            self.anchor = next;
        }
        self.edit(EditKind::Deleting, cx, |doc, start, end| doc.delete(start, end));
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            return;
        }
        self.edit(EditKind::Typing, cx, |doc, start, end| {
            let pos = doc.delete(start, end);
            doc.split_block(pos)
        });
    }

    fn toggle(&mut self, mark: Marks, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            self.update_board_style(
                |style| {
                    style.marks =
                        if style.marks.contains(mark) { style.marks.without(mark) } else { style.marks.with(mark) }
                },
                window,
                cx,
            );
            return;
        }
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

    fn set_block_kind(&mut self, kind: BlockKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            self.update_board_style(
                |style| {
                    style.typography.size = Some(match kind {
                        BlockKind::Heading(1) => 32,
                        BlockKind::Heading(_) => 24,
                        _ => 16,
                    });
                },
                window,
                cx,
            );
            return;
        }
        let (anchor, head) = (self.anchor, self.head);
        self.edit(EditKind::Other, cx, |doc, start, end| {
            for block in start.block..=end.block {
                let current = doc.blocks[block].kind;
                doc.set_block_kind(block, if current == kind { BlockKind::Paragraph } else { kind });
            }
            end
        });
        self.anchor = anchor;
        self.head = head;
    }

    /// Marks set on the whole selection (or on the character before the caret).
    fn active_marks(&self) -> Marks {
        if self.mode == Mode::Board {
            return self
                .board
                .iter()
                .find(|i| Some(i.id) == self.selected_item)
                .and_then(|i| i.kind.text())
                .map_or(Marks::NONE, |(_, s)| s.marks);
        }
        let (start, end) = self.selection();
        let Some(block) = self.doc.blocks.get(start.block) else { return Marks::NONE };
        if start == end || start.block != end.block {
            return block.marks_at(start.offset);
        }
        [Marks::BOLD, Marks::ITALIC, Marks::UNDERLINE, Marks::STRIKE, Marks::HIGHLIGHT]
            .into_iter()
            .filter(|m| block.all_have(start.offset, end.offset, *m))
            .fold(Marks::NONE, Marks::with)
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            if let Some(editing) = &self.editing {
                editing.state.update(cx, |s, cx| s.history(false, cx));
            } else {
                self.undo_board(cx);
            }
            return;
        }
        if !self.commit() {
            cx.notify();
            return;
        }
        let restored = self.library.borrow_mut().undo(self.id);
        self.apply_history(restored, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            if let Some(editing) = &self.editing {
                editing.state.update(cx, |s, cx| s.history(true, cx));
            }
            return;
        }
        if !self.commit() {
            cx.notify();
            return;
        }
        let restored = self.library.borrow_mut().redo(self.id);
        self.apply_history(restored, cx);
    }

    fn checkout(&mut self, rev: quill_store::RevId, cx: &mut Context<Self>) {
        if !self.commit() {
            cx.notify();
            return;
        }
        let restored = self.library.borrow_mut().checkout(self.id, rev).map(Some);
        self.apply_history(restored, cx);
    }

    fn apply_history(&mut self, restored: quill_store::Result<Option<(Document, Pos)>>, cx: &mut Context<Self>) {
        match restored {
            Ok(Some((doc, cursor))) => {
                self.doc = doc;
                self.words = self.doc.word_count();
                self.block_texts = self.doc.blocks.iter().map(|b| Arc::from(b.text())).collect();
                self.block_utf16 = self.block_texts.iter().map(|t| t.encode_utf16().count()).collect();
                self.layout_revision += 1;
                self.reveal_caret = true;
                self.spell_menu = None;
                self.suggestion_task = None;
                self.schedule_spellcheck(Duration::ZERO, cx);
                let cursor = self.clamp(cursor);
                self.anchor = cursor;
                self.head = cursor;
                self.marked = None;
                self.typing_at = None;
                self.touch();
                if self.history.is_some() {
                    self.history = Some(self.load_history());
                }
                cx.notify();
            }
            Ok(None) => {}
            Err(err) => eprintln!("roughcut: history step failed: {err}"),
        }
    }

    fn load_history(&self) -> Vec<Revision> {
        let mut revs = self.library.borrow().revisions(self.id).unwrap_or_default();
        revs.reverse();
        revs.truncate(HISTORY_SHOWN);
        revs
    }

    pub(crate) fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.commit();
        if self.history_slide.target() > 0.5 {
            self.history_slide.set(0., anim::QUICK);
        } else {
            self.history = Some(self.load_history());
            self.history_slide.set(1., anim::STANDARD);
        }
        cx.notify();
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
        if self.anchor != self.head && self.mode != Mode::Board {
            cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text()));
            self.edit(EditKind::Other, cx, |doc, start, end| doc.delete(start, end));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text.replace("\r\n", "\n"), EditKind::Other, cx);
        }
    }

    fn close(&mut self, _: &CloseDocument, _: &mut Window, cx: &mut Context<Self>) {
        if !self.commit() {
            cx.notify();
            return;
        }
        self.stop_editing_item(cx);
        cx.emit(EditorEvent::Close);
    }

    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.format_menu.take().is_some() {
            cx.notify();
            return;
        }
        if self.spell_menu.take().is_some() {
            cx.notify();
            return;
        }
        match self.mode {
            Mode::Focus => self.set_mode(Mode::Page, window, cx),
            Mode::Board if self.editing.is_some() || self.selected_item.is_some() => {
                self.stop_editing_item(cx);
                self.selected_item = None;
                cx.notify();
            }
            Mode::Board => self.set_mode(Mode::Page, window, cx),
            Mode::Page if self.history_slide.target() > 0.5 => self.toggle_history(cx),
            Mode::Page => self.close(&CloseDocument, window, cx),
        }
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        self.commit();
        let dir = directories::UserDirs::new()
            .and_then(|u| u.document_dir().map(std::path::PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        let name = format!("{}.md", self.doc.title.replace(['/', ':'], "-"));
        let target = cx.prompt_for_new_path(&dir, Some(&name));
        let doc = self.doc.clone();
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = target.await else { return };
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            let contents = match ext.as_deref() {
                Some("txt") => doc.plain_text() + "\n",
                _ => doc.to_markdown(),
            };
            if let Err(err) = std::fs::write(&path, contents) {
                eprintln!("roughcut: export to {} failed: {err}", path.display());
            }
        })
        .detach();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(layout) = &self.layout else { return };
        // Hit-test against the layout the user clicked on, before any zoom.
        let pos = layout.pos_for_point(event.position);
        let returning_to_page = self.mode != Mode::Focus
            && (self.mode == Mode::Board
                || (self.zoom.target() - 1.).abs() > 0.001
                || (self.board_origin.x - self.scroll.bounds().center().x).abs() > px(2.));
        if returning_to_page {
            self.set_mode(Mode::Page, window, cx);
        }
        window.focus(&self.focus_handle, cx);
        self.selecting = true;
        self.goal_x = None;
        self.move_to(pos, event.modifiers.shift, cx);
        if returning_to_page {
            self.center_page = true;
            self.reveal_caret = false;
            self.selecting = false;
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if let Some(layout) = &self.layout {
            let pos = layout.pos_for_point(event.position);
            if pos != self.head {
                self.head = pos;
                self.touch();
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
        self.block_utf16[..pos.block].iter().map(|n| n + 1).sum::<usize>()
            + self.block_texts[pos.block][..pos.offset].encode_utf16().count()
    }

    fn pos_from_utf16(&self, mut offset: usize) -> Pos {
        for (block, &len) in self.block_utf16.iter().enumerate() {
            if offset <= len {
                let mut units = 0;
                let mut bytes = 0;
                for ch in self.block_texts[block].chars() {
                    if units >= offset {
                        break;
                    }
                    units += ch.len_utf16();
                    bytes += ch.len_utf8();
                }
                return Pos::new(block, bytes);
            }
            offset -= len + 1;
        }
        self.doc.end()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> (Pos, Pos) {
        (self.pos_from_utf16(range.start), self.pos_from_utf16(range.end))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let saving = self.pending.is_some();
        let save_tip = self.save_error.clone().unwrap_or_else(|| "Changes are saved on this computer.".into());
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .h(px(52.))
            .child(
                icon_button("library", "chevron-left", "Library (⌘W)", false)
                    .on_click(cx.listener(|this, _, w, cx| this.close(&CloseDocument, w, cx))),
            )
            .child(
                div()
                    .min_w_0()
                    .max_w(px(340.))
                    .text_ellipsis()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.doc.title.clone()),
            )
            .child(
                div()
                    .id("save-status")
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(12.))
                    .text_color(if self.save_error.is_some() { rgb(0xfbbf24) } else { theme::muted() })
                    .child(
                        icon(if self.save_error.is_some() {
                            "rotate-ccw"
                        } else if saving {
                            "pen-line"
                        } else {
                            "check"
                        })
                        .size(px(13.))
                        .text_color(if self.save_error.is_some() {
                            rgb(0xfbbf24)
                        } else {
                            theme::muted()
                        }),
                    )
                    .child(if self.save_error.is_some() {
                        "Not saved · Retry"
                    } else if saving {
                        "Saving…"
                    } else {
                        "Saved locally"
                    })
                    .when(self.save_error.is_some(), |d| d.cursor_pointer())
                    .tooltip(move |window, cx| Tooltip::new(save_tip.clone()).build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.commit();
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(
                icon_button("focus", "scan-text", "Focus mode (⇧⌘F)", false)
                    .on_click(cx.listener(|this, _, window, cx| this.set_mode(Mode::Focus, window, cx))),
            )
            .child(
                icon_button("history", "clock", "Version history (⇧⌘Y)", self.history_slide.target() > 0.5)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_history(cx))),
            )
            .child(
                div()
                    .id("export")
                    .ml(px(4.))
                    .px(px(14.))
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .rounded(px(8.))
                    .bg(theme::accent())
                    .text_color(theme::on_accent())
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0x93adf3)))
                    .child(icon("share").size(px(15.)).text_color(theme::on_accent()))
                    .child("Export")
                    .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
            )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (start, _) = self.selection();
        let block_kind = self.doc.blocks.get(start.block).map(|b| b.kind);
        let marks = self.active_marks();
        let mark_button =
            |id: &'static str, glyph: &'static str, tip: &'static str, mark: Marks, cx: &mut Context<Self>| {
                icon_button(id, glyph, tip, marks.contains(mark))
                    .on_click(cx.listener(move |this, _, window, cx| this.toggle(mark, window, cx)))
            };
        let kind_button =
            |id: &'static str, glyph: &'static str, tip: &'static str, kind: BlockKind, cx: &mut Context<Self>| {
                icon_button(id, glyph, tip, block_kind == Some(kind))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_block_kind(kind, window, cx)))
            };
        div()
            .id("format-toolbar")
            .block_mouse_except_scroll()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .overflow_x_scroll()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(12.))
            .h(px(44.))
            .border_t_1()
            .border_color(theme::border())
            .child(
                icon_button("undo", "undo-2", "Undo (⌘Z)", false)
                    .on_click(cx.listener(|this, _, w, cx| this.undo(&Undo, w, cx))),
            )
            .child(
                icon_button("redo", "redo-2", "Redo (⇧⌘Z)", false)
                    .on_click(cx.listener(|this, _, w, cx| this.redo(&Redo, w, cx))),
            )
            .child(separator())
            .child(kind_button("h1", "heading-1", "Title", BlockKind::Heading(1), cx))
            .child(kind_button("h2", "heading-2", "Heading", BlockKind::Heading(2), cx))
            .child(kind_button("bullet", "list", "Bulleted list", BlockKind::Bullet, cx))
            .child(kind_button("numbered", "list-ordered", "Numbered list", BlockKind::Numbered, cx))
            .child(kind_button("quote", "text-quote", "Quote", BlockKind::Quote, cx))
            .child(separator())
            .child(mark_button("bold", "bold", "Bold (⌘B)", Marks::BOLD, cx))
            .child(mark_button("italic", "italic", "Italic (⌘I)", Marks::ITALIC, cx))
            .child(mark_button("underline", "underline", "Underline (⌘U)", Marks::UNDERLINE, cx))
            .child(mark_button("strike", "strikethrough", "Strikethrough (⇧⌘X)", Marks::STRIKE, cx))
            .child(mark_button("highlight", "highlighter", "Highlight (⇧⌘H)", Marks::HIGHLIGHT, cx))
            .children(self.format_controls(cx))
            .child(div().flex_1())
            .child(
                icon_button("spelling", "spell-check", "Check spelling (⌥⌘L)", self.spellcheck)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_spelling(cx))),
            )
            .child(
                div()
                    .px(px(8.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child(format!("{} words", crate::home::group(self.words as i64))),
            )
    }

    fn history_panel(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let slide = self.history_slide.sample(window);
        if slide <= 0.001 {
            return None;
        }
        let revisions = self.history.clone().unwrap_or_default();
        let head = self.library.borrow().head(self.id).ok();
        let tz = jiff::tz::TimeZone::system();
        let today = quill_store::today();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last_day = None;
        for (i, rev) in revisions.iter().enumerate() {
            let when = rev.created_at.to_zoned(tz.clone());
            let day = when.date();
            if last_day != Some(day) {
                last_day = Some(day);
                let label = if day == today {
                    "Today".to_string()
                } else if today.yesterday().ok() == Some(day) {
                    "Yesterday".to_string()
                } else {
                    day.strftime("%A, %B %-d").to_string()
                };
                rows.push(
                    div()
                        .px(px(18.))
                        .pt(px(16.))
                        .pb(px(6.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::muted())
                        .child(label.to_uppercase())
                        .into_any_element(),
                );
            }
            let current = Some(rev.id) == head;
            let change = revisions.get(i + 1).map(|older| rev.word_count - older.word_count);
            let rev_id = rev.id;
            rows.push(
                div()
                    .id(("rev", rev.id as u64))
                    .mx(px(8.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .when(current, |d| d.bg(theme::selected_bg()))
                    .when(!current, |d| d.hover(|s| s.bg(theme::raised())))
                    .child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .when(current, |d| d.bg(theme::accent()))
                            .when(!current, |d| d.border_1().border_color(rgb(0x4a505c))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(13.)).child(when.strftime("%-I:%M:%S %p").to_string()))
                            .child(div().text_size(px(12.)).text_color(theme::muted()).child(match change {
                                _ if current => format!("{} words · current", rev.word_count),
                                Some(c) if c > 0 => format!("{} words · +{c}", rev.word_count),
                                Some(c) if c < 0 => format!("{} words · {c}", rev.word_count),
                                _ => format!("{} words", rev.word_count),
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.checkout(rev_id, cx)))
                    .into_any_element(),
            );
        }
        Some(
            div()
                .w(px(HISTORY_WIDTH * slide))
                .h_full()
                .flex_none()
                .overflow_hidden()
                .bg(theme::surface())
                .border_l_1()
                .border_color(theme::border())
                .child(
                    div()
                        .id("history")
                        .w(px(HISTORY_WIDTH))
                        .h_full()
                        .flex()
                        .flex_col()
                        .opacity(slide)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .px(px(18.))
                                .h(px(48.))
                                .border_b_1()
                                .border_color(theme::border())
                                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("Version history"))
                                .child(
                                    icon_button("history-close", "x", "Close", false)
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_history(cx))),
                                ),
                        )
                        .child(
                            div().px(px(18.)).py(px(10.)).text_size(px(12.)).text_color(theme::muted()).child(
                                "Every change is kept. Click a version to restore it — later versions stay too.",
                            ),
                        )
                        .child(div().id("history-list").flex_1().overflow_y_scroll().pb(px(16.)).children(rows)),
                )
                .into_any_element(),
        )
    }

    fn media_card(&self, compact: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(error) = &self.media.read(cx).error {
            return Some(
                div()
                    .max_w(px(360.))
                    .p(px(12.))
                    .rounded(px(10.))
                    .bg(theme::raised())
                    .text_size(px(12.))
                    .text_color(theme::text_soft())
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        let now = self.media.read(cx).now.clone()?;
        let artwork = self.media.read(cx).artwork();
        let control = |id: &'static str, glyph: &'static str, size: f32, control: Control, media: Entity<Media>| {
            div()
                .id(id)
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(theme::raised()))
                .child(icon(glyph).size(px(size)).text_color(theme::text()))
                .on_click(move |_, _, cx| media.update(cx, |m, cx| m.control(control, cx)))
        };
        let media = self.media.clone();
        let play = if now.playing { "pause" } else { "play" };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(8.))
                .pr(px(10.))
                .rounded(px(16.))
                .bg(rgba(0x1d2027f2))
                .border_1()
                .border_color(theme::border())
                .shadow(floating_shadow())
                .when(!compact, |d| {
                    d.child(match artwork {
                        Some(image) => img(image).size(px(44.)).rounded(px(8.)).into_any_element(),
                        None => div()
                            .size(px(44.))
                            .rounded(px(8.))
                            .bg(theme::selected_bg())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("music").size(px(18.)).text_color(rgb(0x9db4f5)))
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .w(px(150.))
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_ellipsis()
                                    .child(now.title.clone()),
                            )
                            .child(div().text_size(px(12.)).text_color(theme::muted()).text_ellipsis().child(format!(
                                "{} · {}",
                                now.player.name(),
                                now.artist
                            )))
                            .child(div().mt(px(7.)).h(px(3.)).rounded(px(2.)).bg(theme::border()).child(
                                div().h_full().rounded(px(2.)).w(relative(now.progress)).bg(theme::text_soft()),
                            )),
                    )
                })
                .when(compact, |d| {
                    d.child(
                        div()
                            .max_w(px(240.))
                            .text_size(px(13.))
                            .text_color(theme::muted())
                            .text_ellipsis()
                            .child(format!("{} · {}", now.title, now.artist)),
                    )
                })
                .when(!compact, |d| d.child(control("prev", "skip-back", 15., Control::Previous, media.clone())))
                .child(control("playpause", play, 16., Control::PlayPause, media.clone()))
                .child(control("next", "skip-forward", 15., Control::Next, media))
                .into_any_element(),
        )
    }

    fn page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let z = self.page_scale();
        div()
            .relative()
            .w(px((TEXT_WIDTH + PAGE_PADDING.0 * 2.) * z))
            .min_h(px(PAGE_MIN_HEIGHT * z))
            .px(px(PAGE_PADDING.0 * z))
            .py(px(PAGE_PADDING.1 * z))
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(PageElement { editor: cx.entity() })
            .when(self.doc.blocks.len() == 1 && self.doc.blocks[0].is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(PAGE_PADDING.0 * z))
                        .top(px(PAGE_PADDING.1 * z))
                        .font_family(theme::PAGE_FONT)
                        .text_size(px(18. * z))
                        .line_height(px(28.8 * z))
                        .text_color(theme::muted())
                        .opacity(0.65)
                        .child("Start writing…"),
                )
            })
    }

    /// How big the board must be (at the current zoom) to hold the page and
    /// every item.
    fn board_extent(&self) -> (f32, f32) {
        let z = self.page_scale();
        let text_h = self.layout.as_ref().map_or(0., |l| f32::from(l.height) / z.max(0.01));
        let page_h = PAGE_MIN_HEIGHT.max(text_h + PAGE_PADDING.1 * 2.);
        let mut half_w = TEXT_WIDTH / 2. + PAGE_PADDING.0;
        let mut bottom = page_h;
        for item in &self.board {
            half_w = half_w.max(item.x.abs()).max((item.x + item.w).abs());
            bottom = bottom.max(item.y + item.h);
        }
        ((half_w * 2. + 160.) * z, (bottom + 240.) * z + PAGE_TOP)
    }

    fn render_focus(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let written = self.words.saturating_sub(self.session_start_words);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x121419))
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(20.))
                    .h(px(52.))
                    .child(div().text_size(px(13.)).text_color(theme::muted()).child(self.doc.title.clone()))
                    .child(div().flex_1())
                    .child(
                        icon_button("exit-focus", "x", "Exit focus (Esc)", false)
                            .on_click(cx.listener(|this, _, window, cx| this.set_mode(Mode::Page, window, cx))),
                    ),
            )
            .child(
                div()
                    .id("focus-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.focus_scroll)
                    .child(div().flex().justify_center().pt(px(40.)).pb(px(160.)).child(self.page(cx))),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .items_center()
                    .gap(px(24.))
                    .h(px(64.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(format!("{} words this session", crate::home::group(written as i64)))
                    .children(self.media_card(true, cx)),
            )
            .with_animation("focus-enter", Animation::new(anim::STANDARD).with_easing(anim::ease_out), |el, t| {
                el.opacity(t)
            })
    }
}

/// Hooks for the UI integration tests.
#[doc(hidden)]
impl EditorView {
    /// Called after mounting a document so the old library search cannot retain focus.
    pub fn focus_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(Mode::Page, window, cx);
        self.anchor = Pos::default();
        self.head = Pos::default();
        self.center_page = true;
        self.caret_on = true;
        cx.notify();
    }

    /// Use the laid-out canvas origin and viewport, not dimensions from the previous frame.
    pub(crate) fn record_board_origin(&mut self, origin: Point<Pixels>, cx: &mut Context<Self>) {
        self.board_origin = origin;
        if !self.center_page {
            return;
        }
        let viewport = self.scroll.bounds();
        if viewport.size.width <= px(0.) {
            return;
        }
        self.reveal_caret = false;
        let offset = self.scroll.offset();
        let target = point((offset.x + viewport.center().x - origin.x).min(px(0.)), px(0.));
        if (target.x - offset.x).abs() > px(0.5) || target.y != offset.y {
            self.scroll.set_offset(target);
            cx.notify();
        } else if !self.zoom.animating() {
            self.center_page = false;
        }
    }

    pub fn page_count(&self) -> usize {
        self.layout.as_ref().map_or(1, |layout| layout.pages)
    }

    /// Window position of a document position, as of the last frame.
    pub fn point_for(&self, pos: Pos) -> Option<Point<Pixels>> {
        self.layout.as_ref()?.point_for_pos(pos)
    }

    pub fn spell_menu(&self) -> Option<(String, Vec<String>, Point<Pixels>)> {
        self.spell_menu.as_ref().map(|m| (m.word.clone(), m.suggestions.clone(), m.at))
    }

    pub fn shaped_blocks(&self) -> usize {
        self.layout_cache.shaped_blocks
    }

    pub fn word_count(&self) -> usize {
        self.words
    }

    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.pending.is_some()
    }

    pub fn spelling_ready(&self) -> bool {
        !self.spellcheck || self.spell_ready == self.spell_generation
    }

    pub fn suggestions_ready(&self) -> bool {
        self.spell_menu.as_ref().is_none_or(|m| !m.loading)
    }

    pub fn canvas_bounds(&self) -> Bounds<Pixels> {
        self.scroll.bounds()
    }
}

pub(crate) fn icon_button(id: &'static str, glyph: &'static str, tip: &'static str, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor_pointer()
        .when(active, |d| d.bg(theme::selected_bg()))
        .when(!active, |d| d.hover(|s| s.bg(theme::raised())))
        .child(icon(glyph).size(px(17.)).text_color(if active { theme::accent() } else { theme::text_soft() }))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
}

fn separator() -> Div {
    div().w(px(1.)).h(px(20.)).mx(px(6.)).bg(theme::border())
}

pub(crate) fn floating_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: hsla(0., 0., 0., 0.35),
            offset: point(px(0.), px(10.)),
            blur_radius: px(30.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: hsla(0., 0., 0., 0.25),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        },
    ]
}

/// The board's dot grid, painted across the whole board.
fn dot_grid(zoom: f32, opacity: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if opacity <= 0.01 {
                return;
            }
            let step = px(DOT_SPACING * zoom);
            let dot = px(1.6 * zoom.max(0.6));
            let color = hsla(220. / 360., 0.12, 0.32, opacity);
            let visible = window.content_mask().bounds.intersect(&bounds);
            let start = bounds.origin + point(step / 2., step / 2.);
            let mut y = start.y + step * (f32::from(visible.top() - start.y) / f32::from(step)).ceil().max(0.);
            while y < visible.bottom() {
                let mut x = start.x + step * (f32::from(visible.left() - start.x) / f32::from(step)).ceil().max(0.);
                while x < visible.right() {
                    window.paint_quad(fill(Bounds::new(point(x, y), size(dot, dot)), color).corner_radii(dot / 2.));
                    x += step;
                }
                y += step;
            }
        },
    )
    .absolute()
    .size_full()
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep frames coming while anything is mid-animation.
        let zoom = self.zoom.sample(window);
        self.render_scale = zoom;

        let root = div()
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
            .on_action(cx.listener(|this, _: &LineStart, _, cx| this.move_to(Pos::new(this.head.block, 0), false, cx)))
            .on_action(cx.listener(|this, _: &LineEnd, _, cx| {
                let block = this.head.block;
                this.move_to(Pos::new(block, this.doc.blocks[block].len()), false, cx)
            }))
            .on_action(cx.listener(|this, _: &DocStart, _, cx| this.move_to(Pos::default(), false, cx)))
            .on_action(cx.listener(|this, _: &DocEnd, _, cx| this.move_to(this.doc.end(), false, cx)))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|this, _: &Bold, window, cx| this.toggle(Marks::BOLD, window, cx)))
            .on_action(cx.listener(|this, _: &Italic, window, cx| this.toggle(Marks::ITALIC, window, cx)))
            .on_action(cx.listener(|this, _: &Underline, window, cx| this.toggle(Marks::UNDERLINE, window, cx)))
            .on_action(cx.listener(|this, _: &Strike, window, cx| this.toggle(Marks::STRIKE, window, cx)))
            .on_action(cx.listener(|this, _: &Highlight, window, cx| this.toggle(Marks::HIGHLIGHT, window, cx)))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(|this, _: &ToggleFocus, window, cx| {
                let next = if this.mode == Mode::Focus { Mode::Page } else { Mode::Focus };
                this.set_mode(next, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleBoard, window, cx| {
                let next = if this.mode == Mode::Board { Mode::Page } else { Mode::Board };
                this.set_mode(next, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleHistory, _, cx| this.toggle_history(cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_step(true, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_step(false, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom_to(1., None, true, cx)))
            .on_action(cx.listener(|this, _: &ToggleSpelling, _, cx| this.toggle_spelling(cx)))
            .children(self.spell_menu_element(cx))
            .children(self.format_menu_element(cx));

        if self.mode == Mode::Focus {
            return root.child(self.render_focus(cx)).into_any_element();
        }

        let (board_w, board_h) = self.board_extent();
        let history = self.history_panel(window, cx);
        root.child(
            div()
                .flex()
                .flex_col()
                .bg(theme::surface())
                .border_b_1()
                .border_color(theme::border())
                .child(self.header(cx))
                .child(self.toolbar(cx)),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.tool_bar(cx))
                .child(
                    div().relative().flex_1().min_w_0().h_full().child(
                        div().id("board-scroll").size_full().overflow_scroll().track_scroll(&self.scroll).child(
                            div()
                                .id("board")
                                .relative()
                                .min_w(px(board_w))
                                .w_full()
                                .h(px(board_h))
                                .when(self.mode == Mode::Board && self.tool != Tool::Select, |d| d.cursor_crosshair())
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::board_mouse_down))
                                .on_mouse_move(cx.listener(Self::board_mouse_move))
                                .on_mouse_up(MouseButton::Left, cx.listener(Self::board_mouse_up))
                                .on_pinch(cx.listener(Self::on_pinch))
                                .on_scroll(cx.listener(Self::on_scroll_wheel))
                                .child(dot_grid(zoom, 1.))
                                .child(div().flex().justify_center().pt(px(PAGE_TOP)).child(self.page(cx)))
                                .child(self.render_board(1., window, cx)),
                        ),
                    ),
                )
                .children(history),
        )
        .children(self.media_card(false, cx).map(|card| {
            div()
                .id("media-dock")
                .flex_none()
                .flex()
                .justify_end()
                .items_center()
                .px(px(12.))
                .py(px(8.))
                .bg(theme::surface())
                .border_t_1()
                .border_color(theme::border())
                .block_mouse_except_scroll()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .child(card)
        }))
        .into_any_element()
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

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let (start, end) = self.selection();
        Some(UTF16Selection { range: self.to_utf16(start)..self.to_utf16(end), reversed: self.head < self.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.map(|(start, end)| self.to_utf16(start)..self.to_utf16(end))
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
        if self.mode == Mode::Board {
            return;
        }
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
        if self.mode == Mode::Board {
            return;
        }
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
        Some(Bounds::from_corners(a, point(b.x.max(a.x + px(1.)), b.y + lh)))
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

    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        self.mode != Mode::Board
    }
}
