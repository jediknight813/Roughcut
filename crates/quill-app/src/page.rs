//! Lays out and paints a document as a column of wrapped blocks, and maps
//! between screen points and document positions for the editor.

use gpui_kit::*;
use std::ops::Range;
use std::rc::Rc;

use quill_doc::{Block, BlockKind, Document, Marks, Pos};

use crate::editor::EditorView;
use crate::theme;

pub const TEXT_WIDTH: f32 = 600.;
pub const PAPER_HEIGHT: f32 = 1000.;
pub const PAPER_GAP: f32 = 32.;
pub const PAPER_MARGIN: f32 = 72.;
const PAPER_BODY: f32 = PAPER_HEIGHT - 2. * PAPER_MARGIN;
const CARET_WIDTH: f32 = 2.;

struct BlockStyle {
    size: f32,
    weight: FontWeight,
    italic: bool,
    indent: f32,
    space_after: f32,
    family: &'static str,
}

impl BlockStyle {
    fn scaled(self, scale: f32) -> BlockStyle {
        BlockStyle {
            size: self.size * scale,
            indent: self.indent * scale,
            space_after: self.space_after * scale,
            ..self
        }
    }
}

fn block_style(kind: BlockKind) -> BlockStyle {
    let body = BlockStyle {
        size: 18.,
        weight: FontWeight::NORMAL,
        italic: false,
        indent: 0.,
        space_after: 14.,
        family: theme::PAGE_FONT,
    };
    match kind {
        BlockKind::Paragraph => body,
        BlockKind::Heading(1) => BlockStyle { size: 32., weight: FontWeight::BOLD, space_after: 20., ..body },
        BlockKind::Heading(_) => BlockStyle { size: 23., weight: FontWeight::SEMIBOLD, space_after: 12., ..body },
        BlockKind::Bullet | BlockKind::Numbered => BlockStyle { indent: 28., space_after: 6., ..body },
        BlockKind::Quote => BlockStyle { italic: true, indent: 24., ..body },
    }
}

#[derive(Clone)]
pub struct BlockLayout {
    /// Top-left of the block's first line, relative to the page's text origin.
    pub origin: Point<Pixels>,
    pub line_height: Pixels,
    pub height: Pixels,
    line: Rc<WrappedLine>,
    marker: Option<ShapedLine>,
    rows: Vec<Row>,
    align: TextAlign,
    width: Pixels,
}

#[derive(Clone)]
struct Row {
    start: usize,
    end: usize,
    x: Pixels,
    y: Pixels,
}

#[derive(Clone)]
pub struct PageLayout {
    pub blocks: Rc<Vec<BlockLayout>>,
    pub height: Pixels,
    pub pages: usize,
    paginated: bool,
    /// Left edge and width of the text column.
    left: Pixels,
    width: Pixels,
    scale: f32,
    origin: Point<Pixels>,
}

struct CachedBlock {
    block: Block,
    misspelled: Vec<Range<usize>>,
    dimmed: bool,
    number: usize,
    layout: BlockLayout,
}

/// Keeps shaped paragraphs across caret blinks, edits and scroll frames.
#[derive(Default)]
pub(crate) struct LayoutCache {
    key: Option<(u64, u32, Option<usize>, Option<Pos>)>,
    blocks: Vec<CachedBlock>,
    page: Option<PageLayout>,
    pub shaped_blocks: usize,
}

pub(crate) struct LayoutState {
    pub revision: u64,
    pub scale: f32,
    pub dim_except: Option<usize>,
    pub typing: Option<Pos>,
}

impl LayoutCache {
    /// Lays out `doc` at `scale` (1.0 = writing size). With `dim_except`,
    /// every block but that one is faded, for focus mode.
    pub fn shape(
        &mut self,
        doc: &Document,
        state: LayoutState,
        misspelled: impl FnOnce() -> Vec<Vec<Range<usize>>>,
        window: &mut Window,
    ) -> PageLayout {
        let LayoutState { revision, scale, dim_except, typing } = state;
        let key = (revision, scale.to_bits(), dim_except, typing);
        if self.key == Some(key) {
            return self.page.as_ref().expect("cached page").clone();
        }
        if self.key.is_some_and(|old| old.1 != key.1) {
            self.blocks.clear();
        }
        let misspelled = misspelled();
        let mut previous = std::mem::take(&mut self.blocks).into_iter();
        let mut y = px(0.);
        let mut number = 0;
        let blocks = doc
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| {
                number = if block.kind == BlockKind::Numbered { number + 1 } else { 0 };
                let spelling = misspelled.get(i).map_or(&[][..], Vec::as_slice);
                let dimmed = dim_except.is_some_and(|keep| keep != i);
                let mut style = block_style(block.kind);
                if let Some(size) = block.style.size {
                    style.size = size as f32;
                }
                style.family = crate::editor::formatting::font_family(block.style.typeface);
                let style = style.scaled(scale);
                if let Some(mut cached) = previous.next()
                    && cached.block == *block
                    && cached.misspelled == spelling
                    && cached.dimmed == dimmed
                    && cached.number == number
                {
                    place_rows(&mut cached.layout, &mut y, scale, dim_except.is_none());
                    y += px(style.space_after);
                    let layout = cached.layout.clone();
                    self.blocks.push(cached);
                    return layout;
                }
                self.shaped_blocks += 1;
                let color: Hsla = if dimmed {
                    theme::text_dimmed()
                } else {
                    block.style.color.map(rgb).unwrap_or_else(theme::text).into()
                };
                let font_size = px(style.size);
                let line_height = px(style.size * 1.6);
                let runs = runs(block, &style, color, spelling);
                let line = window
                    .text_system()
                    .shape_text(
                        block.text().into(),
                        font_size,
                        &runs,
                        Some(px(TEXT_WIDTH * scale - style.indent)),
                        None,
                    )
                    .ok()
                    .and_then(|lines| lines.into_iter().next())
                    .unwrap_or_default();
                let marker = match block.kind {
                    BlockKind::Bullet => Some("•".to_string()),
                    BlockKind::Numbered => Some(format!("{number}.")),
                    _ => None,
                }
                .map(|text| {
                    let run = TextRun { len: text.len(), ..plain_run(&style, color) };
                    window.text_system().shape_line(text.into(), font_size, &[run], None)
                });
                let height = line.size(line_height).height.max(line_height);
                let mut layout = BlockLayout {
                    origin: point(px(style.indent), y),
                    line_height,
                    height,
                    line: Rc::new(line),
                    marker,
                    rows: Vec::new(),
                    align: crate::editor::formatting::alignment(block.style.alignment),
                    width: px(TEXT_WIDTH * scale - style.indent),
                };
                place_rows(&mut layout, &mut y, scale, dim_except.is_none());
                y += px(style.space_after);
                self.blocks.push(CachedBlock {
                    block: block.clone(),
                    misspelled: spelling.to_vec(),
                    dimmed,
                    number,
                    layout: layout.clone(),
                });
                layout
            })
            .collect();
        let paginated = dim_except.is_none();
        let pages =
            if paginated { ((f32::from(y) / scale) / (PAPER_HEIGHT + PAPER_GAP)).floor() as usize + 1 } else { 1 };
        let page = PageLayout {
            blocks: Rc::new(blocks),
            height: if paginated {
                px((pages as f32 * (PAPER_HEIGHT + PAPER_GAP) - PAPER_GAP - PAPER_MARGIN * 2.) * scale)
            } else {
                y
            },
            pages,
            paginated,
            left: px(0.),
            width: px(TEXT_WIDTH * scale),
            scale,
            origin: Point::default(),
        };
        self.key = Some(key);
        self.page = Some(page.clone());
        page
    }
}

impl PageLayout {
    pub(crate) fn translate(&mut self, by: Point<Pixels>) {
        self.left += by.x;
        self.origin += by;
    }

    /// Top of the caret for `pos`.
    pub fn point_for_pos(&self, pos: Pos) -> Option<Point<Pixels>> {
        let block = self.blocks.get(pos.block)?;
        let row = block.rows.iter().find(|r| pos.offset < r.end).or_else(|| block.rows.last())?;
        let x =
            block.line.unwrapped_layout.x_for_index(pos.offset) - block.line.unwrapped_layout.x_for_index(row.start);
        Some(self.origin + point(block.origin.x + row.x + x, row.y))
    }

    pub fn line_height_at(&self, block: usize) -> Pixels {
        self.blocks.get(block).map_or(px(28.), |b| b.line_height)
    }

    /// Adjacent visual row, skipping sheet margins and paragraph spacing.
    pub(crate) fn adjacent_row(&self, pos: Pos, down: bool, x: Pixels) -> Pos {
        let Some(block) = self.blocks.get(pos.block) else {
            return pos;
        };
        let row = block.rows.iter().position(|r| pos.offset < r.end).unwrap_or(block.rows.len() - 1);
        let (block_index, row_index) = if down {
            if row + 1 < block.rows.len() {
                (pos.block, row + 1)
            } else if pos.block + 1 < self.blocks.len() {
                (pos.block + 1, 0)
            } else {
                return Pos::new(pos.block, block.line.len());
            }
        } else if row > 0 {
            (pos.block, row - 1)
        } else if pos.block > 0 {
            (pos.block - 1, self.blocks[pos.block - 1].rows.len() - 1)
        } else {
            return Pos::default();
        };
        let target = &self.blocks[block_index];
        self.pos_for_point(point(x, self.origin.y + target.rows[row_index].y + target.line_height / 2.))
    }

    pub fn pos_for_point(&self, point: Point<Pixels>) -> Pos {
        let point = point - self.origin;
        let Some(last) = self.blocks.len().checked_sub(1) else {
            return Pos::default();
        };
        let index = self.blocks.partition_point(|b| point.y >= b.origin.y + b.height).min(last);
        let block = &self.blocks[index];
        let row = block.rows.iter().find(|r| point.y < r.y + block.line_height).or_else(|| block.rows.last()).unwrap();
        let x = point.x - block.origin.x - row.x + block.line.unwrapped_layout.x_for_index(row.start);
        let offset = block.line.unwrapped_layout.closest_index_for_x(x).clamp(row.start, row.end);
        Pos::new(index, offset)
    }

    pub(crate) fn paint_text(&self, window: &mut Window, cx: &mut App) {
        let visible = window.content_mask().bounds;
        let first = self.blocks.partition_point(|b| self.origin.y + b.origin.y + b.height < visible.top());
        for block in self.blocks[first..].iter().take_while(|b| self.origin.y + b.origin.y < visible.bottom()) {
            // Paint a continuous run once, clipping at page boundaries.
            let mut index = 0;
            while index < block.rows.len() {
                let mut end = index + 1;
                while end < block.rows.len()
                    && (block.rows[end].y - block.rows[end - 1].y - block.line_height).abs() < px(0.1)
                {
                    end += 1;
                }
                let top = self.origin.y + block.rows[index].y;
                let bottom = self.origin.y + block.rows[end - 1].y + block.line_height;
                if bottom >= visible.top() && top <= visible.bottom() {
                    let origin = point(self.origin.x + block.origin.x, top - block.line_height * index);
                    let row_bounds = Bounds::new(point(origin.x, top), size(block.width, bottom - top));
                    window.with_content_mask(Some(ContentMask { bounds: row_bounds.intersect(&visible) }), |window| {
                        let _ = block.line.paint_background(
                            origin,
                            block.line_height,
                            block.align,
                            Some(row_bounds),
                            window,
                            cx,
                        );
                        let _ = block.line.paint(origin, block.line_height, block.align, Some(row_bounds), window, cx);
                    });
                }
                index = end;
            }
            if let Some(marker) = &block.marker {
                let at = self.origin + block.origin - point(px(22. * self.scale), px(0.));
                let _ = marker.paint(at, block.line_height, TextAlign::Left, None, window, cx);
            }
        }
    }

    pub(crate) fn selection_quads(&self, start: Pos, end: Pos) -> Vec<PaintQuad> {
        let mut quads = Vec::new();
        for i in start.block..=end.block {
            let Some(block) = self.blocks.get(i) else {
                break;
            };
            let from = if i == start.block { start.offset } else { 0 };
            let to = if i == end.block { end.offset } else { block.line.len() };
            for row in &block.rows {
                let a = from.max(row.start);
                let b = to.min(row.end);
                if a > b || (a == b && i == end.block) {
                    continue;
                }
                let left = self.origin.x + block.origin.x + row.x;
                let start_x = block.line.unwrapped_layout.x_for_index(row.start);
                let ax = left + block.line.unwrapped_layout.x_for_index(a) - start_x;
                let bx = left + block.line.unwrapped_layout.x_for_index(b) - start_x;
                let tail = if i < end.block && b == block.line.len() { px(8.) } else { px(0.) };
                quads.push(fill(
                    Bounds::new(
                        point(ax, self.origin.y + row.y),
                        size((bx - ax + tail).max(px(0.)), block.line_height),
                    ),
                    theme::selection(),
                ));
            }
        }
        quads
    }
}

fn plain_run(style: &BlockStyle, color: Hsla) -> TextRun {
    TextRun {
        len: 0,
        font: Font {
            weight: style.weight,
            style: if style.italic { FontStyle::Italic } else { FontStyle::Normal },
            ..font(style.family)
        },
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Text runs for a block: one per span, split further wherever a
/// misspelled word starts or ends so it can carry a wavy underline.
fn runs(block: &Block, style: &BlockStyle, color: Hsla, misspelled: &[Range<usize>]) -> Vec<TextRun> {
    let base = plain_run(style, color);
    let mut out = Vec::new();
    let mut start = 0;
    for span in &block.spans {
        let end = start + span.text.len();
        let mut run = TextRun { len: 0, ..base.clone() };
        if span.marks.contains(Marks::BOLD) {
            run.font.weight = FontWeight::BOLD;
        }
        if span.marks.contains(Marks::ITALIC) {
            run.font.style = if style.italic { FontStyle::Normal } else { FontStyle::Italic };
        }
        if span.marks.contains(Marks::UNDERLINE) {
            run.underline = Some(UnderlineStyle { color: Some(run.color), thickness: px(1.), wavy: false });
        }
        if span.marks.contains(Marks::STRIKE) {
            run.strikethrough = Some(StrikethroughStyle { color: Some(run.color), thickness: px(1.) });
        }
        if span.marks.contains(Marks::HIGHLIGHT) {
            run.background_color = Some(theme::highlight());
        }
        let mut cuts: Vec<usize> =
            misspelled.iter().flat_map(|r| [r.start, r.end]).filter(|&c| c > start && c < end).collect();
        cuts.push(end);
        cuts.sort_unstable();
        cuts.dedup();
        let mut from = start;
        for to in cuts {
            let mut piece = TextRun { len: to - from, ..run.clone() };
            if misspelled.iter().any(|r| r.start <= from && to <= r.end) {
                piece.underline =
                    Some(UnderlineStyle { color: Some(theme::misspelled()), thickness: px(1.5), wavy: true });
            }
            out.push(piece);
            from = to;
        }
        start = end;
    }
    out
}

pub struct PageElement {
    pub editor: Entity<EditorView>,
}

pub struct Prepainted {
    selection: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
}

impl IntoElement for PageElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PageElement {
    type RequestLayoutState = Option<PageLayout>;
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout = self.editor.update(cx, |editor, _| editor.shape_page(window));
        let mut style = Style::default();
        style.size.width = layout.width.into();
        style.size.height = layout.height.into();
        (window.request_layout(style, [], cx), Some(layout))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        _: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let layout = layout.as_mut().expect("laid out");
        layout.translate(bounds.origin);
        let editor = self.editor.read(cx);
        let (start, end) = editor.selection();
        let selection = if start == end { Vec::new() } else { layout.selection_quads(start, end) };
        let caret = (start == end).then(|| layout.point_for_pos(editor.head)).flatten().map(|p| {
            fill(Bounds::new(p, size(px(CARET_WIDTH), layout.line_height_at(editor.head.block))), theme::accent())
        });
        Prepainted { selection, caret }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepainted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = layout.take().expect("laid out");
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);

        if layout.paginated {
            let visible = window.content_mask().bounds;
            for page in 0..layout.pages {
                let paper = Bounds::new(
                    layout.origin
                        + point(
                            px(-80. * layout.scale),
                            px((page as f32 * (PAPER_HEIGHT + PAPER_GAP) - PAPER_MARGIN) * layout.scale),
                        ),
                    size(px((TEXT_WIDTH + 160.) * layout.scale), px(PAPER_HEIGHT * layout.scale)),
                );
                if paper.bottom() >= visible.top() && paper.top() <= visible.bottom() {
                    window.paint_quad(quad(paper, px(6.), theme::page(), px(1.), rgb(0x2c303a), BorderStyle::Solid));
                }
            }
        }
        for quad in prepainted.selection.drain(..) {
            window.paint_quad(quad);
        }
        layout.paint_text(window, cx);
        if focus.is_focused(window)
            && self.editor.read(cx).caret_on
            && self.editor.read(cx).mode != crate::editor::Mode::Board
            && let Some(caret) = prepainted.caret.take()
        {
            window.paint_quad(caret);
        }
        self.editor.update(cx, |editor, cx| {
            editor.reveal_caret(&layout, cx);
            editor.layout = Some(layout);
        });
    }
}

/// Positions wrapped rows on sheets without reshaping unchanged paragraphs.
fn place_rows(block: &mut BlockLayout, y: &mut Pixels, scale: f32, paginated: bool) {
    block.rows.clear();
    let mut start = 0;
    let ends: Vec<usize> = block
        .line
        .wrap_boundaries
        .iter()
        .map(|b| block.line.runs()[b.run_ix].glyphs[b.glyph_ix].index)
        .chain([block.line.len()])
        .collect();
    for end in ends {
        if paginated {
            let stride = px((PAPER_HEIGHT + PAPER_GAP) * scale);
            let page = (*y / stride).floor();
            if *y + block.line_height > stride * page + px(PAPER_BODY * scale) {
                *y = stride * (page + 1.);
            }
        }
        let width = block.line.unwrapped_layout.x_for_index(end) - block.line.unwrapped_layout.x_for_index(start);
        let remaining = (block.width - width).max(px(0.));
        let x = match block.align {
            TextAlign::Center => remaining / 2.,
            TextAlign::Right => remaining,
            _ => px(0.),
        };
        block.rows.push(Row { start, end, x, y: *y });
        *y += block.line_height;
        start = end;
    }
    block.origin.y = block.rows[0].y;
    block.height = *y - block.origin.y;
}

pub(crate) fn canvas_layout(doc: &Document, width: Pixels, scale: f32, window: &mut Window) -> PageLayout {
    let mut y = px(0.);
    let blocks = doc
        .blocks
        .iter()
        .map(|block| {
            let size = block.style.size.unwrap_or(16) as f32 * scale;
            let style = BlockStyle {
                size,
                weight: FontWeight::NORMAL,
                italic: false,
                indent: 0.,
                space_after: 0.,
                family: crate::editor::formatting::font_family(block.style.typeface),
            };
            let color = block.style.color.map(rgb).unwrap_or_else(theme::text_soft).into();
            let runs = runs(block, &style, color, &[]);
            let line = window
                .text_system()
                .shape_text(block.text().into(), px(size), &runs, Some(width), None)
                .ok()
                .and_then(|l| l.into_iter().next())
                .unwrap_or_default();
            let mut layout = BlockLayout {
                origin: point(px(0.), y),
                line_height: px(size * 1.5),
                height: px(0.),
                line: Rc::new(line),
                marker: None,
                rows: Vec::new(),
                align: crate::editor::formatting::alignment(block.style.alignment),
                width,
            };
            place_rows(&mut layout, &mut y, scale, false);
            layout
        })
        .collect();
    PageLayout {
        blocks: Rc::new(blocks),
        height: y,
        pages: 1,
        paginated: false,
        left: px(0.),
        width,
        scale,
        origin: Point::default(),
    }
}
