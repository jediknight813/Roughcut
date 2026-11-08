mod board;
mod editor;
mod home;
mod import;
mod media;
mod page;
mod storage;
mod theme;

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
    media: Entity<Media>,
    screen: Screen,
    _subscription: Subscription,
}

impl QuillApp {
    fn new(library: Rc<RefCell<Library>>, notice: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let media = cx.new(Media::new);
        let (screen, subscription) = Self::home(library.clone(), notice, window, cx);
        QuillApp { library, media, screen, _subscription: subscription }
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
        self._subscription = subscription;
        cx.notify();
    }

    fn show_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let import = cx.new(|_| ImportView::new(self.library.clone()));
        self._subscription = cx.subscribe_in(&import, window, |this, _, event, window, cx| match event {
            ImportEvent::Done => this.show_home(window, cx),
        });
        self.screen = Screen::Import(import);
        cx.notify();
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
        cx.notify();
    }
}

impl Render for QuillApp {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let screen = div().size_full();
        match &self.screen {
            Screen::Home(home) => screen.child(home.clone()),
            Screen::Import(import) => screen.child(import.clone()),
            Screen::Editor(editor) => screen.child(editor.clone()),
        }
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

fn main() {
    let locations = Rc::new(Locations::resolve());
    let (mut library, recovery) = match locations.open() {
        Ok(opened) => opened,
        Err(err) => {
            eprintln!("quill: could not open the library at {}: {err}", locations.library.display());
            std::process::exit(1);
        }
    };
    if library.list(None).map(|d| d.is_empty()).unwrap_or(false) {
        welcome(&mut library);
    }
    locations.back_up(&library);
    let library = Rc::new(RefCell::new(library));
    let notice = storage::describe(&recovery);

    gpui_kit::application().run(move |cx| {
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
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(860.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("Quill".into()), ..Default::default() }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| QuillApp::new(library, notice, window, cx)))
            .expect("failed to open window");
        cx.activate(true);
    });
}
