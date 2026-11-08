//! Screenshot tour for design review: walks through every screen and saves a
//! PNG of each, rendered offscreen (no Screen Recording permission needed).
//!
//! ```sh
//! QUILL_HOME=/tmp/q QUILL_DEMO=1 QUILL_SCREENSHOTS=/tmp/shots cargo run -p quill --features screenshots
//! ```

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::*;

use crate::editor::Mode;
use crate::{QuillApp, Screen};

type Step = (&'static str, fn(&mut QuillApp, &mut Window, &mut Context<QuillApp>));

/// Long enough for entrance animations to settle.
const SETTLE: Duration = Duration::from_millis(1100);

const STEPS: &[Step] = &[
    ("01-home", |_, _, _| {}),
    ("02-editor-page", |app, window, cx| app.open_titled("The Lighthouse Keeper", window, cx)),
    ("03-editor-board", |app, window, cx| app.with_editor(window, cx, |e, w, cx| e.set_mode(Mode::Board, w, cx))),
    ("04-board-editing-note", |app, window, cx| {
        app.with_editor(window, cx, |e, w, cx| {
            if let Some(id) = e.board.first().map(|i| i.id) {
                e.edit_item(id, w, cx);
            }
        })
    }),
    ("05-history", |app, window, cx| {
        app.with_editor(window, cx, |e, w, cx| {
            e.stop_editing_item(cx);
            e.set_mode(Mode::Page, w, cx);
            e.toggle_history(cx);
        })
    }),
    ("06-focus", |app, window, cx| app.with_editor(window, cx, |e, w, cx| e.set_mode(Mode::Focus, w, cx))),
    ("07-import", |app, window, cx| app.show_import(window, cx)),
    ("08-home-search", |app, window, cx| {
        app.show_home(window, cx);
        if let Screen::Home(home) = &app.screen {
            home.update(cx, |h, cx| h.set_query("light", window, cx));
        }
    }),
];

pub fn run(window: AnyWindowHandle, app: Entity<QuillApp>, dir: PathBuf, cx: &mut App) {
    std::fs::create_dir_all(&dir).expect("screenshot directory");
    cx.spawn(async move |cx| {
        for (name, step) in STEPS {
            window.update(cx, |_, window, cx| app.update(cx, |app, cx| step(app, window, cx))).ok();
            cx.background_executor().timer(SETTLE).await;
            let shot = window.update(cx, |_, window, _| window.render_to_image());
            match shot {
                Ok(Ok(image)) => {
                    let path = dir.join(format!("{name}.png"));
                    if let Err(err) = image.save(&path) {
                        eprintln!("quill: saving {} failed: {err}", path.display());
                    }
                }
                Ok(Err(err)) | Err(err) => eprintln!("quill: capturing {name} failed: {err}"),
            }
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}
