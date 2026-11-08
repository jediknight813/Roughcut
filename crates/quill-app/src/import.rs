//! The import screen: pick or drop files, folders or a Takeout zip; each
//! supported file becomes a library document. Comments in the source become
//! board text beside the page.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;
use quill_import::{Item, Options};
use quill_store::{ItemKind, Library};

use crate::anim;
use crate::assets::icon;
use crate::editor::icon_button;
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
            let kind = ItemKind::Text { text: note.clone(), style: Default::default() };
            if let Err(err) = library.add_item(id, x, 40. + i as f32 * 170., 200., 150., kind) {
                eprintln!("roughcut: adding a comment note failed: {err}");
            }
        }
        Row { source, status: Status::Imported { notes: imported.notes.len() } }
    }

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        detail: &'static str,
        on: bool,
        cx: &mut Context<Self>,
        flip: fn(&mut Options),
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(14.))
            .py(px(12.))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(2.))
                    .child(label)
                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(detail)),
            )
            .child(
                // An Apple-style switch.
                div()
                    .w(px(38.))
                    .h(px(22.))
                    .p(px(2.))
                    .rounded_full()
                    .flex()
                    .when(on, |d| d.bg(theme::accent()).justify_end())
                    .when(!on, |d| d.bg(rgb(0x3a3f4b)))
                    .child(div().size(px(18.)).rounded_full().bg(rgb(0xffffff)).shadow_sm()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(&mut this.options);
                cx.notify();
            }))
    }
}

impl Render for ImportView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running;
        let done = self.rows.iter().filter(|r| matches!(r.status, Status::Imported { .. })).count();
        if self.running {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .text_color(theme::text())
            .font_family(theme::UI_FONT)
            .text_size(px(14.))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.run(paths.paths().to_vec(), cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .h(px(52.))
                    .bg(theme::surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        icon_button("back", "chevron-left", "Library", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ImportEvent::Done))),
                    )
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Import")),
            )
            .child(
                div().id("import-scroll").flex_1().overflow_y_scroll().child(
                    div()
                        .w_full()
                        .max_w(px(760.))
                        .mx_auto()
                        .px(px(28.))
                        .py(px(48.))
                        .flex()
                        .flex_col()
                        .gap(px(24.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).child("Bring in your writing"))
                                .child(
                                    div()
                                        .text_color(theme::muted())
                                        .child("Word, OpenDocument, Markdown, RTF, HTML and text files, whole folders, or ZIP archives."),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(16.))
                                .py(px(44.))
                                .rounded(px(18.))
                                .border_2()
                                .border_dashed()
                                .border_color(if self.running { theme::accent() } else { rgb(0x3a3f4b) })
                                .bg(theme::surface())
                                .child(
                                    div()
                                        .size(px(56.))
                                        .rounded(px(16.))
                                        .bg(theme::selected_bg())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon(if self.running { "clock" } else { "download" }).size(px(26.)).text_color(rgb(0x9db4f5)))
                                        .with_animation(
                                            ("pulse", running as usize),
                                            Animation::new(Duration::from_millis(1100)).repeat(),
                                            move |el, t| {
                                                let pulse = 0.55 + 0.45 * (t * std::f32::consts::PI).cos().abs();
                                                el.opacity(if running { pulse } else { 1. })
                                            },
                                        ),
                                )
                                .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(if self.running {
                                    "Importing…"
                                } else {
                                    "Drop files or folders here"
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(10.))
                                        .child(
                                            button("files", "file-plus", "Choose files…", true)
                                                .on_click(cx.listener(|this, _, _, cx| this.choose(false, cx))),
                                        )
                                        .child(
                                            button("folder", "folder-open", "Choose a folder…", false)
                                                .on_click(cx.listener(|this, _, _, cx| this.choose(true, cx))),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .px(px(18.))
                                .rounded(px(14.))
                                .bg(theme::surface())
                                .border_1()
                                .border_color(theme::border())
                                .child(self.toggle(
                                    "opt-folders",
                                    "Folders become collections",
                                    "Each folder's name becomes a filter on your library.",
                                    self.options.folders_as_collections,
                                    cx,
                                    |o| o.folders_as_collections = !o.folders_as_collections,
                                ))
                                .child(div().h(px(1.)).bg(theme::border()))
                                .child(self.toggle(
                                    "opt-notes",
                                    "Comments become board text",
                                    "Imported document comments appear beside the page.",
                                    self.options.comments_as_notes,
                                    cx,
                                    |o| o.comments_as_notes = !o.comments_as_notes,
                                )),
                        )
                        .when(!self.rows.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .rounded(px(14.))
                                    .bg(theme::surface())
                                    .border_1()
                                    .border_color(theme::border())
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .px(px(18.))
                                            .h(px(48.))
                                            .border_b_1()
                                            .border_color(theme::border())
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(format!("{done} of {} imported", self.rows.len())),
                                            )
                                            .child(
                                                button("open-library", "check", "Done", true)
                                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ImportEvent::Done))),
                                            ),
                                    )
                                    .children(self.rows.iter().enumerate().map(|(i, row)| {
                                        let (glyph, status, color) = match &row.status {
                                            Status::Imported { notes: 0 } => ("check", "Imported".to_string(), rgb(0x4ade80)),
                                            Status::Imported { notes } => (
                                                "check",
                                                format!("Imported · {notes} note{}", if *notes == 1 { "" } else { "s" }),
                                                rgb(0x4ade80),
                                            ),
                                            Status::Failed(err) => ("x", err.clone(), rgb(0xfbbf24)),
                                        };
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(12.))
                                            .px(px(18.))
                                            .py(px(11.))
                                            .when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                                            .child(icon("file-text").size(px(16.)).text_color(theme::muted()))
                                            .child(div().flex_1().min_w_0().text_ellipsis().overflow_hidden().whitespace_nowrap().child(row.source.clone()))
                                            .child(icon(glyph).size(px(14.)).text_color(color))
                                            .child(div().max_w(px(260.)).text_size(px(12.)).text_color(color).text_ellipsis().child(status))
                                            .with_animation(("row", i), Animation::new(anim::QUICK).with_easing(anim::ease_out), |el, t| el.opacity(t))
                                    })),
                            )
                        }),
                ),
            )
    }
}
