//! The import screen: pick or drop files, folders or a Takeout zip; each
//! supported file becomes a library document. Comments in the source become
//! sticky notes beside the page.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use quill_import::{Item, Options};
use quill_store::{ItemKind, Library};

use crate::board::NOTE_COLORS;
use crate::home::button;
use crate::theme;

pub enum ImportEvent {
    Done,
}

enum Status {
    Imported { notes: usize },
    Failed(String),
}

struct Row {
    source: String,
    status: Status,
}

pub struct ImportView {
    library: Rc<RefCell<Library>>,
    options: Options,
    rows: Vec<Row>,
    running: bool,
}

impl EventEmitter<ImportEvent> for ImportView {}

impl ImportView {
    pub fn new(library: Rc<RefCell<Library>>) -> ImportView {
        ImportView { library, options: Options::default(), rows: Vec::new(), running: false }
    }

    fn choose(&mut self, folders: bool, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| this.run(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn run(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.running || paths.is_empty() {
            return;
        }
        self.running = true;
        cx.notify();
        let options = self.options;
        let task = cx.background_executor().spawn(async move { quill_import::import_paths(&paths, options) });
        cx.spawn(async move |this, cx| {
            let items = task.await;
            this.update(cx, |this, cx| {
                for item in items {
                    let row = this.store(item);
                    this.rows.push(row);
                }
                this.running = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn store(&mut self, item: Item) -> Row {
        let source = item.source;
        let imported = match item.result {
            Ok(imported) => imported,
            Err(err) => return Row { source, status: Status::Failed(err.to_string()) },
        };
        let mut library = self.library.borrow_mut();
        let id = match library.create(&imported.doc, item.collection.as_deref()) {
            Ok(id) => id,
            Err(err) => return Row { source, status: Status::Failed(err.to_string()) },
        };
        // Comments line up down the left side of the page.
        let x = -(crate::page::TEXT_WIDTH / 2. + 80. + 260.);
        for (i, note) in imported.notes.iter().enumerate() {
            let kind = ItemKind::Sticky { text: note.clone(), color: NOTE_COLORS[i % NOTE_COLORS.len()] };
            if let Err(err) = library.add_item(id, x, 40. + i as f32 * 170., 200., 150., kind) {
                eprintln!("quill: adding a comment note failed: {err}");
            }
        }
        Row { source, status: Status::Imported { notes: imported.notes.len() } }
    }

    fn option(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        cx: &mut Context<Self>,
        flip: fn(&mut Options),
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .child(
                div()
                    .size(px(18.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if on { theme::accent() } else { rgb(0x353a45) })
                    .when(on, |d| d.bg(theme::accent()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::on_accent())
                    .text_size(px(12.))
                    .child(if on { "✓" } else { "" }),
            )
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(&mut this.options);
                cx.notify();
            }))
    }
}

impl Render for ImportView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let done = self.rows.iter().filter(|r| matches!(r.status, Status::Imported { .. })).count();
        div()
            .id("import")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(theme::app_bg())
            .text_color(theme::text())
            .font_family(theme::UI_FONT)
            .text_size(px(14.))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.run(paths.paths().to_vec(), cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(24.))
                    .py(px(12.))
                    .bg(theme::surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        button("back", "‹ Library", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ImportEvent::Done))),
                    )
                    .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child("Import documents")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(24.))
                    .p(px(32.))
                    .child(
                        div()
                            .flex_grow(1.)
                            .flex_basis(px(560.))
                            .flex()
                            .flex_col()
                            .gap(px(20.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(14.))
                                    .py(px(40.))
                                    .px(px(24.))
                                    .rounded(px(14.))
                                    .border_2()
                                    .border_dashed()
                                    .border_color(theme::accent())
                                    .bg(theme::surface())
                                    .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(
                                        if self.running { "Importing…" } else { "Drop files or folders here" },
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(10.))
                                            .child(
                                                button("files", "Choose files…", true)
                                                    .on_click(cx.listener(|this, _, _, cx| this.choose(false, cx))),
                                            )
                                            .child(
                                                button("folder", "Choose a folder…", false)
                                                    .on_click(cx.listener(|this, _, _, cx| this.choose(true, cx))),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(theme::muted())
                                            .child(".docx  .odt  .md  .txt  .rtf  .html  .zip"),
                                    ),
                            )
                            .when(!self.rows.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .rounded(px(14.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .bg(theme::surface())
                                        .child(
                                            div()
                                                .px(px(20.))
                                                .py(px(14.))
                                                .border_b_1()
                                                .border_color(theme::border())
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(format!("{done} of {} imported", self.rows.len())),
                                        )
                                        .children(self.rows.iter().map(|row| {
                                            let (status, color) = match &row.status {
                                                Status::Imported { notes: 0 } => {
                                                    ("Imported".to_string(), rgb(0x4ade80))
                                                }
                                                Status::Imported { notes } => (
                                                    format!(
                                                        "Imported, {notes} note{}",
                                                        if *notes == 1 { "" } else { "s" }
                                                    ),
                                                    rgb(0x4ade80),
                                                ),
                                                Status::Failed(err) => (err.clone(), rgb(0xfbbf24)),
                                            };
                                            div()
                                                .flex()
                                                .gap(px(14.))
                                                .px(px(20.))
                                                .py(px(10.))
                                                .border_b_1()
                                                .border_color(theme::raised())
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .text_ellipsis()
                                                        .overflow_hidden()
                                                        .child(row.source.clone()),
                                                )
                                                .child(div().text_size(px(13.)).text_color(color).child(status))
                                        }))
                                        .child(
                                            div().flex().justify_end().p(px(14.)).child(
                                                button("open-library", "Open library", true)
                                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ImportEvent::Done))),
                                            ),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_grow(1.)
                            .flex_basis(px(280.))
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .p(px(20.))
                            .rounded(px(14.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Options"))
                            .child(self.option(
                                "opt-folders",
                                "Folders become collections",
                                self.options.folders_as_collections,
                                cx,
                                |o| o.folders_as_collections = !o.folders_as_collections,
                            ))
                            .child(self.option(
                                "opt-notes",
                                "Comments become sticky notes",
                                self.options.comments_as_notes,
                                cx,
                                |o| o.comments_as_notes = !o.comments_as_notes,
                            )),
                    ),
            )
    }
}
