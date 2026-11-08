//! The library screen: writing stats on top, then every document, with
//! search and collection filters.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::civil::{Date, Weekday};
use quill_doc::Document;
use quill_store::{DocId, DocSummary, Library, Stats};

use crate::anim;
use crate::assets::icon;
use crate::theme;

const BAR_DAYS: usize = 14;
const HEATMAP_WEEKS: usize = 26;
const DAILY_GOAL: i64 = 1000;
/// Heatmap steps from empty to most, light-on-dark.
const RAMP: [u32; 5] = [0x1f232a, 0x22305a, 0x2f4a9a, 0x4f72d9, 0x9db4f5];
const CARD_WIDTH: f32 = 196.;

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
    /// Bumped whenever the visible list changes, so cards animate in again.
    generation: usize,
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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search titles and text"));
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
            generation: 0,
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
        self.stats = library.stats(quill_store::today(), HEATMAP_WEEKS * 7 + 7).ok();
        self.generation += 1;
        cx.notify();
    }

    #[cfg_attr(not(feature = "screenshots"), allow(dead_code))]
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.set_value(query.to_string(), window, cx));
        self.reload(cx);
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
            .items_center()
            .gap(px(16.))
            .px(px(28.))
            .h(px(64.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(div().font_family(theme::PAGE_FONT).text_size(px(24.)).font_weight(FontWeight::BOLD).child("Quill"))
            .child(div().flex_1())
            .child(
                div().w(px(380.)).child(
                    Input::new(&self.search)
                        .cleanable(true)
                        .prefix(icon("search").size(px(15.)).text_color(theme::muted())),
                ),
            )
            .child(
                button("import", "folder-open", "Import", false)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::Import))),
            )
            .child(
                button("new-doc", "plus", "New document", true)
                    .on_click(cx.listener(|this, _, _, cx| this.new_document(cx))),
            )
    }

    fn stats_band(&self) -> impl IntoElement {
        let Some(stats) = &self.stats else {
            return div().into_any_element();
        };
        let bars = &stats.recent_days[stats.recent_days.len().saturating_sub(BAR_DAYS)..];
        let recent_total: i64 = bars.iter().map(|(_, w)| w).sum();
        let goal = (stats.today as f32 / DAILY_GOAL as f32).min(1.);
        let tiles: [(&str, &str, String, String, Option<f32>); 4] = [
            (
                "sparkles",
                "Words written",
                group(stats.total_words_written),
                format!("{} in the last two weeks", group(recent_total)),
                None,
            ),
            ("file-text", "Documents", stats.documents.to_string(), "In your library".into(), None),
            ("clock", "Current streak", days(stats.current_streak), format!("Best {}", days(stats.best_streak)), None),
            ("pen-line", "Today", group(stats.today), format!("of {} words", group(DAILY_GOAL)), Some(goal)),
        ];
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .px(px(28.))
            .pt(px(28.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(12.))
                    .child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).child(greeting()))
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme::muted())
                            .child(quill_store::today().strftime("%A, %B %-d").to_string()),
                    ),
            )
            .child(div().flex().gap(px(16.)).children(tiles.into_iter().enumerate().map(
                |(i, (glyph, label, value, sub, progress))| {
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .p(px(18.))
                        .rounded(px(14.))
                        .bg(theme::surface())
                        .border_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(icon(glyph).size(px(14.)).text_color(theme::accent()))
                                .child(label),
                        )
                        .child(div().text_size(px(30.)).font_weight(FontWeight::SEMIBOLD).child(value))
                        .child(match progress {
                            Some(p) => div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(5.))
                                        .rounded_full()
                                        .bg(theme::raised())
                                        .child(div().h_full().rounded_full().w(relative(p)).bg(theme::accent())),
                                )
                                .child(div().text_size(px(12.)).text_color(theme::muted()).child(sub))
                                .into_any_element(),
                            None => div().text_size(px(12.)).text_color(theme::muted()).child(sub).into_any_element(),
                        })
                        .with_animation(("tile", i), entrance(i), |el, t| el.opacity(t).mt(px((1. - t) * 10.)))
                },
            )))
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .child(div().flex_1().min_w(px(0.)).child(words_per_day(bars)))
                    .child(div().flex_none().child(heatmap(&stats.recent_days))),
            )
            .into_any_element()
    }

    fn notice(&self) -> Option<impl IntoElement> {
        self.notice.as_ref().map(|text| {
            div()
                .mx(px(28.))
                .mt(px(20.))
                .p(px(14.))
                .rounded(px(10.))
                .bg(theme::selected_bg())
                .text_color(theme::text())
                .child(text.clone())
        })
    }

    fn chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let all = std::iter::once(None).chain(self.collections.iter().cloned().map(Some));
        div()
            .flex()
            .p(px(3.))
            .gap(px(2.))
            .rounded(px(10.))
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .children(all.enumerate().map(|(i, collection)| {
                let active = self.filter == collection;
                let label = collection.clone().unwrap_or_else(|| "All".into());
                div()
                    .id(("chip", i))
                    .px(px(14.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(active, |d| d.bg(rgb(0x3a3f4b)).text_color(theme::text()))
                    .when(!active, |d| d.text_color(theme::muted()).hover(|s| s.text_color(theme::text())))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = collection.clone();
                        this.reload(cx);
                    }))
            }))
    }

    fn grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let searching = !self.search.read(cx).value().trim().is_empty();
        let title = if searching { format!("Results · {}", self.docs.len()) } else { "Documents".into() };
        let generation = self.generation;
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(28.))
            .pt(px(32.))
            .pb(px(48.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).child(title))
                    .child(div().flex_1())
                    .child(self.chips(cx)),
            )
            .when(self.docs.is_empty(), |d| {
                d.child(
                    div()
                        .py(px(48.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(10.))
                        .text_color(theme::muted())
                        .child(
                            icon(if searching { "search" } else { "file-plus" })
                                .size(px(28.))
                                .text_color(theme::muted()),
                        )
                        .child(if searching { "Nothing matches that search." } else { "No documents yet." }),
                )
            })
            .child(div().flex().flex_wrap().gap(px(24.)).children(self.docs.iter().enumerate().map(|(i, doc)| {
                let id = doc.id;
                // A short first line reads as the document's title; anything
                // longer is just the opening paragraph.
                let (head, body) = match doc.preview.split_once('\n') {
                    Some((h, b)) if h.chars().count() <= 60 => (h.to_string(), b.replace('\n', "\n\n")),
                    _ => (String::new(), doc.preview.replace('\n', "\n\n")),
                };
                div()
                    .id(("doc", id as u64))
                    .w(px(CARD_WIDTH))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .cursor_pointer()
                    .group("card")
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeEvent::Open(id))))
                    .child(
                        div()
                            .relative()
                            .h(px(CARD_WIDTH * 1.29))
                            .px(px(18.))
                            .py(px(20.))
                            .rounded(px(8.))
                            .bg(theme::page())
                            .border_1()
                            .border_color(rgb(0x2c303a))
                            .shadow(card_shadow(false))
                            .group_hover("card", |s| s.border_color(rgb(0x4f72d9)))
                            .overflow_hidden()
                            .font_family(theme::PAGE_FONT)
                            .when(!head.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .line_height(px(14.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme::text())
                                        .mb(px(8.))
                                        .child(head),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(7.5))
                                    .line_height(px(11.5))
                                    .text_color(theme::text_soft())
                                    .child(body),
                            )
                            .child(div().absolute().left_0().right_0().bottom_0().h(px(60.)).bg(linear_gradient(
                                180.,
                                linear_color_stop(rgba(0x1c1f2600), 0.),
                                linear_color_stop(theme::page(), 1.),
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(doc.title.clone()),
                            )
                            .child(div().text_size(px(12.)).text_color(theme::muted()).child(card_meta(doc))),
                    )
                    .with_animation(("card", generation * 10_000 + i), entrance(i.min(12) + 2), |el, t| {
                        el.opacity(t).mt(px((1. - t) * 12.))
                    })
            })))
    }
}

impl Render for HomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .text_color(theme::text())
            .font_family(theme::UI_FONT)
            .text_size(px(14.))
            .child(self.header(cx))
            .child(
                div()
                    .id("home-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(self.notice())
                    .child(self.stats_band())
                    .child(self.grid(cx)),
            )
    }
}

/// A staggered entrance: item `i` starts a little after item `i - 1`.
fn entrance(i: usize) -> Animation {
    let delay = i as f32 * 0.045;
    let length = 0.42;
    let total = delay + length;
    Animation::new(Duration::from_secs_f32(total))
        .with_easing(move |t| anim::ease_out(((t * total - delay) / length).clamp(0., 1.)))
}

fn greeting() -> &'static str {
    match jiff::Zoned::now().hour() {
        5..12 => "Good morning",
        12..18 => "Good afternoon",
        _ => "Good evening",
    }
}

fn card_meta(doc: &DocSummary) -> String {
    let opened = doc.opened_at.to_zoned(jiff::tz::TimeZone::system()).date();
    let today = quill_store::today();
    let when = if opened == today {
        "Today".to_string()
    } else if today.yesterday().ok() == Some(opened) {
        "Yesterday".to_string()
    } else {
        opened.strftime("%b %-d").to_string()
    };
    format!("{when} · {} words", group(doc.word_count))
}

fn card_shadow(lifted: bool) -> Vec<BoxShadow> {
    let (y, blur, alpha) = if lifted { (12., 28., 0.5) } else { (4., 14., 0.35) };
    vec![BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }]
}

pub fn button(id: &'static str, glyph: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(34.))
        .px(px(14.))
        .flex()
        .items_center()
        .gap(px(7.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .when(primary, |d| {
            d.bg(theme::accent())
                .text_color(theme::on_accent())
                .hover(|s| s.bg(rgb(0x93adf3)))
                .child(icon(glyph).size(px(15.)).text_color(theme::on_accent()))
        })
        .when(!primary, |d| {
            d.bg(theme::raised())
                .hover(|s| s.bg(rgb(0x30353e)))
                .child(icon(glyph).size(px(15.)).text_color(theme::text_soft()))
        })
        .child(label)
}

fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(18.))
        .rounded(px(14.))
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
}

fn panel_title(title: &'static str, detail: String) -> impl IntoElement {
    div()
        .flex()
        .items_baseline()
        .gap(px(10.))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_size(px(12.)).text_color(theme::muted()).child(detail))
}

/// One bar per day; the newest bar is today and stands out. Days with no
/// writing show as a short stub so the axis still reads as continuous.
fn words_per_day(days: &[(Date, i64)]) -> impl IntoElement {
    const HEIGHT: f32 = 132.;
    let max = days.iter().map(|(_, w)| *w).max().unwrap_or(0).max(DAILY_GOAL) as f32;
    let avg = days.iter().map(|(_, w)| w).sum::<i64>() / days.len().max(1) as i64;
    let last = days.len().saturating_sub(1);
    panel()
        .child(panel_title("Words per day", format!("Average {}", group(avg))))
        .child(
            div()
                .relative()
                .h(px(HEIGHT))
                .flex()
                .items_end()
                .gap(px(6.))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(DAILY_GOAL as f32 / max * HEIGHT))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().flex_1().h(px(1.)).bg(rgb(0x30353f)))
                        .child(div().text_size(px(10.)).text_color(theme::muted()).child("Goal")),
                )
                .children(days.iter().enumerate().map(|(i, (date, words))| {
                    let h = if *words == 0 { 3. } else { (*words as f32 / max * HEIGHT).max(4.) };
                    let tip = format!("{} · {} words", date.strftime("%a %b %-d"), group(*words));
                    let color = match (*words, i == last) {
                        (0, _) => rgb(0x2a2f38),
                        (_, true) => theme::accent(),
                        _ => rgb(0x3d5bb8),
                    };
                    div()
                        .id(("bar", i))
                        .flex_1()
                        .h_full()
                        .flex()
                        .justify_center()
                        .items_end()
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(
                            div()
                                .w_full()
                                .max_w(px(26.))
                                .h(px(h))
                                .rounded_t(px(5.))
                                .rounded_b(px(2.))
                                .bg(color)
                                .with_animation(("grow", i), entrance(i), move |el, t| el.h(px(h * t))),
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

/// GitHub-style calendar: one column per week (Sunday at the top), the last
/// column ending today, with month labels along the top.
fn heatmap(days: &[(Date, i64)]) -> impl IntoElement {
    const CELL: f32 = 13.;
    const GAP: f32 = 3.;
    let level = |w: i64| match w {
        0 => 0,
        1..500 => 1,
        500..900 => 2,
        900..1200 => 3,
        _ => 4,
    };
    // Trim the front so the first column starts on a Sunday.
    let Some(&(today, _)) = days.last() else { return panel() };
    let in_last_week = today.weekday().to_sunday_zero_offset() as usize + 1;
    let wanted = (HEATMAP_WEEKS - 1) * 7 + in_last_week;
    let days = &days[days.len().saturating_sub(wanted)..];
    let weeks: Vec<&[(Date, i64)]> = days.chunks(7).collect();
    let cell = |color: u32| div().size(px(CELL)).rounded(px(3.)).bg(rgb(color));
    let total: i64 = days.iter().map(|(_, w)| w).sum();
    let active = days.iter().filter(|(_, w)| *w > 0).count();

    panel()
        .child(panel_title("Streak", format!("{active} writing days · {} words", group(total))))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .pt(px(CELL + 6.))
                        .text_size(px(9.))
                        .text_color(theme::muted())
                        .children(
                            [
                                Weekday::Sunday,
                                Weekday::Monday,
                                Weekday::Tuesday,
                                Weekday::Wednesday,
                                Weekday::Thursday,
                                Weekday::Friday,
                                Weekday::Saturday,
                            ]
                            .into_iter()
                            .map(|d| {
                                let label = match d {
                                    Weekday::Monday => "Mon",
                                    Weekday::Wednesday => "Wed",
                                    Weekday::Friday => "Fri",
                                    _ => "",
                                };
                                div().h(px(CELL)).flex().items_center().child(label)
                            }),
                        ),
                )
                .child(div().flex().gap(px(GAP)).children(weeks.iter().enumerate().map(|(wi, week)| {
                    let month = week.first().filter(|(d, _)| d.day() <= 7).map(|(d, _)| d.strftime("%b").to_string());
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .child(
                            div()
                                .h(px(CELL))
                                .mb(px(6. - GAP))
                                .w(px(CELL))
                                .text_size(px(9.))
                                .text_color(theme::muted())
                                .whitespace_nowrap()
                                .child(month.unwrap_or_default()),
                        )
                        .children(week.iter().enumerate().map(move |(di, (date, w))| {
                            let tip = format!("{} · {} words", date.strftime("%a %b %-d"), group(*w));
                            cell(RAMP[level(*w)])
                                .id(("heat", wi * 7 + di))
                                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        }))
                }))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(theme::muted())
                .child("Less")
                .children(RAMP.iter().map(|c| cell(*c).size(px(11.))))
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
