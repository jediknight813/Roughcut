//! Shared formatting controls for document paragraphs and selected canvas objects.
use super::*;
use quill_doc::{Alignment, TextStyle, Typeface};

#[derive(Clone, Copy)]
pub(crate) enum FormatMenu {
    Font,
    Size,
    Color,
}

pub(crate) fn font_family(font: Typeface) -> &'static str {
    match font {
        Typeface::Serif => theme::PAGE_FONT,
        Typeface::Sans => theme::UI_FONT,
        Typeface::Mono => "Menlo",
    }
}
pub(crate) fn alignment(align: Alignment) -> TextAlign {
    match align {
        Alignment::Left => TextAlign::Left,
        Alignment::Center => TextAlign::Center,
        Alignment::Right => TextAlign::Right,
    }
}

impl EditorView {
    pub fn formatting_menu_position(&self) -> Option<Point<Pixels>> {
        self.format_menu.map(|(_, at)| at)
    }

    fn text_style(&self) -> TextStyle {
        if self.mode == Mode::Board {
            return self
                .board
                .iter()
                .find(|i| Some(i.id) == self.selected_item)
                .and_then(|i| i.kind.text())
                .map(|(_, s)| s.typography.clone())
                .unwrap_or_default();
        }
        self.doc.blocks[self.head.block].style.clone()
    }

    fn apply_text_style(&mut self, change: impl Fn(&mut TextStyle), window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            self.update_board_style(|s| change(&mut s.typography), window, cx);
        } else {
            let (anchor, head) = (self.anchor, self.head);
            self.edit(EditKind::Other, cx, |doc, start, end| {
                for block in &mut doc.blocks[start.block..=end.block] {
                    change(&mut block.style);
                }
                end
            });
            self.anchor = anchor;
            self.head = head;
        }
    }

    fn clear_formatting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Board {
            let shape = self
                .board
                .iter()
                .find(|i| Some(i.id) == self.selected_item)
                .is_some_and(|i| matches!(i.kind, quill_store::ItemKind::Shape { .. }));
            self.update_board_style(
                |s| *s = if shape { quill_store::BoardTextStyle::shape() } else { Default::default() },
                window,
                cx,
            );
        } else {
            let (anchor, head) = (self.anchor, self.head);
            self.edit(EditKind::Other, cx, |doc, start, end| {
                for block in &mut doc.blocks[start.block..=end.block] {
                    block.style = TextStyle::default();
                    block.kind = BlockKind::Paragraph;
                    for span in &mut block.spans {
                        span.marks = Marks::NONE;
                    }
                }
                end
            });
            self.anchor = anchor;
            self.head = head;
        }
    }

    pub(super) fn format_controls(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let style = self.text_style();
        let font_label = match style.typeface {
            Typeface::Serif => "Serif ▾",
            Typeface::Sans => "Sans ▾",
            Typeface::Mono => "Mono ▾",
        };
        let mut controls = vec![separator().into_any_element()];
        for (id, label, menu) in [
            ("font-family", font_label.to_string(), FormatMenu::Font),
            (
                "font-size",
                format!("{} ▾", style.size.unwrap_or(if self.mode == Mode::Board { 16 } else { 18 })),
                FormatMenu::Size,
            ),
            ("text-color", "Color ▾".to_string(), FormatMenu::Color),
        ] {
            controls.push(
                div()
                    .id(id)
                    .h(px(32.))
                    .px(px(8.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::raised()))
                    .child(label)
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.format_menu = Some((menu, event.position() + point(px(0.), px(18.))));
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        for (id, glyph, tip, align) in [
            ("align-left", "align-left", "Align left", Alignment::Left),
            ("align-center", "align-center", "Center text", Alignment::Center),
            ("align-right", "align-right", "Align right", Alignment::Right),
        ] {
            controls.push(
                icon_button(id, glyph, tip, style.alignment == align)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_text_style(|s| s.alignment = align, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        controls.push(
            div()
                .id("clear-formatting")
                .h(px(32.))
                .px(px(8.))
                .flex_none()
                .flex()
                .items_center()
                .text_size(px(12.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme::raised()))
                .child("Clear")
                .tooltip(|window, cx| Tooltip::new("Clear text formatting").build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.clear_formatting(window, cx)))
                .into_any_element(),
        );
        controls
    }

    pub(super) fn format_menu_element(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (menu, at) = self.format_menu?;
        let mut options: Vec<(String, TextStyle)> = Vec::new();
        let current = self.text_style();
        match menu {
            FormatMenu::Font => {
                for (name, typeface) in
                    [("Serif", Typeface::Serif), ("Sans serif", Typeface::Sans), ("Monospace", Typeface::Mono)]
                {
                    options.push((name.into(), TextStyle { typeface, ..current.clone() }));
                }
            }
            FormatMenu::Size => {
                for size in [12, 14, 16, 18, 20, 24, 28, 32, 40, 48, 64, 72] {
                    options.push((format!("{size}"), TextStyle { size: Some(size), ..current.clone() }));
                }
            }
            FormatMenu::Color => {
                for (name, color) in [
                    ("Default", None),
                    ("Ink", Some(0x20232b)),
                    ("White", Some(0xf5f5f5)),
                    ("Blue", Some(0x7b9af0)),
                    ("Red", Some(0xf87171)),
                    ("Green", Some(0x4ade80)),
                    ("Gold", Some(0xfacc15)),
                    ("Pink", Some(0xf472b6)),
                ] {
                    options.push((name.into(), TextStyle { color, ..current.clone() }));
                }
            }
        }
        Some(
            deferred(
                anchored().position(at).snap_to_window().child(
                    div()
                        .id("format-menu")
                        .block_mouse_except_scroll()
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .w(px(160.))
                        .p(px(6.))
                        .rounded(px(8.))
                        .bg(theme::raised())
                        .border_1()
                        .border_color(theme::border())
                        .shadow(floating_shadow())
                        .children(options.into_iter().enumerate().map(|(i, (label, style))| {
                            div()
                                .id(("format-option", i))
                                .h(px(28.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::selected_bg()))
                                .text_size(px(13.))
                                .child(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.apply_text_style(|s| *s = style.clone(), window, cx);
                                    this.format_menu = None;
                                    cx.notify();
                                }))
                        }))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.format_menu = None;
                            cx.notify();
                        })),
                ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
