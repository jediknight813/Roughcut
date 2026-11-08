//! Lays out and paints a document as a column of wrapped blocks, and maps
//! between screen points and document positions for the editor.

use gpui_kit::*;
use quill_doc::{Block, BlockKind, Document, Marks, Pos};

use crate::editor::EditorView;
use crate::theme;

pub const TEXT_WIDTH: f32 = 600.;
const CARET_WIDTH: f32 = 2.;

struct BlockStyle {
    size: f32,
    weight: FontWeight,
    italic: bool,
    indent: f32,
    space_after: f32,
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
    let body = BlockStyle { size: 18., weight: FontWeight::NORMAL, italic: false, indent: 0., space_after: 14. };
    match kind {
        BlockKind::Paragraph => body,
        BlockKind::Heading(1) => BlockStyle { size: 32., weight: FontWeight::BOLD, space_after: 20., ..body },
        BlockKind::Heading(_) => BlockStyle { size: 23., weight: FontWeight::SEMIBOLD, space_after: 12., ..body },
        BlockKind::Bullet | BlockKind::Numbered => BlockStyle { indent: 28., space_after: 6., ..body },
        BlockKind::Quote => BlockStyle { italic: true, indent: 24., ..body },
    }
}

pub struct BlockLayout {
    /// Top-left of the block's first line, in window coordinates once
    /// painted.
    pub origin: Point<Pixels>,
    pub line_height: Pixels,
    pub height: Pixels,
    line: WrappedLine,
    marker: Option<ShapedLine>,
}

pub struct PageLayout {
    pub blocks: Vec<BlockLayout>,
    pub height: Pixels,
    /// Left edge and width of the text column.
    left: Pixels,
    width: Pixels,
    scale: f32,
}

impl PageLayout {
    /// Lays out `doc` at `scale` (1.0 = writing size). With `dim_except`,
    /// every block but that one is faded, for focus mode.
    pub fn shape(doc: &Document, scale: f32, dim_except: Option<usize>, window: &mut Window) -> PageLayout {
        let mut y = px(0.);
        let mut number = 0;
        let blocks = doc
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let style = block_style(block.kind).scaled(scale);
                let color: Hsla =
                    if dim_except.is_some_and(|keep| keep != i) { theme::text_dimmed() } else { theme::text().into() };
                let font_size = px(style.size);
                let line_height = px(style.size * 1.6);
                let runs = runs(block, &style, color);
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
                number = if block.kind == BlockKind::Numbered { number + 1 } else { 0 };
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
                let layout = BlockLayout { origin: point(px(style.indent), y), line_height, height, line, marker };
                y += height + px(style.space_after);
                layout
            })
            .collect();
        PageLayout { blocks, height: y, left: px(0.), width: px(TEXT_WIDTH * scale), scale }
    }

    fn translate(&mut self, by: Point<Pixels>) {
        self.left += by.x;
        for block in &mut self.blocks {
            block.origin += by;
        }
    }

    /// Top of the caret for `pos`.
    pub fn point_for_pos(&self, pos: Pos) -> Option<Point<Pixels>> {
        let block = self.blocks.get(pos.block)?;
        let local = block.line.position_for_index(pos.offset, block.line_height).unwrap_or_default();
        Some(block.origin + local)
    }

    pub fn line_height_at(&self, block: usize) -> Pixels {
        self.blocks.get(block).map_or(px(28.), |b| b.line_height)
    }

    pub fn pos_for_point(&self, point: Point<Pixels>) -> Pos {
        let Some(last) = self.blocks.len().checked_sub(1) else {
            return Pos::default();
        };
        let index = self.blocks.iter().position(|b| point.y < b.origin.y + b.height).unwrap_or(last);
        let block = &self.blocks[index];
        let local = point - block.origin;
        let local = gpui_kit::point(local.x, local.y.clamp(px(0.), block.height - px(1.)));
        let offset = match block.line.closest_index_for_position(local, block.line_height) {
            Ok(i) | Err(i) => i,
        };
        Pos::new(index, offset.min(block.line.len()))
    }

    fn selection_quads(&self, start: Pos, end: Pos) -> Vec<PaintQuad> {
        let mut quads = Vec::new();
        for i in start.block..=end.block {
            let Some(block) = self.blocks.get(i) else {
                break;
            };
            let from = if i == start.block { start.offset } else { 0 };
            let to = if i == end.block { end.offset } else { block.line.len() };
            let (Some(a), Some(b)) = (self.point_for_pos(Pos::new(i, from)), self.point_for_pos(Pos::new(i, to)))
            else {
                continue;
            };
            let lh = block.line_height;
            let left = block.origin.x;
            let right = self.left + self.width;
            // Selections running past the end of a block cover a little space
            // so a selected empty line or line break is still visible.
            let tail = if i < end.block { px(8.) } else { px(0.) };
            if a.y == b.y {
                quads.push(fill(Bounds::from_corners(a, point(b.x + tail, b.y + lh)), theme::selection()));
            } else {
                quads.push(fill(Bounds::from_corners(a, point(right, a.y + lh)), theme::selection()));
                if b.y > a.y + lh {
                    quads
                        .push(fill(Bounds::from_corners(point(left, a.y + lh), point(right, b.y)), theme::selection()));
                }
                quads.push(fill(
                    Bounds::from_corners(point(left, b.y), point(b.x + tail, b.y + lh)),
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
            ..font(theme::PAGE_FONT)
        },
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

fn runs(block: &Block, style: &BlockStyle, color: Hsla) -> Vec<TextRun> {
    let base = plain_run(style, color);
    block
        .spans
        .iter()
        .map(|span| {
            let mut run = TextRun { len: span.text.len(), ..base.clone() };
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
            run
        })
        .collect()
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
        let editor = self.editor.read(cx);
        let layout = PageLayout::shape(&editor.doc, editor.page_scale(), editor.dim_except(), window);
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

        for quad in prepainted.selection.drain(..) {
            window.paint_quad(quad);
        }
        for block in &layout.blocks {
            let _ = block.line.paint_background(block.origin, block.line_height, TextAlign::Left, None, window, cx);
            let _ = block.line.paint(block.origin, block.line_height, TextAlign::Left, None, window, cx);
            if let Some(marker) = &block.marker {
                let at = point(block.origin.x - px(22. * layout.scale), block.origin.y);
                let _ = marker.paint(at, block.line_height, TextAlign::Left, None, window, cx);
            }
        }
        if focus.is_focused(window)
            && self.editor.read(cx).caret_on
            && self.editor.read(cx).mode != crate::editor::Mode::Board
            && let Some(caret) = prepainted.caret.take()
        {
            window.paint_quad(caret);
        }
        self.editor.update(cx, |editor, _| editor.layout = Some(layout));
    }
}
