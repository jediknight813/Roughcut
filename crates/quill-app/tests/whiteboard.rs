//! Drives the real editor in an offscreen window with real mouse, trackpad
//! and keyboard events, through every whiteboard tool, and saves screenshots
//! to `target/whiteboard/` for review.
//!
//! cargo test -p quill --features screenshots --test whiteboard

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
use quill::editor::{EditorView, Mode};
use quill::media::Media;
use quill_doc::Document;
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
    let doc = Document::from_plain_text("Test", "The lamp had burned every night for forty-one years.");
    let id = library.create(&doc, None).unwrap();
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
                    let media = cx.new(|_| Media::idle());
                    let editor = cx.new(|cx| EditorView::new(lib, id, doc, media, cx));
                    window.focus(&editor.focus_handle(cx), cx);
                    editor
                },
            )
        })
        .unwrap();
    let mut h = Harness { cx, window, editor, library, id, name, shots: 0 };
    h.settle();
    h
}

impl Harness {
    fn frame(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, w, cx| {
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
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/whiteboard");
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
        ItemKind::Sticky { text, .. } | ItemKind::Text { text } => text,
        _ => "",
    }
}

// --- The tests -------------------------------------------------------------

fn sticky_note_is_created_typed_into_and_kept() {
    let mut h = harness("sticky");
    h.board_mode();
    h.tool(Tool::Sticky);
    let spot = h.at(-620., 200.);
    h.click(spot);
    let items = h.items();
    assert_eq!(items.len(), 1, "clicking with the sticky tool adds a note");
    assert!(matches!(items[0].kind, ItemKind::Sticky { .. }));
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
    h.tool(Tool::Sticky);
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
    h.tool(Tool::Sticky);
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
    assert!(matches!(items[0].kind, ItemKind::Shape { .. }));
    assert!((items[0].w - 200.).abs() < 6. && (items[0].h - 130.).abs() < 6., "shape matches the drag: {:?}", items[0]);
}

fn select_drags_a_note_and_delete_and_undo_work() {
    let mut h = harness("select");
    h.board_mode();
    h.tool(Tool::Sticky);
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

fn main() {
    let tests: &[(&str, fn())] = &[
        ("sticky note is created, typed into and kept", sticky_note_is_created_typed_into_and_kept),
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
    ];
    let mut failed = Vec::new();
    for (name, test) in tests {
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
