//! The library screen: writing stats on top, then every document.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::*;
use quill_doc::Document;
use quill_store::{DocId, DocSummary, Library, Stats};

use crate::theme;

const RECENT_DAYS: usize = 14;
const DAILY_GOAL: i64 = 1000;

pub enum HomeEvent {
    Open(DocId),
}

pub struct HomeView {
    library: Rc<RefCell<Library>>,
    docs: Vec<DocSummary>,
    stats: Option<Stats>,
    notice: Option<String>,
}

impl EventEmitter<HomeEvent> for HomeView {}

impl HomeView {
    pub fn new(library: Rc<RefCell<Library>>, notice: Option<String>) -> HomeView {
        let mut home = HomeView {
            library,
            docs: Vec::new(),
            stats: None,
            notice,
        };
        home.reload();
        home
    }

    pub fn reload(&mut self) {
        let library = self.library.borrow();
        self.docs = library.list(None).unwrap_or_else(|err| {
            eprintln!("quill: listing documents failed: {err}");
            Vec::new()
        });
        self.stats = library.stats(quill_store::today(), RECENT_DAYS).ok();
    }

    fn new_document(&mut self, cx: &mut Context<Self>) {
        let created = self
            .library
            .borrow_mut()
            .create(&Document::new("Untitled"), None);
        match created {
            Ok(id) => cx.emit(HomeEvent::Open(id)),
            Err(err) => eprintln!("quill: creating a document failed: {err}"),
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(24.))
            .py(px(12.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .font_family(theme::PAGE_FONT)
                    .text_size(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Quill"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("new-doc")
                    .px(px(16.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(theme::accent())
                    .text_color(theme::on_accent())
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .child("New document")
                    .on_click(cx.listener(|this, _, _, cx| this.new_document(cx))),
            )
    }

    fn stats_band(&self) -> impl IntoElement {
        let Some(stats) = &self.stats else {
            return div();
        };
        let recent_total: i64 = stats.recent_days.iter().map(|(_, w)| w).sum();
        let pct = stats.today * 100 / DAILY_GOAL;
        let tiles = [
            (
                "Words written",
                group(stats.total_words_written),
                format!("{} in the last {RECENT_DAYS} days", group(recent_total)),
            ),
            ("Documents", stats.documents.to_string(), String::new()),
            (
                "Current streak",
                days(stats.current_streak),
                format!("Best: {}", days(stats.best_streak)),
            ),
            (
                "Today",
                group(stats.today),
                format!("{pct}% of {}-word goal", group(DAILY_GOAL)),
            ),
        ];
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(24.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(16.))
                    .children(tiles.into_iter().map(|(label, value, sub)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .flex_1()
                            .min_w(px(180.))
                            .p(px(16.))
                            .rounded(px(12.))
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::muted())
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_size(px(30.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(value),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::muted())
                                    .child(sub),
                            )
                    })),
            )
            .child(words_per_day(&stats.recent_days))
    }

    fn notice(&self) -> Option<impl IntoElement> {
        self.notice.as_ref().map(|text| {
            div()
                .mx(px(24.))
                .mt(px(16.))
                .p(px(12.))
                .rounded(px(8.))
                .bg(theme::selected_bg())
                .text_color(theme::text())
                .child(text.clone())
        })
    }

    fn grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(24.))
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("All documents"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.))
                    .children(self.docs.iter().map(|doc| {
                        let id = doc.id;
                        div()
                            .id(("doc", id as u64))
                            .w(px(200.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeEvent::Open(id))))
                            .child(
                                div()
                                    .h(px(256.))
                                    .p(px(16.))
                                    .rounded(px(6.))
                                    .bg(theme::surface())
                                    .border_1()
                                    .border_color(rgb(0x353a45))
                                    .hover(|s| s.border_color(theme::accent()))
                                    .overflow_hidden()
                                    .font_family(theme::PAGE_FONT)
                                    .text_size(px(9.))
                                    .line_height(px(14.))
                                    .text_color(theme::text_soft())
                                    .child(doc.preview.clone()),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(doc.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::muted())
                                    .child(format!("{} words", group(doc.word_count))),
                            )
                    })),
            )
    }
}

impl Render for HomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("home")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(theme::app_bg())
            .text_color(theme::text())
            .font_family(theme::UI_FONT)
            .text_size(px(14.))
            .child(self.header(cx))
            .children(self.notice())
            .child(self.stats_band())
            .child(self.grid(cx))
    }
}

/// One bar per day; the newest bar is today. Days with no writing show as a
/// short grey stub so the axis still reads as continuous.
fn words_per_day(days: &[(jiff::civil::Date, i64)]) -> impl IntoElement {
    const HEIGHT: f32 = 120.;
    let max = days
        .iter()
        .map(|(_, w)| *w)
        .max()
        .unwrap_or(0)
        .max(DAILY_GOAL) as f32;
    let avg = days.iter().map(|(_, w)| w).sum::<i64>() / days.len().max(1) as i64;
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(16.))
        .rounded(px(12.))
        .border_1()
        .border_color(theme::border())
        .child(
            div()
                .flex()
                .gap(px(12.))
                .items_end()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Words per day"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(format!("Last {} days · avg {}", days.len(), group(avg))),
                ),
        )
        .child(
            div()
                .h(px(HEIGHT))
                .flex()
                .items_end()
                .gap(px(2.))
                .border_b_1()
                .border_color(rgb(0x353a45))
                .children(days.iter().map(|(_, words)| {
                    let h = if *words == 0 {
                        2.
                    } else {
                        (*words as f32 / max * HEIGHT).max(3.)
                    };
                    div()
                        .flex_1()
                        .flex()
                        .justify_center()
                        .items_end()
                        .h_full()
                        .child(
                            div()
                                .w(relative(0.7))
                                .max_w(px(28.))
                                .h(px(h))
                                .rounded_t(px(4.))
                                .bg(if *words == 0 {
                                    rgb(0x353a45)
                                } else {
                                    theme::accent()
                                }),
                        )
                })),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(11.))
                .text_color(theme::muted())
                .child(
                    days.first()
                        .map(|(d, _)| d.strftime("%b %-d").to_string())
                        .unwrap_or_default(),
                )
                .child("Today"),
        )
}

fn days(n: u32) -> String {
    if n == 1 {
        "1 day".into()
    } else {
        format!("{n} days")
    }
}

/// 12345 -> "12,345"
fn group(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups_thousands() {
        assert_eq!(super::group(0), "0");
        assert_eq!(super::group(999), "999");
        assert_eq!(super::group(1000), "1,000");
        assert_eq!(super::group(1234567), "1,234,567");
        assert_eq!(super::group(-4200), "-4,200");
    }
}
