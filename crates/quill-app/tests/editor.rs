//! Drives the real editor in an offscreen window with real mouse, trackpad
//! and keyboard events — every whiteboard tool, zooming and spellcheck — and
//! saves screenshots to `target/ui-shots/` for review.
//!
//! cargo test -p quill --features screenshots --test editor

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use quill::assets::Assets;
use quill::board::Tool;
use quill::editor::{EditorView, MENU_PADDING, MENU_ROW, Mode};
use quill::media::Media;
use quill_doc::{Document, Pos};
use quill_store::{DocId, ItemKind, Library};

struct Harness {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    editor: Entity<EditorView>,
    library: Rc<RefCell<Library>>,
    id: DocId,
    name: &'static str,
    shots: usize,
}

fn harness(name: &'static str) -> Harness {
    harness_with(name, "The lamp had burned every night for forty-one years.")
}

fn harness_with(name: &'static str, text: &str) -> Harness {
    harness_seeded(name, text, |_, _| {}, None, false)
}

fn harness_seeded(
    name: &'static str,
    text: &str,
    seed: impl FnOnce(&mut Library, DocId),
    playback: Option<quill::media::NowPlaying>,
    opening: bool,
) -> Harness {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        quill::editor::bind_keys(cx);
    });
    let mut library = Library::open_in_memory().unwrap();
    let doc = Document::from_plain_text("Test", text);
    let id = library.create(&doc, None).unwrap();
    seed(&mut library, id);
    let library = Rc::new(RefCell::new(library));
    let lib = library.clone();
    let (window, editor) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(1440.), px(900.)),
                    })),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    let media = cx.new(|_| {
                        let mut media = Media::idle();
                        media.now = playback;
                        media
                    });
                    let speller = Rc::new(RefCell::new(quill_spell::Speller::english()));
                    let editor = cx.new(|cx| EditorView::new(lib, id, doc, media, speller, cx));
                    window.focus(&editor.focus_handle(cx), cx);
                    if opening {
                        let opened = editor.clone();
                        window.on_next_frame(move |window, cx| opened.update(cx, |e, cx| e.focus_document(window, cx)));
                    }
                    editor
                },
            )
        })
        .unwrap();
    let mut h = Harness { cx, window, editor, library, id, name, shots: 0 };
    h.settle();
    h.wait_for(|e| e.spelling_ready());
    h
}

impl Harness {
    fn wait_for(&mut self, ready: impl Fn(&EditorView) -> bool) {
        for _ in 0..200 {
            if self.read(&ready) {
                return;
            }
            self.cx.advance_clock(Duration::from_millis(20));
            std::thread::sleep(Duration::from_millis(2));
            self.frame();
        }
        assert!(self.read(ready), "background editor work did not finish");
    }

    fn frame(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, w, cx| {
                w.simulate_next_frame(cx);
                w.refresh();
                w.draw(cx).clear(cx);
            })
            .unwrap();
        self.cx.run_until_parked();
    }

    /// Animations run on wall-clock time; let them finish.
    /// GPUI's element animations run on the test clock and the editor's
    /// zoom on the wall clock; advance both.
    fn settle(&mut self) {
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(90));
            self.cx.advance_clock(Duration::from_millis(90));
            self.frame();
        }
    }

    fn right_click(&mut self, at: Point<Pixels>) {
        self.mv(at, false);
        self.event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        self.event(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        self.wait_for(|e| e.suggestions_ready());
    }

    /// The middle of a word on the page, for clicking on it.
    fn word_at(&mut self, block: usize, offset: usize) -> Point<Pixels> {
        let top = self.read(|e| e.point_for(Pos::new(block, offset))).expect("laid out");
        point(top.x + px(2.), top.y + px(12.))
    }

    /// Clicks row `row` of the open spelling menu.
    fn click_menu_row(&mut self, row: f32) {
        let (_, _, at) = self.read(|e| e.spell_menu()).expect("menu open");
        let target = point(at.x + px(40.), at.y + px(MENU_PADDING + MENU_ROW * row + MENU_ROW / 2.));
        self.click(target);
    }

    fn misspelled(&mut self) -> Vec<String> {
        self.read(|e| {
            let text = e.doc.blocks.iter().map(|b| b.text()).collect::<Vec<_>>();
            e.misspellings()
                .iter()
                .enumerate()
                .flat_map(|(i, ranges)| ranges.iter().map(|r| text[i][r.clone()].to_string()).collect::<Vec<_>>())
                .collect()
        })
    }

    fn event(&mut self, event: impl InputEvent) {
        let input = event.to_platform_input();
        self.cx.update_window(self.window, |_, w, cx| w.dispatch_event(input, cx)).unwrap();
        self.frame();
    }

    fn down(&mut self, at: Point<Pixels>, clicks: usize) {
        self.event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: clicks,
            first_mouse: false,
        });
    }

    fn up(&mut self, at: Point<Pixels>, clicks: usize) {
        self.event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: clicks,
        });
    }

    fn mv(&mut self, at: Point<Pixels>, pressed: bool) {
        self.event(MouseMoveEvent {
            position: at,
            pressed_button: pressed.then_some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
    }

    fn click(&mut self, at: Point<Pixels>) {
        self.mv(at, false);
        self.down(at, 1);
        self.up(at, 1);
    }

    fn double_click(&mut self, at: Point<Pixels>) {
        self.click(at);
        self.down(at, 2);
        self.up(at, 2);
    }

    fn drag(&mut self, from: Point<Pixels>, to: Point<Pixels>) {
        self.mv(from, false);
        self.down(from, 1);
        for i in 1..=12 {
            let t = i as f32 / 12.;
            self.mv(point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t), true);
        }
        self.up(to, 1);
    }

    fn keys(&mut self, keys: &'static str) {
        for key in keys.split(' ') {
            let keystroke = Keystroke::parse(key).unwrap();
            self.cx.update_window(self.window, |_, w, cx| w.dispatch_keystroke(keystroke, cx)).unwrap();
            self.frame();
        }
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            let key = if ch == ' ' { "space".to_string() } else { ch.to_string() };
            let keystroke = Keystroke { modifiers: Modifiers::default(), key, key_char: Some(ch.to_string()) };
            self.cx.update_window(self.window, |_, w, cx| w.dispatch_keystroke(keystroke, cx)).unwrap();
        }
        self.frame();
    }

    fn read<R>(&mut self, f: impl FnOnce(&EditorView) -> R) -> R {
        let editor = self.editor.clone();
        self.cx.update(|cx| f(editor.read(cx)))
    }

    fn update(&mut self, f: impl FnOnce(&mut EditorView, &mut Window, &mut Context<EditorView>)) {
        let editor = self.editor.clone();
        self.cx.update_window(self.window, |_, w, cx| editor.update(cx, |e, cx| f(e, w, cx))).unwrap();
        self.frame();
    }

    /// Board coordinates (page pixels, origin at the page's top centre) to window.
    fn at(&mut self, x: f32, y: f32) -> Point<Pixels> {
        self.read(|e| e.board_to_window(x, y))
    }

    fn board_mode(&mut self) {
        self.keys("cmd-shift-b");
        self.settle();
        assert_eq!(self.read(|e| e.mode), Mode::Board, "⇧⌘B should switch to the board");
    }

    fn tool(&mut self, tool: Tool) {
        self.update(|e, _, cx| e.set_tool(tool, cx));
    }

    fn items(&mut self) -> Vec<quill_store::BoardItem> {
        self.read(|e| e.board_items().to_vec())
    }

    fn stored(&self) -> Vec<quill_store::BoardItem> {
        self.library.borrow().board(self.id).unwrap()
    }

    fn shot(&mut self, label: &str) {
        self.settle();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-shots");
        std::fs::create_dir_all(&dir).unwrap();
        self.shots += 1;
        let path = dir.join(format!("{}-{}-{label}.png", self.name, self.shots));
        match self.cx.capture_screenshot(self.window) {
            Ok(image) => image.save(&path).unwrap(),
            Err(err) => eprintln!("  (no screenshot: {err})"),
        }
    }
}

fn text_of(item: &quill_store::BoardItem) -> &str {
    match &item.kind {
        ItemKind::Text { text, .. } => text,
        _ => "",
    }
}

// --- The tests -------------------------------------------------------------

fn board_toolbar_clicks_do_not_reach_the_page() {
    let mut h = harness("toolbar-clicks");
    let selection = h.read(|e| e.selection());
    // Actual pointer clicks in the 1440×900 fixture, rather than set_tool().
    h.click(point(px(38.), px(125.)));
    h.settle();
    assert_eq!(h.read(|e| e.mode), Mode::Board);
    assert_eq!(h.read(|e| e.selection()), selection, "opening the toolbar must not click the page underneath");
    for (i, tool) in Tool::ALL.into_iter().enumerate() {
        let at = point(px(38.), px(125. + i as f32 * 40.));
        h.down(at, 1);
        assert_eq!(h.read(|e| e.mode), Mode::Board, "pressing {tool:?} must keep the toolbar open");
        h.up(at, 1);
        assert_eq!(h.read(|e| e.active_tool()), tool, "clicking must activate {tool:?}");
        assert_eq!(h.read(|e| e.mode), Mode::Board);
    }
    for (x, y) in [(24., 418.), (52., 418.), (24., 446.), (52., 446.), (38., 493.), (38., 525.), (38., 557.)] {
        h.click(point(px(x), px(y)));
        assert_eq!(h.read(|e| e.mode), Mode::Board, "color and zoom controls must keep the toolbar open");
    }
    assert!(h.items().is_empty(), "toolbar clicks must not create canvas items");
    h.shot("left-sidebar");
}

fn double_click_board_creates_editable_text() {
    let mut h = harness("board-text");
    h.board_mode();
    h.tool(Tool::Select);
    let spot = h.at(-620., 200.);
    h.double_click(spot);
    let items = h.items();
    assert_eq!(items.len(), 1, "double-clicking the board adds text");
    assert!(matches!(items[0].kind, ItemKind::Text { .. }));
    assert_eq!(h.read(|e| e.editing_item()), Some(items[0].id), "the new note is ready to type into");

    h.type_text("Buy milk");
    h.shot("typing");
    assert_eq!(text_of(&h.items()[0]), "Buy milk", "typing goes into the note");

    let empty = h.at(620., 900.);
    h.click(empty);
    assert_eq!(h.read(|e| e.editing_item()), None, "clicking away finishes editing");
    let stored = h.stored();
    assert_eq!(stored.len(), 1, "the note survives finishing editing: {stored:?}");
    assert_eq!(text_of(&stored[0]), "Buy milk", "the text is saved");
    assert_eq!(h.items().len(), 1, "clicking away doesn't make another note");
    h.shot("kept");
}

fn double_click_edits_an_existing_note() {
    let mut h = harness("edit-note");
    h.board_mode();
    h.tool(Tool::Text);
    let spot = h.at(-620., 200.);
    h.click(spot);
    h.type_text("First");
    let away = h.at(620., 900.);
    h.click(away);
    assert_eq!(h.read(|e| e.editing_item()), None);

    let note = h.items()[0].clone();
    let inside = h.at(note.x + note.w / 2., note.y + note.h / 2.);
    h.double_click(inside);
    assert_eq!(h.read(|e| e.editing_item()), Some(note.id), "double-click opens the note for editing");
    h.keys("cmd-down");
    h.type_text(" draft");
    let away = h.at(620., 900.);
    h.click(away);
    assert_eq!(text_of(&h.stored()[0]), "First draft", "edits to an existing note are saved");
}

fn text_box_is_created_and_kept() {
    let mut h = harness("text");
    h.board_mode();
    h.tool(Tool::Text);
    let spot = h.at(480., 120.);
    h.click(spot);
    assert_eq!(h.items().len(), 1);
    h.type_text("Maybe the last line");
    h.shot("typing");
    let away = h.at(-620., 900.);
    h.click(away);
    let stored = h.stored();
    assert_eq!(stored.len(), 1, "the text box is kept: {stored:?}");
    assert_eq!(text_of(&stored[0]), "Maybe the last line");
    h.shot("kept");
}

fn empty_note_disappears_when_left() {
    let mut h = harness("empty-note");
    h.board_mode();
    h.tool(Tool::Text);
    let spot = h.at(-620., 200.);
    h.click(spot);
    let away = h.at(620., 900.);
    h.click(away);
    assert!(h.stored().is_empty(), "an untouched new note isn't left behind");
}

fn pen_and_highlighter_draw_strokes() {
    let mut h = harness("pen");
    h.board_mode();
    h.tool(Tool::Pen);
    let (a, b) = (h.at(-640., 300.), h.at(-480., 420.));
    h.drag(a, b);
    h.tool(Tool::Highlighter);
    let (a, b) = (h.at(-640., 480.), h.at(-460., 480.));
    h.drag(a, b);
    let items = h.stored();
    assert_eq!(items.len(), 2, "each drag makes one stroke: {items:?}");
    match &items[0].kind {
        ItemKind::Stroke { points, width, .. } => {
            assert!(points.len() >= 8, "the stroke follows the pointer ({} points)", points.len());
            assert!(*width < 5.);
        }
        other => panic!("expected a pen stroke, got {other:?}"),
    }
    assert!(matches!(items[1].kind, ItemKind::Stroke { width, .. } if width > 10.), "highlighter is wide");
    h.shot("strokes");
}

fn shape_tool_drags_out_a_rectangle() {
    let mut h = harness("shape");
    h.board_mode();
    h.tool(Tool::Shape);
    let (a, b) = (h.at(450., 200.), h.at(650., 330.));
    h.drag(a, b);
    let items = h.stored();
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0].kind, ItemKind::Shape { color: 0xfde68a, .. }));
    assert!((items[0].w - 200.).abs() < 6. && (items[0].h - 130.).abs() < 6., "shape matches the drag: {:?}", items[0]);
    let away = h.at(-620., 500.);
    h.click(away);
    let inside = h.at(550., 260.);
    h.click(inside);
    assert_eq!(h.read(|e| e.selected()), Some(items[0].id));
    h.click(point(px(52.), px(418.)));
    assert!(
        matches!(h.stored()[0].kind, ItemKind::Shape { color: 0xfbcfe8, .. }),
        "the palette must recolor and save the selected shape"
    );
    h.mv(point(px(100.), px(100.)), false);
    h.shot("filled-and-recolored");
    let shot = h.cx.capture_screenshot(h.window).unwrap();
    let scale = shot.width() as f32 / 1440.;
    // Sample away from the centered text caret, which now appears on a shape click.
    let pixel = shot.get_pixel((f32::from(inside.x + px(20.)) * scale) as u32, (f32::from(inside.y) * scale) as u32);
    for (actual, expected) in pixel.0[..3].iter().zip([251u8, 207, 232]) {
        assert!(actual.abs_diff(expected) <= 3, "the rectangle interior must be filled: {pixel:?}");
    }
}

fn double_click_blank_board_from_page_mode_starts_text() {
    let mut h = harness("page-to-board-text");
    let at = h.at(-600., 250.);
    h.double_click(at);
    assert_eq!(h.read(|e| e.mode), Mode::Board);
    assert_eq!(h.items().len(), 1);
    h.type_text("First line");
    h.keys("enter");
    h.type_text("Second line");
    h.keys("enter");
    h.type_text("Third line");
    h.keys("enter");
    h.type_text("Fourth line");
    assert_eq!(text_of(&h.stored()[0]), "First line\nSecond line\nThird line\nFourth line");
    assert!(h.items()[0].h >= 96., "the text box grows to fit all four lines");
    let away = h.at(620., 900.);
    h.click(away);
    h.shot("multiline-text");
}

fn clicking_page_restores_actual_size_and_centers_after_panning() {
    let mut h = harness("recenter-page");
    // A distant image makes the board much wider than the viewport.
    let png = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/recenter-image.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([120, 150, 240, 255])).save(&png).unwrap();
    h.update(|e, _, cx| e.insert_image_from(&png, point(px(1500.), px(600.)), cx));
    h.board_mode();
    let viewport = h.read(|e| e.canvas_bounds());
    let position = viewport.center();
    h.event(ScrollEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(-700.), px(-100.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    let page = h.at(0., 150.);
    h.click(page);
    h.settle();
    assert_eq!(h.read(|e| e.mode), Mode::Page);
    assert!((h.read(|e| e.zoom_level()) - 1.).abs() < 0.001);
    let origin = h.at(0., 0.);
    assert!(
        (origin.x - viewport.center().x).abs() < px(2.),
        "page center must match canvas center: {origin:?} / {viewport:?}"
    );
    assert!((origin.y - viewport.top() - px(quill::editor::PAGE_TOP)).abs() < px(2.));
    // Also reset page-mode zoom (not only the Board -> Page transition).
    h.keys("cmd-=");
    h.settle();
    assert!(h.read(|e| e.zoom_level()) > 1.);
    let page = h.at(0., 100.);
    h.click(page);
    h.settle();
    assert!((h.read(|e| e.zoom_level()) - 1.).abs() < 0.001);
    assert!((h.at(0., 0.).x - viewport.center().x).abs() < px(2.));
    // A horizontal pan at 100% also returns to center on a page click.
    h.event(ScrollEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(-150.), px(0.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    let page = h.at(0., 100.);
    h.click(page);
    h.settle();
    assert!((h.at(0., 0.).x - viewport.center().x).abs() < px(2.));
    h.shot("centered-at-100");
}

fn select_drags_a_note_and_delete_and_undo_work() {
    let mut h = harness("select");
    h.board_mode();
    h.tool(Tool::Text);
    let spot = h.at(-620., 200.);
    h.click(spot);
    h.type_text("Move me");
    let away = h.at(620., 900.);
    h.click(away);
    let before = h.stored()[0].clone();

    h.tool(Tool::Select);
    let (from, to) = (h.at(before.x + 40., before.y + 40.), h.at(before.x + 140., before.y + 90.));
    h.drag(from, to);
    let after = h.stored()[0].clone();
    assert!(
        (after.x - before.x - 100.).abs() < 3. && (after.y - before.y - 50.).abs() < 3.,
        "dragging moves the note: {before:?} -> {after:?}"
    );
    assert_eq!(h.read(|e| e.selected()), Some(after.id));
    h.shot("moved");

    h.keys("backspace");
    assert!(h.stored().is_empty(), "Delete removes the selected note");
    h.keys("cmd-z");
    assert_eq!(h.stored().len(), 1, "⌘Z brings it back");
}

fn eraser_removes_what_it_touches() {
    let mut h = harness("eraser");
    h.board_mode();
    h.tool(Tool::Shape);
    let (a, b) = (h.at(450., 200.), h.at(650., 330.));
    h.drag(a, b);
    h.tool(Tool::Eraser);
    let inside = h.at(550., 260.);
    h.click(inside);
    assert!(h.stored().is_empty(), "the eraser removes the shape");
}

fn image_tool_places_an_image() {
    let mut h = harness("image");
    h.board_mode();
    let png = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/whiteboard-test.png");
    image::RgbaImage::from_pixel(64, 48, image::Rgba([120, 150, 240, 255])).save(&png).unwrap();
    h.update(|e, _, cx| e.insert_image_from(&png, point(px(-560.), px(600.)), cx));
    let items = h.stored();
    assert!(
        matches!(items.as_slice(), [item] if matches!(item.kind, ItemKind::Image { .. }) && item.w == 64.),
        "{items:?}"
    );
    h.shot("image");
}

fn clicking_page_and_board_switches_modes() {
    let mut h = harness("modes");
    h.board_mode();
    let page = h.at(0., 150.);
    h.click(page);
    h.settle();
    assert_eq!(h.read(|e| e.mode), Mode::Page, "clicking the page starts writing");
    assert!((h.read(|e| e.zoom_level()) - 1.).abs() < 0.01, "and zooms to full size");
    h.type_text(" Then the storm.");
    let text = h.read(|e| e.doc.plain_text());
    assert!(text.contains("Then the storm."), "typing on the page works: {text}");
    let board = h.at(-640., 600.);
    h.click(board);
    h.settle();
    assert_eq!(h.read(|e| e.mode), Mode::Board, "clicking the board goes back to it");
}

fn pinch_and_keyboard_zoom() {
    let mut h = harness("zoom");
    h.board_mode();
    let start = h.read(|e| e.zoom_level());
    let center = h.at(0., 300.);
    // A trackpad pinch always has a pointer position over the canvas.
    h.mv(center, false);
    for _ in 0..4 {
        h.event(PinchEvent {
            position: center,
            delta: -0.1,
            modifiers: Modifiers::default(),
            phase: TouchPhase::Moved,
        });
    }
    let pinched = h.read(|e| e.zoom_level());
    assert!(pinched < start - 0.15, "pinching in zooms out: {start} -> {pinched}");
    h.shot("pinched-out");
    for _ in 0..8 {
        h.event(PinchEvent {
            position: center,
            delta: 0.15,
            modifiers: Modifiers::default(),
            phase: TouchPhase::Moved,
        });
    }
    assert!(h.read(|e| e.zoom_level()) > 1.2, "pinching out zooms in past 100%");
    h.keys("cmd-0");
    h.settle();
    assert!((h.read(|e| e.zoom_level()) - 1.).abs() < 0.01, "⌘0 returns to 100%");
    h.keys("cmd--");
    h.settle();
    assert!(h.read(|e| e.zoom_level()) < 0.95, "⌘− zooms out");
    h.keys("cmd-=");
    h.settle();
    assert!((h.read(|e| e.zoom_level()) - 1.).abs() < 0.05, "⌘= zooms back in");
}

fn misspelled_word_is_underlined_and_corrected_from_the_menu() {
    let mut h = harness_with("spelling", "The wind smelled of rain and kelpe.");
    assert_eq!(h.misspelled(), ["kelpe"]);
    let start = h.read(|e| e.doc.blocks[0].text().find("kelpe").unwrap());
    let word = h.word_at(0, start + 2);
    h.right_click(word);
    let (menu_word, suggestions, _) = h.read(|e| e.spell_menu()).expect("right-clicking a misspelling opens the menu");
    assert_eq!(menu_word, "kelpe");
    let kelp = suggestions.iter().position(|w| w == "kelp").expect("kelp is suggested");
    h.shot("menu");
    h.click_menu_row(kelp as f32);
    assert_eq!(h.read(|e| e.doc.plain_text()), "The wind smelled of rain and kelp.");
    assert!(h.misspelled().is_empty());
    assert!(h.read(|e| e.spell_menu()).is_none(), "choosing a suggestion closes the menu");
    h.shot("fixed");
}

fn learn_spelling_is_remembered() {
    let mut h = harness_with("learn", "Elias Marr kept the light.");
    assert_eq!(h.misspelled(), ["Marr"]);
    let word = h.word_at(0, 7);
    h.right_click(word);
    let (_, suggestions, _) = h.read(|e| e.spell_menu()).expect("menu open");
    // Suggestions (or "No guesses"), a divider, then Learn Spelling.
    let rows = suggestions.len().max(1) as f32;
    h.click_menu_row(rows + 9. / MENU_ROW);
    h.wait_for(|e| e.spelling_ready());
    assert!(h.misspelled().is_empty(), "the learned word is no longer flagged");
    assert_eq!(h.library.borrow().learned_words().unwrap(), ["Marr"], "and is saved to the library");
}

fn word_being_typed_is_flagged_only_once_finished() {
    let mut h = harness_with("typing", "Start");
    let end = h.read(|e| e.doc.end());
    let at = h.word_at(0, end.offset);
    h.click(at);
    h.type_text(" teh");
    h.wait_for(|e| e.spelling_ready());
    assert!(h.misspelled().is_empty(), "no red line while the word is still being typed");
    h.cx.advance_clock(Duration::from_secs(1));
    h.frame();
    assert!(!h.read(|e| e.has_unsaved_changes()), "the idle debounce should save the draft");
    assert!(h.misspelled().is_empty(), "autosaving must not flag an unfinished word");
    h.type_text(" ");
    h.wait_for(|e| e.spelling_ready());
    assert_eq!(h.misspelled(), ["teh"], "flagged as soon as the word is finished");
    h.keys("cmd-alt-l");
    assert!(h.misspelled().is_empty(), "⌥⌘L turns spellcheck off");
}

fn unchanged_frames_reuse_layout_and_edits_only_shape_changed_paragraphs() {
    let text = std::iter::repeat_n("The lamp burned through the night.", 200).collect::<Vec<_>>().join("\n");
    let mut h = harness_with("layout-cache", &text);
    h.keys("cmd-alt-l cmd-up");
    let before = h.read(|e| e.shaped_blocks());
    for _ in 0..5 {
        h.frame();
    }
    assert_eq!(h.read(|e| e.shaped_blocks()), before, "idle frames must reuse the full layout");
    h.type_text("A");
    assert_eq!(h.read(|e| e.shaped_blocks()), before + 1, "only the edited paragraph is reshaped");
    assert_eq!(h.read(|e| e.word_count()), h.read(|e| e.doc.word_count()));
    h.keys("enter");
    assert_eq!(h.read(|e| e.word_count()), h.read(|e| e.doc.word_count()));
    h.keys("cmd-a backspace");
    assert_eq!(h.read(|e| e.word_count()), 0);
    h.keys("cmd-z");
    assert_eq!(h.read(|e| e.word_count()), h.read(|e| e.doc.word_count()));
}

fn caret_stays_visible_in_long_documents_and_focus_mode() {
    let text = std::iter::repeat_n("The sea was quiet.", 100).collect::<Vec<_>>().join("\n");
    let mut h = harness_with("caret-scroll", &text);
    h.keys("cmd-down");
    h.frame();
    let at = h.read(|e| e.point_for(e.head).unwrap());
    assert!(at.y > px(100.) && at.y < px(850.), "end caret must be visible: {at:?}");
    h.keys("cmd-up");
    h.frame();
    let at = h.read(|e| e.point_for(e.head).unwrap());
    assert!(at.y > px(100.) && at.y < px(850.), "start caret must be visible: {at:?}");
    h.keys("cmd-shift-f");
    h.settle();
    h.keys("cmd-down");
    h.frame();
    let at = h.read(|e| e.point_for(e.head).unwrap());
    assert!(at.y > px(40.) && at.y < px(830.), "focus caret must be visible: {at:?}");
    h.shot("focus-end");
}

fn failed_save_remains_dirty_and_can_be_retried() {
    let mut h = harness_with("save-failure", "Keep this");
    let healthy = std::mem::replace(&mut *h.library.borrow_mut(), Library::open_in_memory().unwrap());
    h.type_text(" writing");
    h.update(|e, _, _| {
        assert!(!e.commit());
    });
    assert!(h.read(|e| e.has_unsaved_changes() && e.save_error().is_some()));
    h.keys("cmd-z");
    assert_eq!(h.read(|e| e.doc.plain_text()), "Keep this writing", "undo must not discard a failed save");
    *h.library.borrow_mut() = healthy;
    h.update(|e, _, _| {
        assert!(e.commit());
    });
    assert!(!h.read(|e| e.has_unsaved_changes()));
    assert_eq!(h.library.borrow_mut().open_document(h.id).unwrap().plain_text(), "Keep this writing");
}

fn teardown_flushes_pending_typing() {
    let mut h = harness_with("teardown-save", "Keep this");
    h.type_text(" writing");
    assert!(h.read(|e| e.has_unsaved_changes()));
    let library = h.library.clone();
    let id = h.id;
    drop(h);
    assert_eq!(library.borrow_mut().open_document(id).unwrap().plain_text(), "Keep this writing");
}

fn spellcheck_discards_old_results_and_dismisses_stale_menus() {
    let mut h = harness_with("spelling-stale", "The kelpe.");
    h.keys("cmd-a");
    h.type_text("teh ");
    h.keys("cmd-a");
    h.type_text("the ");
    h.wait_for(|e| e.spelling_ready());
    assert!(h.misspelled().is_empty(), "a previous spelling job must not underline newer text");
    h.keys("cmd-a");
    h.type_text("kelpe ");
    h.wait_for(|e| e.spelling_ready());
    let at = h.word_at(0, 2);
    h.right_click(at);
    assert!(h.read(|e| e.spell_menu()).is_some());
    h.type_text("kelp");
    assert!(h.read(|e| e.spell_menu()).is_none(), "editing must dismiss suggestions with stale ranges");
}

fn cached_text_offsets_preserve_unicode_and_multiline_ime_input() {
    let mut h = harness_with("unicode-input", "a😀b\ncafé");
    h.update(|e, window, cx| {
        e.replace_text_in_range(Some(1..3), "🌊", window, cx);
        assert_eq!(e.selected_text_range(false, window, cx).unwrap().range, 3..3);
        e.replace_text_in_range(Some(5..9), "naïve", window, cx);
        assert_eq!(e.selected_text_range(false, window, cx).unwrap().range, 10..10);
        e.replace_and_mark_text_in_range(Some(5..10), "にほん", Some(0..3), window, cx);
        assert_eq!(e.marked_text_range(window, cx), Some(5..8));
        e.replace_text_in_range(None, "日本", window, cx);
        assert_eq!(e.doc.plain_text(), "a🌊b\n日本");
        assert_eq!(e.selected_text_range(false, window, cx).unwrap().range, 7..7);
        assert_eq!(e.word_count(), e.doc.word_count());
    });
    h.keys("cmd-z");
    assert_eq!(h.read(|e| e.doc.plain_text()), "a😀b\ncafé");
}

fn canvas_shapes_edit_resize_and_keep_formatting() {
    let mut h = harness("shape-editing");
    h.board_mode();
    h.tool(Tool::Shape);
    let (a, b) = (h.at(450., 180.), h.at(680., 340.));
    h.drag(a, b);
    let before = h.items()[0].clone();
    let at = h.at(before.x + before.w / 2., before.y + before.h / 2.);
    h.click(at);
    assert_eq!(h.read(|e| e.editing_item()), Some(before.id));
    h.type_text("Centered");
    h.keys("enter");
    h.type_text("Two lines");
    h.click(point(px(296.), px(74.))); // Bold in the formatting toolbar.
    h.type_text("!"); // Clicking a formatting control must retain text focus.
    let item = h.stored()[0].clone();
    let (text, style) = item.kind.text().unwrap();
    assert_eq!(text, "Centered\nTwo lines!");
    assert_eq!(style.typography.alignment, quill_doc::Alignment::Center);
    assert!(style.marks.contains(quill_doc::Marks::BOLD));
    h.shot("centered-editing");
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-shots/shape-editing-1-centered-editing.png");
    let png = image::open(path).unwrap().to_rgb8();
    let scale = png.width() as f32 / 1440.;
    let a = h.at(item.x + 8., item.y + 8.);
    let b = h.at(item.x + item.w - 8., item.y + item.h - 8.);
    let mut left = png.width();
    let mut right = 0;
    for y in (f32::from(a.y) * scale) as u32..(f32::from(b.y) * scale) as u32 {
        for x in (f32::from(a.x) * scale) as u32..(f32::from(b.x) * scale) as u32 {
            let pixel = png.get_pixel(x, y).0;
            if pixel.iter().all(|v| *v < 100) {
                left = left.min(x);
                right = right.max(x);
            }
        }
    }
    assert!(left < right, "shape text is visible while editing");
    let expected = f32::from(h.at(item.x + item.w / 2., item.y).x) * scale;
    assert!(((left + right) as f32 / 2. - expected).abs() < 4. * scale, "text must be visually centered while editing");
    // Right and bottom edges resize; left edge preserves the opposite edge.
    let right = h.at(item.x + item.w, item.y + item.h / 2.);
    h.drag(right, right + point(px(60.), px(0.)));
    let wider = h.stored()[0].clone();
    assert!(wider.w > item.w + 60.);
    let bottom = h.at(wider.x + wider.w / 2., wider.y + wider.h);
    h.drag(bottom, bottom + point(px(0.), px(50.)));
    let taller = h.stored()[0].clone();
    assert!(taller.h > wider.h + 50.);
    let left = h.at(taller.x, taller.y + taller.h / 2.);
    h.drag(left, left + point(px(30.), px(0.)));
    let resized = h.stored()[0].clone();
    assert!((resized.x + resized.w - taller.x - taller.w).abs() < 2.);
    assert_eq!(resized.kind.text().unwrap().0, "Centered\nTwo lines!");
    let at = h.at(resized.x + resized.w / 2., resized.y + resized.h / 2.);
    h.click(at);
    h.shot("resized-centered");
    let page = h.at(0., 100.);
    h.click(page);
    h.settle();
    let shape = h.at(resized.x + resized.w / 2., resized.y + resized.h / 2.);
    h.click(shape);
    assert_eq!(h.read(|e| e.editing_item()), Some(resized.id), "a shape click from Page mode also starts typing");
}

fn canvas_text_and_page_share_format_controls() {
    let mut h = harness("canvas-formatting");
    h.board_mode();
    let at = h.at(-600., 200.);
    h.double_click(at);
    h.type_text("Freeform text");
    h.click(point(px(500.), px(74.))); // Font menu.
    let at = h.read(|e| e.formatting_menu_position()).expect("font menu opened");
    h.click(at + point(px(35.), px(6. + 28. + 14.)));
    h.type_text("!");
    let item = h.stored()[0].clone();
    assert_eq!(item.kind.text().unwrap().0, "Freeform text!");
    assert_eq!(item.kind.text().unwrap().1.typography.typeface, quill_doc::Typeface::Sans);
    h.click(point(px(540.), px(74.)));
    let at = h.read(|e| e.formatting_menu_position()).expect("size menu opened");
    h.click(at + point(px(35.), px(6. + 28. * 5. + 14.)));
    assert_eq!(h.stored()[0].kind.text().unwrap().1.typography.size, Some(24));
    h.click(point(px(586.), px(74.)));
    let at = h.read(|e| e.formatting_menu_position()).expect("color menu opened");
    h.click(at + point(px(35.), px(6. + 28. * 7. + 14.)));
    assert_eq!(h.stored()[0].kind.text().unwrap().1.typography.color, Some(0xf472b6));
    h.click(point(px(364.), px(74.))); // Underline.
    h.click(point(px(398.), px(74.))); // Strikethrough.
    h.click(point(px(432.), px(74.))); // Highlight.
    h.shot("freeform-editing-effects");
    let away = h.at(620., 900.);
    h.click(away);
    let style = h.stored()[0].kind.text().unwrap().1.clone();
    assert!(style.marks.contains(quill_doc::Marks::UNDERLINE));
    assert!(style.marks.contains(quill_doc::Marks::STRIKE));
    assert!(style.marks.contains(quill_doc::Marks::HIGHLIGHT));
    let at = h.at(0., 100.);
    h.click(at);
    h.settle();
    h.click(point(px(500.), px(74.)));
    let at = h.read(|e| e.formatting_menu_position()).unwrap();
    h.click(at + point(px(35.), px(6. + 28. * 2. + 14.)));
    assert_eq!(h.read(|e| e.doc.blocks[0].style.typeface), quill_doc::Typeface::Mono);
    assert_eq!(h.stored()[0].kind.text().unwrap().1, &style);
    h.shot("page-keeps-board-visible");
}

fn document_opens_focused_and_flows_across_pages() {
    let text =
        (0..65).map(|i| format!("Paragraph {i} keeps flowing onto the next page.")).collect::<Vec<_>>().join("\n");
    let mut h = harness_with("pagination", &text);
    h.update(|e, window, cx| e.focus_document(window, cx));
    h.settle();
    assert_eq!(h.read(|e| e.mode), Mode::Page);
    assert_eq!(h.read(|e| e.head), Pos::default());
    assert!(h.read(|e| e.page_count()) >= 3);
    h.type_text("Ready. ");
    assert!(h.read(|e| e.doc.plain_text()).starts_with("Ready. Paragraph 0"));
    let origin = h.read(|e| e.point_for(Pos::default()).unwrap());
    let mut previous = origin.y;
    let mut gap_found = false;
    for i in 1..65 {
        let at = h.read(|e| e.point_for(Pos::new(i, 0)).unwrap());
        if at.y - previous > px(100.) {
            gap_found = true;
        }
        previous = at.y;
    }
    assert!(gap_found, "paragraphs cross a real page margin and gap");
    h.keys("cmd-down");
    h.settle();
    let caret = h.read(|e| e.point_for(e.head).unwrap());
    let viewport = h.read(|e| e.canvas_bounds());
    assert!(caret.y >= viewport.top() && caret.y < viewport.bottom());
    h.shot("last-page");
}

fn long_paragraph_paginates_and_centered_caret_hit_tests() {
    let text = "A long paragraph flows from one sheet to the next. ".repeat(180);
    let mut h = harness_with("wrapped-pages", &text);
    assert!(h.read(|e| e.page_count()) > 2);
    h.keys("cmd-up");
    h.click(point(px(668.), px(74.)));
    assert_eq!(h.read(|e| e.doc.blocks[0].style.alignment), quill_doc::Alignment::Center);
    let pos = Pos::new(0, 5);
    let at = h.read(|e| e.point_for(pos).unwrap()) + point(px(0.), px(8.));
    h.click(at);
    assert_eq!(h.read(|e| e.head), pos, "clicking centered text must position its caret correctly");
    h.shot("wrapped-first-page");
    h.keys("cmd-up");
    let mut previous = h.read(|e| e.head);
    for _ in 0..75 {
        h.keys("down");
        let next = h.read(|e| e.head);
        assert!(next > previous, "Down must cross page gaps instead of getting stuck");
        previous = next;
    }
    for _ in 0..75 {
        h.keys("up");
        let next = h.read(|e| e.head);
        assert!(next < previous, "Up must cross page gaps instead of getting stuck");
        previous = next;
    }

    h.keys("cmd-down");
    h.settle();
    let caret = h.read(|e| e.point_for(e.head).unwrap());
    assert!(caret.y < h.read(|e| e.canvas_bounds().bottom()));
    h.type_text("End.");
    assert!(h.read(|e| e.doc.plain_text()).ends_with("End."));
    h.shot("wrapped-last-page");
}

fn canvas_unicode_ime_and_text_undo_preserve_content() {
    let mut h = harness("canvas-ime");
    h.board_mode();
    let at = h.at(-600., 250.);
    h.double_click(at);
    h.type_text("A🌊B");
    h.update(|e, window, cx| {
        e.editing_text_input().unwrap().update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(Some(1..3), "にほん", Some(0..3), window, cx);
            assert_eq!(input.marked_text_range(window, cx), Some(1..4));
            input.replace_text_in_range(None, "日本", window, cx);
        });
    });
    assert_eq!(text_of(&h.stored()[0]), "A日本B");
    h.keys("cmd-down");
    h.keys("enter");
    h.type_text("Second line");
    h.keys("cmd-a");
    h.keys("cmd-c");
    h.type_text("Replacement");
    h.keys("cmd-z");
    assert_eq!(text_of(&h.stored()[0]), "A日本B\nSecond line");
    h.keys("cmd-a");
    h.keys("cmd-v");
    assert_eq!(text_of(&h.stored()[0]), "A日本B\nSecond line");
}

fn opening_wide_board_centers_page_with_music_outside_canvas() {
    let playback = quill::media::NowPlaying {
        player: quill::media::Player::Spotify,
        title: "A quiet track".into(),
        artist: "Test artist".into(),
        playing: false,
        progress: 0.5,
        artwork_url: None,
    };
    let mut h = harness_seeded(
        "opening-centered-music",
        "Start writing here.",
        |library, id| {
            library
                .add_item(
                    id,
                    -1800.,
                    200.,
                    300.,
                    100.,
                    ItemKind::Text { text: "Left-side ideas".into(), style: Default::default() },
                )
                .unwrap();
            library
                .add_item(
                    id,
                    1300.,
                    180.,
                    260.,
                    160.,
                    ItemKind::Shape {
                        ellipse: false,
                        color: 0xfde68a,
                        text: "Right-side ideas".into(),
                        style: quill_store::BoardTextStyle::shape(),
                    },
                )
                .unwrap();
        },
        Some(playback),
        true,
    );
    let viewport = h.read(|e| e.canvas_bounds());
    assert!(
        (h.at(0., 0.).x - viewport.center().x).abs() < px(1.),
        "a loaded wide board must center on its very first layout"
    );
    assert!((h.at(0., 0.).y - viewport.top() - px(quill::editor::PAGE_TOP)).abs() < px(1.));
    assert!(viewport.bottom() < px(840.), "music occupies its own layout space below the canvas");
    assert_eq!(h.read(|e| e.mode), Mode::Page);
    assert_eq!(h.read(|e| e.zoom_level()), 1.);
    h.type_text("Ready. ");
    assert_eq!(h.read(|e| e.doc.plain_text()), "Ready. Start writing here.");
    h.shot("page-and-docked-player");
    h.keys("cmd-shift-b");
    h.settle();
    assert!(h.read(|e| e.canvas_bounds().bottom()) < px(840.));
    h.shot("board-and-docked-player");
}

fn main() {
    if std::env::args().any(|arg| arg == "--bench-render") {
        let text = (0..500)
            .map(|i| format!("Paragraph {i}. The lamp had burned every night for forty years. The sea was quiet."))
            .collect::<Vec<_>>()
            .join("\n");
        let mut h = harness_with("benchmark", &text);
        let start = std::time::Instant::now();
        for _ in 0..30 {
            h.frame();
        }
        println!("500 paragraphs, idle frame: {:.2} ms", start.elapsed().as_secs_f64() * 1000. / 30.);
        h.keys("cmd-up");
        let start = std::time::Instant::now();
        for _ in 0..20 {
            h.type_text("a");
        }
        println!("500 paragraphs, typed character + frame: {:.2} ms", start.elapsed().as_secs_f64() * 1000. / 20.);
        return;
    }
    let tests: &[(&str, fn())] = &[
        (
            "opening wide board centers page with music outside canvas",
            opening_wide_board_centers_page_with_music_outside_canvas,
        ),
        ("new canvas Unicode IME and text undo preserve content", canvas_unicode_ime_and_text_undo_preserve_content),
        ("new canvas shapes edit resize and keep formatting", canvas_shapes_edit_resize_and_keep_formatting),
        ("new canvas text and page share format controls", canvas_text_and_page_share_format_controls),
        ("new document opens focused and flows across pages", document_opens_focused_and_flows_across_pages),
        (
            "new long paragraph paginates and centered caret hit tests",
            long_paragraph_paginates_and_centered_caret_hit_tests,
        ),
        ("double-click blank board from Page mode starts text", double_click_blank_board_from_page_mode_starts_text),
        (
            "clicking page restores actual size and centers after panning",
            clicking_page_restores_actual_size_and_centers_after_panning,
        ),
        ("board toolbar clicks do not reach the page", board_toolbar_clicks_do_not_reach_the_page),
        (
            "cached text offsets preserve Unicode and multiline IME input",
            cached_text_offsets_preserve_unicode_and_multiline_ime_input,
        ),
        (
            "unchanged frames reuse layout and edits shape only changed paragraphs",
            unchanged_frames_reuse_layout_and_edits_only_shape_changed_paragraphs,
        ),
        ("caret stays visible in long documents and focus mode", caret_stays_visible_in_long_documents_and_focus_mode),
        ("failed save remains dirty and can be retried", failed_save_remains_dirty_and_can_be_retried),
        ("teardown flushes pending typing", teardown_flushes_pending_typing),
        (
            "spellcheck discards old results and dismisses stale menus",
            spellcheck_discards_old_results_and_dismisses_stale_menus,
        ),
        ("double-click board creates editable text", double_click_board_creates_editable_text),
        ("double-click edits an existing note", double_click_edits_an_existing_note),
        ("text box is created and kept", text_box_is_created_and_kept),
        ("empty note disappears when left", empty_note_disappears_when_left),
        ("pen and highlighter draw strokes", pen_and_highlighter_draw_strokes),
        ("shape tool drags out a rectangle", shape_tool_drags_out_a_rectangle),
        ("select drags a note; delete and undo work", select_drags_a_note_and_delete_and_undo_work),
        ("eraser removes what it touches", eraser_removes_what_it_touches),
        ("image tool places an image", image_tool_places_an_image),
        ("clicking page and board switches modes", clicking_page_and_board_switches_modes),
        ("pinch and keyboard zoom", pinch_and_keyboard_zoom),
        (
            "misspelled word is underlined and corrected from the menu",
            misspelled_word_is_underlined_and_corrected_from_the_menu,
        ),
        ("learn spelling is remembered", learn_spelling_is_remembered),
        ("word being typed is flagged only once finished", word_being_typed_is_flagged_only_once_finished),
    ];
    let filter = std::env::args().nth(1);
    let tests: Vec<_> = tests.iter().filter(|(name, _)| filter.as_ref().is_none_or(|f| name.contains(f))).collect();
    assert!(!tests.is_empty(), "no tests match the filter");
    let mut failed = Vec::new();
    for (name, test) in &tests {
        match catch_unwind(AssertUnwindSafe(test)) {
            Ok(()) => println!("ok    {name}"),
            Err(_) => {
                println!("FAIL  {name}");
                failed.push(*name);
            }
        }
    }
    println!("\n{} passed, {} failed", tests.len() - failed.len(), failed.len());
    if !failed.is_empty() {
        std::process::exit(1);
    }
}
