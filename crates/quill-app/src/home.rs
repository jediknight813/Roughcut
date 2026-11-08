//! The library screen: writing stats on top, then every document, with
//! search and collection filters.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::civil::Date;
use quill_doc::Document;
use quill_store::{DocId, DocSummary, Library, Stats};

use crate::theme;

const BAR_DAYS: usize = 14;
const HEATMAP_WEEKS: usize = 15;
const DAILY_GOAL: i64 = 1000;
/// Heatmap steps from empty to most, light-on-dark.
const RAMP: [u32; 5] = [0x23272f, 0x22305a, 0x2f4a9a, 0x4f72d9, 0x9db4f5];

pub enum HomeEvent {
    Open(DocId),
    Import,
}

pub struct HomeView {
    library: Rc<RefCell<Library>>,
    docs: Vec<DocSummary>,
    stats: Option<Stats>,
    collections: Vec<String>,
    filter: Option<String>,
    search: Entity<InputState>,
    notice: Option<String>,
    _search_changed: Subscription,
}

impl EventEmitter<HomeEvent> for HomeView {}

impl HomeView {
    pub fn new(
        library: Rc<RefCell<Library>>,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HomeView {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let search_changed = cx.subscribe(&search, |this, _, event, cx| {
            if let InputEvent::Change = event {
                this.reload(cx);
            }
        });
        let mut home = HomeView {
            library,
            docs: Vec::new(),
            stats: None,
            collections: Vec::new(),
            filter: None,
            search,
            notice,
            _search_changed: search_changed,
        };
        home.reload(cx);
        home
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_string();
        let library = self.library.borrow();
        let docs = if query.trim().is_empty() { library.list(self.filter.as_deref()) } else { library.search(&query) };
        self.docs = docs.unwrap_or_else(|err| {
            eprintln!("quill: listing documents failed: {err}");
            Vec::new()
        });
        if !query.trim().is_empty()
            && let Some(filter) = &self.filter
        {
            self.docs.retain(|d| d.collection.as_ref() == Some(filter));
        }
        self.collections = library.collections().unwrap_or_default();
        self.stats = library.stats(quill_store::today(), HEATMAP_WEEKS * 7).ok();
        cx.notify();
    }

    fn new_document(&mut self, cx: &mut Context<Self>) {
        let created = self.library.borrow_mut().create(&Document::new("Untitled"), self.filter.as_deref());
        match created {
            Ok(id) => cx.emit(HomeEvent::Open(id)),
            Err(err) => eprintln!("quill: creating a document failed: {err}"),
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .px(px(24.))
            .py(px(12.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div().font_family(theme::PAGE_FONT).text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child("Quill"),
            )
            .child(div().flex_1().max_w(px(560.)).min_w(px(200.)).child(Input::new(&self.search).cleanable(true)))
            .child(div().flex_1())
            .child(button("import", "Import", false).on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::Import))))
            .child(
                button("new-doc", "New document", true).on_click(cx.listener(|this, _, _, cx| this.new_document(cx))),
            )
    }

    fn stats_band(&self) -> impl IntoElement {
        let Some(stats) = &self.stats else {
            return div();
        };
        let bars = &stats.recent_days[stats.recent_days.len().saturating_sub(BAR_DAYS)..];
        let recent_total: i64 = bars.iter().map(|(_, w)| w).sum();
        let pct = stats.today * 100 / DAILY_GOAL;
        let tiles = [
            (
                "Words written",
                group(stats.total_words_written),
                format!("{} in the last {BAR_DAYS} days", group(recent_total)),
            ),
            ("Documents", stats.documents.to_string(), String::new()),
            ("Current streak", days(stats.current_streak), format!("Best: {}", days(stats.best_streak))),
            ("Today", group(stats.today), format!("{pct}% of {}-word goal", group(DAILY_GOAL))),
        ];
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(24.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(div().flex().flex_wrap().gap(px(16.)).children(tiles.into_iter().map(|(label, value, sub)| {
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
                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(label))
                    .child(div().text_size(px(30.)).font_weight(FontWeight::SEMIBOLD).child(value))
                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(sub))
            })))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(16.))
                    .child(div().flex_grow(1.).flex_basis(px(440.)).child(words_per_day(bars)))
                    .child(div().flex_grow(1.).flex_basis(px(320.)).child(heatmap(&stats.recent_days))),
            )
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

    fn chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let all = std::iter::once(None).chain(self.collections.iter().cloned().map(Some));
        div().flex().flex_wrap().gap(px(6.)).children(all.enumerate().map(|(i, collection)| {
            let active = self.filter == collection;
            let label = collection.clone().unwrap_or_else(|| "All".into());
            div()
                .id(("chip", i))
                .px(px(14.))
                .h(px(34.))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .cursor_pointer()
                .when(active, |d| d.bg(theme::selected_bg()).border_color(rgb(0x3d5bb8)).text_color(rgb(0x9db4f5)))
                .when(!active, |d| d.border_color(theme::border()).text_color(theme::text_soft()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter = collection.clone();
                    this.reload(cx);
                }))
        }))
    }

    fn grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let searching = !self.search.read(cx).value().trim().is_empty();
        let title = if searching { "Results" } else { "All documents" };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(24.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().flex_1())
                    .child(self.chips(cx)),
            )
            .when(self.docs.is_empty() && searching, |d| d.child(div().text_color(theme::muted()).child("No matches.")))
            .child(div().flex().flex_wrap().gap(px(20.)).children(self.docs.iter().map(|doc| {
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
                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(match &doc.collection {
                        Some(c) if self.filter.is_none() => format!("{} words · {c}", group(doc.word_count)),
                        _ => format!("{} words", group(doc.word_count)),
                    }))
            })))
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

pub fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(16.))
        .py(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD)
        .when(primary, |d| d.bg(theme::accent()).text_color(theme::on_accent()))
        .when(!primary, |d| d.border_1().border_color(rgb(0x353a45)).hover(|s| s.bg(theme::raised())))
        .child(label)
}

fn panel() -> Div {
    div().flex().flex_col().gap(px(10.)).p(px(16.)).rounded(px(12.)).border_1().border_color(theme::border())
}

/// One bar per day; the newest bar is today. Days with no writing show as a
/// short grey stub so the axis still reads as continuous.
fn words_per_day(days: &[(Date, i64)]) -> impl IntoElement {
    const HEIGHT: f32 = 120.;
    let max = days.iter().map(|(_, w)| *w).max().unwrap_or(0).max(DAILY_GOAL) as f32;
    let avg = days.iter().map(|(_, w)| w).sum::<i64>() / days.len().max(1) as i64;
    panel()
        .child(
            div()
                .flex()
                .gap(px(12.))
                .items_end()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Words per day"))
                .child(div().text_size(px(12.)).text_color(theme::muted()).child(format!(
                    "Last {} days · avg {}",
                    days.len(),
                    group(avg)
                ))),
        )
        .child(
            div()
                .relative()
                .h(px(HEIGHT))
                .flex()
                .items_end()
                .gap(px(2.))
                .border_b_1()
                .border_color(rgb(0x353a45))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(DAILY_GOAL as f32 / max * HEIGHT))
                        .border_t_1()
                        .border_color(theme::border()),
                )
                .children(days.iter().map(|(_, words)| {
                    let h = if *words == 0 { 2. } else { (*words as f32 / max * HEIGHT).max(3.) };
                    div().flex_1().flex().justify_center().items_end().h_full().child(
                        div().w(relative(0.7)).max_w(px(28.)).h(px(h)).rounded_t(px(4.)).bg(if *words == 0 {
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
                .child(days.first().map(|(d, _)| d.strftime("%b %-d").to_string()).unwrap_or_default())
                .child("Today"),
        )
}

/// Weeks as columns, days as rows, the last cell today.
fn heatmap(days: &[(Date, i64)]) -> impl IntoElement {
    let level = |w: i64| match w {
        0 => 0,
        1..500 => 1,
        500..900 => 2,
        900..1200 => 3,
        _ => 4,
    };
    let cell = |color: u32| div().size(px(14.)).rounded(px(3.)).bg(rgb(color));
    panel()
        .child(
            div().flex().gap(px(12.)).items_end().child(div().font_weight(FontWeight::SEMIBOLD).child("Streak")).child(
                div().text_size(px(12.)).text_color(theme::muted()).child(format!("Last {HEATMAP_WEEKS} weeks")),
            ),
        )
        .child(div().flex().gap(px(3.)).children(
            days.chunks(7).map(|week| {
                div().flex().flex_col().gap(px(3.)).children(week.iter().map(|(_, w)| cell(RAMP[level(*w)])))
            }),
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(theme::muted())
                .child("Less")
                .children(RAMP.iter().map(|c| cell(*c).size(px(12.))))
                .child("More"),
        )
}

fn days(n: u32) -> String {
    if n == 1 { "1 day".into() } else { format!("{n} days") }
}

/// 12345 -> "12,345"
pub fn group(n: i64) -> String {
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
