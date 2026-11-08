//! Quill, a local-first writing app. `main.rs` only calls [`run`]; the
//! modules are public so integration tests can drive real views.

pub mod anim;
pub mod assets;
pub mod board;
pub mod demo;
pub mod editor;
pub mod home;
pub mod import;
pub mod media;
pub mod page;
pub mod storage;
pub mod theme;
#[cfg(feature = "screenshots")]
mod tour;

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use quill_doc::{Block, BlockKind, Document};
use quill_store::{DocId, Library};

use editor::{EditorEvent, EditorView};
use home::{HomeEvent, HomeView};
use import::{ImportEvent, ImportView};
use media::Media;
use storage::{BACKUP_EVERY, Locations};

actions!(quill, [Quit]);

enum Screen {
    Home(Entity<HomeView>),
    Import(Entity<ImportView>),
    Editor(Entity<EditorView>),
}

struct QuillApp {
    library: Rc<RefCell<Library>>,
    /// Bumped on every screen change so the new screen animates in.
    screen_gen: usize,
    media: Entity<Media>,
    screen: Screen,
    _subscription: Subscription,
}

impl QuillApp {
    fn new(library: Rc<RefCell<Library>>, notice: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let media = cx.new(Media::new);
        let (screen, subscription) = Self::home(library.clone(), notice, window, cx);
        QuillApp { library, screen_gen: 0, media, screen, _subscription: subscription }
    }

    fn home(
        library: Rc<RefCell<Library>>,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Screen, Subscription) {
        let home = cx.new(|cx| HomeView::new(library, notice, window, cx));
        let subscription = cx.subscribe_in(&home, window, |this, _, event, window, cx| match event {
            HomeEvent::Open(id) => this.open(*id, window, cx),
            HomeEvent::Import => this.show_import(window, cx),
        });
        (Screen::Home(home), subscription)
    }

    fn show_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (screen, subscription) = Self::home(self.library.clone(), None, window, cx);
        self.screen = screen;
        self.screen_gen += 1;
        self._subscription = subscription;
        cx.notify();
    }

    fn show_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let import = cx.new(|_| ImportView::new(self.library.clone()));
        self._subscription = cx.subscribe_in(&import, window, |this, _, event, window, cx| match event {
            ImportEvent::Done => this.show_home(window, cx),
        });
        self.screen = Screen::Import(import);
        self.screen_gen += 1;
        cx.notify();
    }

    #[cfg_attr(not(feature = "screenshots"), allow(dead_code))]
    fn open_titled(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .library
            .borrow()
            .list(None)
            .ok()
            .and_then(|docs| docs.into_iter().find(|d| d.title == title))
            .map(|d| d.id);
        if let Some(id) = id {
            self.open(id, window, cx);
        }
    }

    #[cfg_attr(not(feature = "screenshots"), allow(dead_code))]
    fn with_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditorView, &mut Window, &mut Context<EditorView>),
    ) {
        if let Screen::Editor(editor) = &self.screen {
            editor.update(cx, |e, cx| f(e, window, cx));
        }
    }

    fn open(&mut self, id: DocId, window: &mut Window, cx: &mut Context<Self>) {
        let doc = match self.library.borrow_mut().open_document(id) {
            Ok(doc) => doc,
            Err(err) => {
                eprintln!("quill: opening document {id} failed: {err}");
                return;
            }
        };
        let (library, media) = (self.library.clone(), self.media.clone());
        let editor = cx.new(|cx| EditorView::new(library, id, doc, media, cx));
        window.focus(&editor.focus_handle(cx), cx);
        self._subscription = cx.subscribe_in(&editor, window, |this, _, event, window, cx| match event {
            EditorEvent::Close => this.show_home(window, cx),
        });
        self.screen = Screen::Editor(editor);
        self.screen_gen += 1;
        cx.notify();
    }
}

impl Render for QuillApp {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let view = match &self.screen {
            Screen::Home(home) => home.clone().into_any_element(),
            Screen::Import(import) => import.clone().into_any_element(),
            Screen::Editor(editor) => editor.clone().into_any_element(),
        };
        // Each screen settles in: a short fade with a slight rise.
        div().size_full().bg(theme::app_bg()).child(div().size_full().child(view).with_animation(
            ("screen", self.screen_gen),
            Animation::new(anim::STANDARD).with_easing(anim::ease_out),
            |el, t| el.opacity(t).mt(px((1. - t) * 10.)),
        ))
    }
}

fn welcome(library: &mut Library) {
    let mut doc = Document::new("Welcome to Quill");
    doc.blocks = vec![
        Block::new(BlockKind::Heading(1), "Welcome to Quill"),
        Block::new(
            BlockKind::Paragraph,
            "Everything you write stays on this computer. Every change is kept forever, so undo can always take you back.",
        ),
        Block::new(BlockKind::Heading(2), "Try it"),
        Block::new(BlockKind::Bullet, "Select some text and press ⌘B, ⌘I or ⌘U."),
        Block::new(BlockKind::Bullet, "Press ⌘Z as many times as you like, then ⌘⇧Z to come back."),
        Block::new(BlockKind::Bullet, "Click outside the page to use the board: sticky notes, pens and shapes."),
        Block::new(BlockKind::Bullet, "Press ⌘⇧F for focus mode, and Esc to return to your library."),
    ];
    if let Err(err) = library.create(&doc, None) {
        eprintln!("quill: creating the welcome document failed: {err}");
    }
}

/// Starts Quill: opens (or recovers) the library and shows the main window.
pub fn run() {
    let locations = Rc::new(Locations::resolve());
    let (mut library, recovery) = match locations.open() {
        Ok(opened) => opened,
        Err(err) => {
            eprintln!("quill: could not open the library at {}: {err}", locations.library.display());
            std::process::exit(1);
        }
    };
    if library.list(None).map(|d| d.is_empty()).unwrap_or(false) {
        if std::env::var_os("QUILL_DEMO").is_some() {
            if let Err(err) = demo::seed(&mut library) {
                eprintln!("quill: seeding demo content failed: {err}");
            }
        } else {
            welcome(&mut library);
        }
    }
    locations.back_up(&library);
    let library = Rc::new(RefCell::new(library));
    let notice = storage::describe(&recovery);

    gpui_kit::application().with_assets(assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        editor::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        cx.spawn({
            let (library, locations) = (library.clone(), locations.clone());
            async move |cx| loop {
                cx.background_executor().timer(BACKUP_EVERY).await;
                locations.back_up(&library.borrow());
            }
        })
        .detach();
        cx.on_app_quit({
            let (library, locations) = (library.clone(), locations.clone());
            move |_| {
                locations.back_up(&library.borrow());
                async {}
            }
        })
        .detach();

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1440.), px(900.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("Quill".into()), ..Default::default() }),
            ..Default::default()
        };
        let window = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| QuillApp::new(library, notice, window, cx))
        })
        .expect("failed to open window");
        #[cfg(feature = "screenshots")]
        if let Some(dir) = std::env::var_os("QUILL_SCREENSHOTS") {
            tour::run(window.0, window.1, dir.into(), cx);
        }
        #[cfg(not(feature = "screenshots"))]
        let _ = window;
        cx.activate(true);
    });
}
