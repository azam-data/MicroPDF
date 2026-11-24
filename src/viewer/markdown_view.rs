//! CPU-rendered Styled Markdown (`.md` / `.markdown`) Reader Canvas.
//!
//! Zero GPU, zero GUI toolkit: renders directly into the `softbuffer` pixel slice
//! with viewport culling and a cached wrapped layout (only on the active tab).

use crate::markdown::{InlineSpan, MdAlign, MdBlock, MdListItem};
use std::collections::HashMap;

use super::render::fill_rect_clipped;
use super::state::*;
use super::text_editor::{MdEditorMode, TextEditorState};
use super::theme::{UiTheme, UiThemeMode};
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum MarkdownHit {
    ModePreview,
    ModeSplit,
    ModeEdit,
    Save,
    Reload,
    ZoomOut,
    ZoomReset,
    ZoomIn,
    ExportMenu,
    CopyCode(usize),
    CodeScrollLeft(usize),
    CodeScrollRight(usize),
    Link(usize),
}

#[derive(Clone)]
pub(super) struct MdHitRect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: MarkdownHit,
}

impl MdHitRect {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

#[derive(Clone)]
struct StyledWord {
    text: String,
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    math: bool,
    link_idx: Option<usize>,
    w: f32,
}

#[derive(Clone)]
struct WrappedLine {
    spans: Vec<StyledWord>,
    total_w: f32,
}

#[derive(Clone)]
enum LaidOutBlock {
    Heading {
        y: f32,
        h: f32,
        level: u8,
        lines: Vec<WrappedLine>,
        line_h: f32,
        font_sz: f32,
    },
    Paragraph {
        y: f32,
        h: f32,
        lines: Vec<WrappedLine>,
        line_h: f32,
        font_sz: f32,
    },
    CodeBlock {
        y: f32,
        h: f32,
        code_idx: usize,
        lang: Option<String>,
        lines: Vec<String>,
        line_h: f32,
        font_sz: f32,
        max_line_w: f32,
    },
    Blockquote {
        y: f32,
        h: f32,
        inner: Vec<LaidOutBlock>,
    },
    Table {
        y: f32,
        h: f32,
        col_widths: Vec<f32>,
        alignments: Vec<MdAlign>,
        headers: Vec<Vec<WrappedLine>>,
        header_h: f32,
        rows: Vec<(Vec<Vec<WrappedLine>>, f32)>,
        line_h: f32,
        font_sz: f32,
    },
    ListItem {
        y: f32,
        h: f32,
        indent: f32,
        marker: ListMarker,
        lines: Vec<WrappedLine>,
        line_h: f32,
        font_sz: f32,
    },
    HorizontalRule {
        y: f32,
        h: f32,
    },
    MathBlock {
        y: f32,
        h: f32,
        tex: String,
        font_sz: f32,
    },
    Image {
        y: f32,
        h: f32,
        img_idx: usize,
        w_px: u32,
        h_px: u32,
        caption: Option<String>,
        fallback_path: String,
        font_sz: f32,
    },
}

#[derive(Clone)]
enum ListMarker {
    Bullet(usize),
    Ordered(String),
    Task(bool),
}

pub(super) struct MdLayoutCache {
    pub(super) col_w: u32,
    pub(super) zoom_bits: u32,
    pub(super) sf_bits: u64,
    pub(super) doc_gen: u64,
    pub(super) total_h: f32,
    blocks: Vec<LaidOutBlock>,
    pub(super) block_y_by_ast_idx: HashMap<usize, f32>,
    pub(super) links: Vec<String>,
    pub(super) code_blocks: Vec<String>,
    pub(super) code_h_scroll: Vec<f32>,
    pub(super) images: Vec<Option<(u32, u32, Vec<u32>)>>,
}

fn tokenize_spans(spans: &[InlineSpan], font_sz: f32, links: &mut Vec<String>) -> Vec<StyledWord> {
    let mut out = Vec::new();
    for sp in spans {
        let (txt, bold, italic, strike, code, math, link_idx) = match sp {
            InlineSpan::Text(t) => (t.as_str(), false, false, false, false, false, None),
            InlineSpan::Bold(t) => (t.as_str(), true, false, false, false, false, None),
            InlineSpan::Italic(t) => (t.as_str(), false, true, false, false, false, None),
            InlineSpan::BoldItalic(t) => (t.as_str(), true, true, false, false, false, None),
            InlineSpan::Strikethrough(t) => (t.as_str(), false, false, true, false, false, None),
            InlineSpan::Code(t) => (t.as_str(), false, false, false, true, false, None),
            InlineSpan::Math(t) => (t.as_str(), false, false, false, false, true, None),
            InlineSpan::Link { text, url } => {
                let idx = links.len();
                links.push(url.clone());
                (text.as_str(), false, false, false, false, false, Some(idx))
            }
        };
        if math {
            let nodes = crate::math_render::parse_math_nodes(txt);
            let (mw, _, _) = crate::math_render::measure_math_nodes(&nodes, font_sz);
            out.push(StyledWord {
                text: txt.to_string(),
                bold: false,
                italic: false,
                strike: false,
                code: false,
                math: true,
                link_idx,
                w: mw + font_sz * 0.15,
            });
            continue;
        }
        if code {
            let w = measure_text(txt, font_sz * 0.92, false) + font_sz * 0.65;
            out.push(StyledWord {
                text: txt.to_string(),
                bold: false,
                italic: false,
                strike: false,
                code: true,
                math: false,
                link_idx,
                w,
            });
            continue;
        }
        let mut cur_start = 0usize;
        let bytes = txt.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i].is_ascii_whitespace() {
                if i > cur_start {
                    let word = &txt[cur_start..i];
                    let w = measure_text(word, font_sz, bold);
                    out.push(StyledWord {
                        text: word.to_string(),
                        bold,
                        italic,
                        strike,
                        code: false,
                        math: false,
                        link_idx,
                        w,
                    });
                }
                let sp_start = i;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let space_str = if txt[sp_start..i].contains('\n') { "\n" } else { " " };
                let w = if space_str == "\n" {
                    0.0
                } else {
                    measure_text(" ", font_sz, bold)
                };
                out.push(StyledWord {
                    text: space_str.to_string(),
                    bold,
                    italic,
                    strike,
                    code: false,
                    math: false,
                    link_idx,
                    w,
                });
                cur_start = i;
            } else {
                i += 1;
            }
        }
        if cur_start < bytes.len() {
            let word = &txt[cur_start..];
            let w = measure_text(word, font_sz, bold);
            out.push(StyledWord {
                text: word.to_string(),
                bold,
                italic,
                strike,
                code: false,
                math: false,
                link_idx,
                w,
            });
        }
    }
    out
}

fn wrap_styled_words(words: Vec<StyledWord>, max_w: f32) -> Vec<WrappedLine> {
    let mut lines = Vec::new();
    let mut cur: Vec<StyledWord> = Vec::new();
    let mut cur_w = 0.0f32;
    let limit = max_w.max(60.0);

    for word in words {
        if word.text == "\n" {
            lines.push(WrappedLine {
                spans: std::mem::take(&mut cur),
                total_w: cur_w,
            });
            cur_w = 0.0;
            continue;
        }
        let is_space = word.text == " ";
        if is_space && cur.is_empty() {
            continue;
        }
        if !cur.is_empty() && !is_space && cur_w + word.w > limit {
            while cur.last().is_some_and(|w| w.text == " ") {
                if let Some(popped) = cur.pop() {
                    cur_w -= popped.w;
                }
            }
            lines.push(WrappedLine {
                spans: std::mem::take(&mut cur),
                total_w: cur_w.max(0.0),
            });
            cur_w = 0.0;
        }
        cur_w += word.w;
        cur.push(word);
    }
    while cur.last().is_some_and(|w| w.text == " ") {
        if let Some(popped) = cur.pop() {
            cur_w -= popped.w;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(WrappedLine {
            spans: cur,
            total_w: cur_w.max(0.0),
        });
    }
    lines
}

#[allow(clippy::too_many_arguments)]
fn layout_blocks(
    ast_blocks: &[MdBlock],
    col_w: f32,
    base_fs: f32,
    s: f32,
    mut y: f32,
    links: &mut Vec<String>,
    code_blocks: &mut Vec<String>,
    images: &mut Vec<Option<(u32, u32, Vec<u32>)>>,
    block_y_by_ast_idx: Option<&mut HashMap<usize, f32>>,
) -> (Vec<LaidOutBlock>, f32) {
    let mut out = Vec::new();
    let mut y_map = block_y_by_ast_idx;

    for (idx, blk) in ast_blocks.iter().enumerate() {
        if let Some(ref mut map) = y_map {
            map.insert(idx, y);
        }
        match blk {
            MdBlock::Heading { level, spans } => {
                let scale = match level {
                    1 => 1.85,
                    2 => 1.48,
                    3 => 1.24,
                    4 => 1.08,
                    5 => 0.98,
                    _ => 0.92,
                };
                let font_sz = base_fs * scale;
                let line_h = font_sz * 1.32;
                let mut words = tokenize_spans(spans, font_sz, links);
                for w in &mut words {
                    w.bold = true;
                    w.w = measure_text(&w.text, font_sz, true);
                }
                let lines = wrap_styled_words(words, col_w);
                let div_extra = if *level <= 2 { 10.0 * s } else { 0.0 };
                let h = lines.len() as f32 * line_h + div_extra;
                let top_pad = if idx == 0 { 0.0 } else { 10.0 * s };
                let by = y + top_pad;
                if let Some(ref mut map) = y_map {
                    map.insert(idx, by);
                }
                out.push(LaidOutBlock::Heading {
                    y: by,
                    h,
                    level: *level,
                    lines,
                    line_h,
                    font_sz,
                });
                y = by + h + 10.0 * s;
            }
            MdBlock::Paragraph(spans) => {
                let font_sz = base_fs;
                let line_h = font_sz * 1.56;
                let words = tokenize_spans(spans, font_sz, links);
                let lines = wrap_styled_words(words, col_w);
                let h = lines.len() as f32 * line_h;
                out.push(LaidOutBlock::Paragraph {
                    y,
                    h,
                    lines,
                    line_h,
                    font_sz,
                });
                y += h + 11.0 * s;
            }
            MdBlock::CodeBlock { lang, code } => {
                let font_sz = (base_fs * 0.88).max(10.0);
                let line_h = font_sz * 1.48;
                let code_idx = code_blocks.len();
                code_blocks.push(code.clone());
                let lines: Vec<String> = if code.is_empty() {
                    vec![String::new()]
                } else {
                    code.lines().map(|l| l.to_string()).collect()
                };
                let max_line_w = lines
                    .iter()
                    .map(|l| measure_text(l, font_sz, false))
                    .fold(0.0f32, f32::max);
                let hdr_h = 26.0 * s;
                let pad_y = 10.0 * s;
                let h = hdr_h + pad_y * 2.0 + lines.len() as f32 * line_h;
                out.push(LaidOutBlock::CodeBlock {
                    y,
                    h,
                    code_idx,
                    lang: lang.clone(),
                    lines,
                    line_h,
                    font_sz,
                    max_line_w,
                });
                y += h + 12.0 * s;
            }
            MdBlock::Blockquote(inner_ast) => {
                let inner_w = (col_w - 28.0 * s).max(80.0);
                let (inner, end_y) = layout_blocks(
                    inner_ast,
                    inner_w,
                    base_fs * 0.97,
                    s,
                    y + 8.0 * s,
                    links,
                    code_blocks,
                    images,
                    None,
                );
                let h = (end_y - y).max(24.0 * s);
                out.push(LaidOutBlock::Blockquote { y, h, inner });
                y += h + 12.0 * s;
            }
            MdBlock::Table {
                headers,
                alignments,
                rows,
            } => {
                let ncols = headers.len().max(1);
                let col_w_each = (col_w / ncols as f32).max(60.0);
                let col_widths = vec![col_w_each; ncols];
                let font_sz = (base_fs * 0.92).max(10.5);
                let line_h = font_sz * 1.42;
                let cell_pad_x = 10.0 * s;
                let cell_pad_y = 7.0 * s;

                let mut hdr_wrapped = Vec::with_capacity(ncols);
                let mut max_hdr_lines = 1usize;
                for c in 0..ncols {
                    let spans = headers.get(c).map(|v| v.as_slice()).unwrap_or(&[]);
                    let mut words = tokenize_spans(spans, font_sz, links);
                    for w in &mut words {
                        w.bold = true;
                        w.w = measure_text(&w.text, font_sz, true);
                    }
                    let lines = wrap_styled_words(words, (col_widths[c] - cell_pad_x * 2.0).max(30.0));
                    max_hdr_lines = max_hdr_lines.max(lines.len());
                    hdr_wrapped.push(lines);
                }
                let header_h = max_hdr_lines as f32 * line_h + cell_pad_y * 2.0;

                let mut laid_rows = Vec::with_capacity(rows.len());
                let mut total_rows_h = 0.0f32;
                for row in rows {
                    let mut cells = Vec::with_capacity(ncols);
                    let mut max_lines = 1usize;
                    for c in 0..ncols {
                        let spans = row.get(c).map(|v| v.as_slice()).unwrap_or(&[]);
                        let words = tokenize_spans(spans, font_sz, links);
                        let lines = wrap_styled_words(
                            words,
                            (col_widths[c] - cell_pad_x * 2.0).max(30.0),
                        );
                        max_lines = max_lines.max(lines.len());
                        cells.push(lines);
                    }
                    let rh = max_lines as f32 * line_h + cell_pad_y * 2.0;
                    total_rows_h += rh;
                    laid_rows.push((cells, rh));
                }

                let h = header_h + total_rows_h;
                out.push(LaidOutBlock::Table {
                    y,
                    h,
                    col_widths,
                    alignments: alignments.clone(),
                    headers: hdr_wrapped,
                    header_h,
                    rows: laid_rows,
                    line_h,
                    font_sz,
                });
                y += h + 14.0 * s;
            }
            MdBlock::List {
                ordered,
                start,
                items,
            } => {
                y = layout_list_items(
                    items,
                    *ordered,
                    *start,
                    0,
                    col_w,
                    base_fs,
                    s,
                    y,
                    links,
                    &mut out,
                );
                y += 6.0 * s;
            }
            MdBlock::HorizontalRule => {
                let h = 18.0 * s;
                out.push(LaidOutBlock::HorizontalRule { y, h });
                y += h + 8.0 * s;
            }
            MdBlock::MathBlock(tex) => {
                let fs = base_fs * 1.15;
                let nodes = crate::math_render::parse_math_nodes(tex);
                let (_, asc, desc) = crate::math_render::measure_math_nodes(&nodes, fs);
                let h = (asc + desc + 22.0 * s).max(36.0 * s);
                out.push(LaidOutBlock::MathBlock {
                    y,
                    h,
                    tex: tex.clone(),
                    font_sz: fs,
                });
                y += h + 10.0 * s;
            }
            MdBlock::Image { path, caption } => {
                let img_idx = images.len();
                let decoded = image::open(path).ok().map(|im| {
                    let rgb = im.to_rgb8();
                    let (w, h) = rgb.dimensions();
                    let mut px = Vec::with_capacity((w * h) as usize);
                    for p in rgb.pixels() {
                        let [r, g, b] = p.0;
                        px.push(((r as u32) << 16) | ((g as u32) << 8) | (b as u32));
                    }
                    (w, h, px)
                });
                let (w_px, h_px) = if let Some((iw, ih, _)) = &decoded {
                    let nat_w = (*iw as f32) * s;
                    let nat_h = (*ih as f32) * s;
                    let mut dw = nat_w.min(col_w);
                    let mut dh = if nat_w > 0.0 { nat_h * (dw / nat_w) } else { 44.0 * s };
                    let max_h = 520.0 * s;
                    if dh > max_h && dh > 0.0 {
                        dw *= max_h / dh;
                        dh = max_h;
                    }
                    (dw.round().max(1.0) as u32, dh.round().max(1.0) as u32)
                } else {
                    (col_w.min(420.0 * s).round().max(120.0) as u32, (44.0 * s).round() as u32)
                };
                images.push(decoded);
                let cap_h = if caption.is_some() { 22.0 * s } else { 0.0 };
                let h = h_px as f32 + cap_h + 8.0 * s;
                out.push(LaidOutBlock::Image {
                    y,
                    h,
                    img_idx,
                    w_px,
                    h_px,
                    caption: caption.clone(),
                    fallback_path: path.clone(),
                    font_sz: base_fs * 0.88,
                });
                y += h + 10.0 * s;
            }
        }
    }

    (out, y)
}

#[allow(clippy::too_many_arguments)]
fn layout_list_items(
    items: &[MdListItem],
    ordered: bool,
    start: u64,
    depth: usize,
    col_w: f32,
    base_fs: f32,
    s: f32,
    mut y: f32,
    links: &mut Vec<String>,
    out: &mut Vec<LaidOutBlock>,
) -> f32 {
    let indent = (depth as f32 + 1.0) * (22.0 * s);
    let line_h = base_fs * 1.52;
    let avail = (col_w - indent).max(80.0);

    for (i, item) in items.iter().enumerate() {
        let marker = if let Some(checked) = item.task_checked {
            ListMarker::Task(checked)
        } else if ordered {
            ListMarker::Ordered(format!("{}.", start + i as u64))
        } else {
            ListMarker::Bullet(depth)
        };
        let words = tokenize_spans(&item.spans, base_fs, links);
        let lines = wrap_styled_words(words, avail);
        let h = lines.len() as f32 * line_h;
        out.push(LaidOutBlock::ListItem {
            y,
            h,
            indent,
            marker,
            lines,
            line_h,
            font_sz: base_fs,
        });
        y += h + 4.0 * s;

        for sub in &item.sublists {
            if let MdBlock::List {
                ordered: sub_ord,
                start: sub_start,
                items: sub_items,
            } = sub
            {
                y = layout_list_items(
                    sub_items,
                    *sub_ord,
                    *sub_start,
                    depth + 1,
                    col_w,
                    base_fs,
                    s,
                    y,
                    links,
                    out,
                );
            }
        }
    }
    y
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn md_header_bar_height_px(&self, sf: f64) -> u32 {
        (36.0 * sf as f32).round() as u32
    }

    pub(super) fn ensure_md_layout(&mut self, canvas_w: u32, sf: f64) {
        let Some(ref doc) = self.markdown_doc else {
            self.md_layout = None;
            return;
        };
        let s = sf as f32;
        let pad_x = (36.0 * s).round() as u32;
        let full_w = canvas_w.saturating_sub(pad_x * 2).max(200);
        let col_w = if self.view_mode == ViewMode::FitPage {
            full_w.min((780.0 * s).round() as u32).max(200)
        } else {
            full_w
        };
        let zoom_bits = self.zoom_level.to_bits();
        let sf_bits = sf.to_bits();
        let doc_gen = self.md_doc_gen;

        if let Some(ref cache) = self.md_layout {
            if cache.col_w == col_w
                && cache.zoom_bits == zoom_bits
                && cache.sf_bits == sf_bits
                && cache.doc_gen == doc_gen
            {
                return;
            }
        }

        let base_fs = (14.5 * s * self.zoom_level).clamp(9.0, 34.0);
        let mut links = Vec::new();
        let mut code_blocks = Vec::new();
        let mut images = Vec::new();
        let mut block_y_by_ast_idx = HashMap::new();
        let (blocks, end_y) = layout_blocks(
            &doc.blocks,
            col_w as f32,
            base_fs,
            s,
            24.0 * s,
            &mut links,
            &mut code_blocks,
            &mut images,
            Some(&mut block_y_by_ast_idx),
        );
        let code_h_scroll = vec![0.0f32; code_blocks.len()];
        self.md_layout = Some(MdLayoutCache {
            col_w,
            zoom_bits,
            sf_bits,
            doc_gen,
            total_h: end_y + 48.0 * s,
            blocks,
            block_y_by_ast_idx,
            links,
            code_blocks,
            code_h_scroll,
            images,
        });
    }

    pub(super) fn md_max_scroll_y(&self, canvas_h: u32, sf: f64) -> f32 {
        let hdr_h = self.md_header_bar_height_px(sf) as f32;
        let view_h = (canvas_h as f32 - hdr_h).max(60.0);
        self.md_layout
            .as_ref()
            .map(|c| (c.total_h - view_h).max(0.0))
            .unwrap_or(0.0)
    }

    pub(super) fn scroll_to_md_heading(&mut self, ast_block_idx: usize) {
        if let Some(ref cache) = self.md_layout {
            if let Some(&y) = cache.block_y_by_ast_idx.get(&ast_block_idx) {
                self.scroll_y = (y - 16.0).max(0.0);
                self.request_redraw();
            }
        }
    }

    pub(super) fn md_editor_rect(
        &self,
        sidebar_w: u32,
        top_bars_h: u32,
        win_w: u32,
        win_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32)> {
        if !self.is_markdown_tab() || self.md_editor_mode == MdEditorMode::Preview {
            return None;
        }
        let canvas_w = win_w.saturating_sub(sidebar_w);
        let canvas_h = win_h.saturating_sub(top_bars_h + status_bar_h);
        let hdr_h = self.md_header_bar_height_px(sf);
        let content_h = canvas_h.saturating_sub(hdr_h);
        if canvas_w == 0 || content_h == 0 {
            return None;
        }
        let ex = sidebar_w as i32;
        let ey = (top_bars_h + hdr_h) as i32;
        let ew = match self.md_editor_mode {
            MdEditorMode::Edit => canvas_w,
            MdEditorMode::Split => canvas_w / 2,
            MdEditorMode::Preview => return None,
        };
        Some((ex, ey, ew, content_h))
    }

    pub(super) fn ensure_md_editor_initialized(&mut self) {
        if self.md_editor.is_none() {
            let src = self.md_source.as_deref().unwrap_or("");
            self.md_editor = Some(TextEditorState::from_text(src));
        }
    }

    fn collect_md_header_hits(
        &self,
        sidebar_w: u32,
        top_bars_h: u32,
        win_w: u32,
        sf: f64,
    ) -> Vec<MdHitRect> {
        let s = sf as f32;
        let hdr_h = self.md_header_bar_height_px(sf);
        let x0 = sidebar_w as i32;
        let y0 = top_bars_h as i32;
        let cw = win_w.saturating_sub(sidebar_w);
        let btn_h = (24.0 * s).round() as u32;
        let by = y0 + ((hdr_h.saturating_sub(btn_h)) / 2) as i32;
        let mut rx = x0 + cw as i32 - (12.0 * s).round() as i32;

        let mut hits = Vec::with_capacity(12);
        let export_w = (108.0 * s).round() as u32;
        rx -= export_w as i32;
        hits.push(MdHitRect { x: rx, y: by, w: export_w, h: btn_h, hit: MarkdownHit::ExportMenu });

        rx -= (6.0 * s).round() as i32;
        let reload_w = (60.0 * s).round() as u32;
        rx -= reload_w as i32;
        hits.push(MdHitRect { x: rx, y: by, w: reload_w, h: btn_h, hit: MarkdownHit::Reload });

        if self.has_unsaved_edits || self.md_editor_mode != MdEditorMode::Preview {
            rx -= (6.0 * s).round() as i32;
            let save_w = (54.0 * s).round() as u32;
            rx -= save_w as i32;
            hits.push(MdHitRect { x: rx, y: by, w: save_w, h: btn_h, hit: MarkdownHit::Save });
        }

        rx -= (8.0 * s).round() as i32;
        let zbtn_w = (22.0 * s).round() as u32;
        let zlbl_w = (44.0 * s).round() as u32;
        rx -= zbtn_w as i32;
        hits.push(MdHitRect { x: rx, y: by, w: zbtn_w, h: btn_h, hit: MarkdownHit::ZoomIn });
        rx -= zlbl_w as i32;
        hits.push(MdHitRect { x: rx, y: by, w: zlbl_w, h: btn_h, hit: MarkdownHit::ZoomReset });
        rx -= zbtn_w as i32;
        hits.push(MdHitRect { x: rx, y: by, w: zbtn_w, h: btn_h, hit: MarkdownHit::ZoomOut });

        rx -= (10.0 * s).round() as i32;
        let mode_w = (54.0 * s).round() as u32;
        let split_w = (44.0 * s).round() as u32;
        let edit_w = (42.0 * s).round() as u32;
        rx -= edit_w as i32;
        hits.push(MdHitRect { x: rx, y: by, w: edit_w, h: btn_h, hit: MarkdownHit::ModeEdit });
        rx -= (split_w as i32) + 2;
        hits.push(MdHitRect { x: rx, y: by, w: split_w, h: btn_h, hit: MarkdownHit::ModeSplit });
        rx -= (mode_w as i32) + 2;
        hits.push(MdHitRect { x: rx, y: by, w: mode_w, h: btn_h, hit: MarkdownHit::ModePreview });

        hits
    }

    pub(super) fn draw_markdown_view(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        let canvas_w = buf_w.saturating_sub(sidebar_w);
        let canvas_h = buf_h.saturating_sub(top_bars_h + status_bar_h);
        if canvas_w == 0 || canvas_h == 0 {
            return;
        }
        let preview_w = match self.md_editor_mode {
            MdEditorMode::Preview => canvas_w,
            MdEditorMode::Split => canvas_w.saturating_sub(canvas_w / 2 + 1).max(160),
            MdEditorMode::Edit => 0,
        };
        if preview_w > 0 {
            self.ensure_md_layout(preview_w, sf);
        }

        let theme = self.ui_theme();
        let s = sf as f32;
        let hdr_h = self.md_header_bar_height_px(sf);
        let x0 = sidebar_w as i32;
        let y0 = top_bars_h as i32;

        // Apply active ReadingFilter (Normal, E-Ink, Sepia, Sage, Dark, Night Amber) to the Markdown canvas
        let (page_bg, content_theme) = md_reading_theme(theme, self.reading_filter, self.ui_theme_mode);
        fill_rect_clipped(buffer, buf_w, buf_h, x0, y0, canvas_w, canvas_h, page_bg);

        let content_top = y0 + hdr_h as i32;
        let content_bottom = y0 + canvas_h as i32;

        self.md_active_hits.clear();

        if preview_w > 0 {
            let max_scroll = self.md_max_scroll_y(canvas_h, sf);
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
            let scroll_y = self.scroll_y;
            let preview_x0 = match self.md_editor_mode {
                MdEditorMode::Split => x0 + (canvas_w / 2) as i32 + 1,
                _ => x0,
            };

            if let Some(ref cache) = self.md_layout {
                let col_x = preview_x0 + ((preview_w.saturating_sub(cache.col_w)) / 2) as i32;
                let view_top = scroll_y;
                let view_bot = scroll_y + (content_bottom - content_top) as f32;

                for blk in &cache.blocks {
                    draw_laid_out_block(
                        buffer,
                        buf_w,
                        buf_h,
                        blk,
                        col_x,
                        cache.col_w as f32,
                        content_top,
                        content_bottom,
                        scroll_y,
                        view_top,
                        view_bot,
                        &cache.code_h_scroll,
                        &cache.images,
                        &content_theme,
                        &self.hovered_widget,
                        s,
                        &mut self.md_active_hits,
                    );
                }
            }
        }

        if let Some((ex, ey, ew, eh)) = self.md_editor_rect(sidebar_w, top_bars_h, buf_w, buf_h, status_bar_h, sf) {
            self.ensure_md_editor_initialized();
            if let Some(ref mut ed) = self.md_editor {
                ed.draw(buffer, buf_w, buf_h, ex, ey, ew, eh, page_bg, &content_theme, sf);
            }
            if self.md_editor_mode == MdEditorMode::Split {
                fill_rect_clipped(buffer, buf_w, buf_h, ex + ew as i32, ey, 1, eh, theme.divider);
            }
        }

        // Top inline Markdown bar (drawn after scrolling content so it cleanly clips above)
        fill_rect_clipped(buffer, buf_w, buf_h, x0, y0, canvas_w, hdr_h, theme.surface_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, x0, y0 + hdr_h as i32 - 1, canvas_w, 1, theme.divider);

        let hdr_hits = self.collect_md_header_hits(sidebar_w, top_bars_h, buf_w, sf);
        let min_btn_x = hdr_hits.iter().map(|h| h.x).min().unwrap_or(x0 + canvas_w as i32);

        let title = self
            .markdown_doc
            .as_ref()
            .map(|d| d.title.as_str())
            .unwrap_or("Markdown");
        let badge_w = (36.0 * s).round() as u32;
        let badge_h = (18.0 * s).round() as u32;
        let badge_y = y0 + ((hdr_h.saturating_sub(badge_h)) / 2) as i32;
        fill_rounded_rect(buffer, buf_w, buf_h, x0 + 12, badge_y, badge_w, badge_h, 4.0 * s, theme.accent_soft_bg);
        let badge_label = if Self::is_latex_path(&self.file_path) {
            "TEX"
        } else {
            "MD"
        };
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            x0 + 18,
            badge_y,
            badge_h,
            x0 + 12 + badge_w as i32,
            badge_label,
            10.5 * s,
            theme.accent_soft_fg,
            true,
        );
        let title_x = x0 + 12 + badge_w as i32 + 8;
        let title_max_w = (min_btn_x - title_x - 10).max(20) as f32;
        let clipped_title = truncate_ellipsis(title, title_max_w, 12.0 * s, true);
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            title_x,
            y0,
            hdr_h,
            min_btn_x - 6,
            &clipped_title,
            12.0 * s,
            theme.text_primary,
            true,
        );

        let zoom_pct = format!("{}%", (self.zoom_level * 100.0).round() as i32);
        for h in &hdr_hits {
            let hov = self.hovered_widget == Some(UiHit::MarkdownView(h.hit));
            let is_primary = h.hit == MarkdownHit::ExportMenu
                || (h.hit == MarkdownHit::Save && self.has_unsaved_edits);
            let is_active = match h.hit {
                MarkdownHit::ModePreview => self.md_editor_mode == MdEditorMode::Preview,
                MarkdownHit::ModeSplit => self.md_editor_mode == MdEditorMode::Split,
                MarkdownHit::ModeEdit => self.md_editor_mode == MdEditorMode::Edit,
                _ => false,
            };
            let bg = if is_primary {
                if hov { theme.accent_hover } else { theme.accent }
            } else if is_active {
                theme.accent_soft_bg
            } else if hov {
                theme.hover_bg
            } else {
                theme.input_bg
            };
            let fg = if is_primary {
                theme.text_on_accent
            } else if is_active {
                theme.accent_soft_fg
            } else {
                theme.text_primary
            };
            fill_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 5.0 * s, bg);
            if !is_primary {
                stroke_rounded_rect(buffer, buf_w, buf_h, h.x, h.y, h.w, h.h, 5.0 * s, if is_active { theme.accent } else { theme.border_subtle });
            }
            let lbl = match h.hit {
                MarkdownHit::ModePreview => "Preview",
                MarkdownHit::ModeSplit => "Split",
                MarkdownHit::ModeEdit => "Edit",
                MarkdownHit::Save => "Save",
                MarkdownHit::ExportMenu => "Export / Print",
                MarkdownHit::Reload => "Reload",
                MarkdownHit::ZoomOut => "-",
                MarkdownHit::ZoomReset => zoom_pct.as_str(),
                MarkdownHit::ZoomIn => "+",
                _ => "",
            };
            let fs = 11.0 * s;
            let tw = measure_text(lbl, fs, is_primary || is_active);
            let tx = h.x + ((h.w as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, tx, h.y, h.h, h.x + h.w as i32 - 2, lbl, fs, fg, is_primary || is_active);
        }

        self.md_active_hits.extend(hdr_hits);
    }

    pub(super) fn hit_test_markdown_view(&self, mx: f64, my: f64) -> Option<MarkdownHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        // Reverse order so top bar hits take precedence over scrolled content
        for h in self.md_active_hits.iter().rev() {
            if h.contains(ix, iy) {
                return Some(h.hit);
            }
        }
        None
    }

    pub(super) fn handle_markdown_click(&mut self, hit: MarkdownHit) {
        match hit {
            MarkdownHit::ModePreview => {
                self.md_editor_mode = MdEditorMode::Preview;
                self.md_layout = None;
                self.request_redraw();
            }
            MarkdownHit::ModeSplit => {
                self.md_editor_mode = MdEditorMode::Split;
                self.ensure_md_editor_initialized();
                self.md_layout = None;
                self.request_redraw();
            }
            MarkdownHit::ModeEdit => {
                self.md_editor_mode = MdEditorMode::Edit;
                self.ensure_md_editor_initialized();
                self.request_redraw();
            }
            MarkdownHit::Save => {
                self.save_active_markdown();
            }
            MarkdownHit::Reload => {
                self.reload_active_markdown();
            }
            MarkdownHit::ZoomOut => {
                self.zoom_level = (self.zoom_level - 0.1).clamp(0.6, 2.2);
                self.update_title_bar();
                self.request_redraw();
            }
            MarkdownHit::ZoomReset => {
                self.zoom_level = 1.0;
                self.update_title_bar();
                self.request_redraw();
            }
            MarkdownHit::ZoomIn => {
                self.zoom_level = (self.zoom_level + 0.1).clamp(0.6, 2.2);
                self.update_title_bar();
                self.request_redraw();
            }
            MarkdownHit::ExportMenu => {
                self.open_markdown_export_modal();
            }
            MarkdownHit::CopyCode(idx) => {
                if let Some(ref cache) = self.md_layout {
                    if let Some(code) = cache.code_blocks.get(idx) {
                        match Self::copy_text_to_os_clipboard(code) {
                            Ok(()) => {
                                self.status_notice = Some("Copied code block to clipboard".to_string());
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("Copy failed: {e}"));
                            }
                        }
                        self.update_title_bar();
                        self.request_redraw();
                    }
                }
            }
            MarkdownHit::CodeScrollLeft(idx) => {
                if let Some(ref mut cache) = self.md_layout {
                    if let Some(off) = cache.code_h_scroll.get_mut(idx) {
                        *off = (*off - 80.0).max(0.0);
                    }
                }
                self.request_redraw();
            }
            MarkdownHit::CodeScrollRight(idx) => {
                if let Some(ref mut cache) = self.md_layout {
                    if let Some(off) = cache.code_h_scroll.get_mut(idx) {
                        *off += 80.0;
                    }
                }
                self.request_redraw();
            }
            MarkdownHit::Link(idx) => {
                if let Some(url) = self.md_layout.as_ref().and_then(|c| c.links.get(idx)).cloned() {
                    if let Err(e) = Self::open_external_url_in_browser(&url) {
                        self.status_notice = Some(format!("Could not open link: {e}"));
                        self.update_title_bar();
                        self.request_redraw();
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_laid_out_block(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    blk: &LaidOutBlock,
    col_x: i32,
    col_w: f32,
    clip_top: i32,
    clip_bot: i32,
    scroll_y: f32,
    view_top: f32,
    view_bot: f32,
    code_h_scroll: &[f32],
    images: &[Option<(u32, u32, Vec<u32>)>],
    theme: &UiTheme,
    hovered: &Option<UiHit>,
    s: f32,
    hits: &mut Vec<MdHitRect>,
) {
    match blk {
        LaidOutBlock::Heading { y, h, level, lines, line_h, font_sz } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            draw_wrapped_lines(buffer, buf_w, buf_h, lines, col_x, sy, *line_h, *font_sz, clip_top, clip_bot, theme.text_primary, theme, MdAlign::Left, col_w, hits);
            if *level <= 2 {
                let div_y = sy + (lines.len() as f32 * *line_h + 4.0 * s).round() as i32;
                if div_y >= clip_top && div_y < clip_bot {
                    fill_rect_clipped(buffer, buf_w, buf_h, col_x, div_y, col_w.round() as u32, 1, theme.divider);
                }
            }
        }
        LaidOutBlock::Paragraph { y, h, lines, line_h, font_sz } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            draw_wrapped_lines(buffer, buf_w, buf_h, lines, col_x, sy, *line_h, *font_sz, clip_top, clip_bot, theme.text_primary, theme, MdAlign::Left, col_w, hits);
        }
        LaidOutBlock::CodeBlock { y, h, code_idx, lang, lines, line_h, font_sz, max_line_w } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let cw = col_w.round() as u32;
            let ch = h.round() as u32;
            let hdr_h = (26.0 * s).round() as u32;
            let box_bg = theme.surface_bg;
            let hdr_bg = theme.sidebar_header_bg;
            let draw_y0 = sy.max(clip_top);
            let draw_y1 = (sy + ch as i32).min(clip_bot);
            if draw_y1 > draw_y0 {
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, draw_y0, cw, (draw_y1 - draw_y0) as u32, box_bg);
            }
            if sy >= clip_top && sy + hdr_h as i32 <= clip_bot {
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, sy, cw, hdr_h, hdr_bg);
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, sy + hdr_h as i32 - 1, cw, 1, theme.divider);
                let lang_lbl = lang.as_deref().unwrap_or("code");
                draw_text_vcenter(buffer, buf_w, buf_h, col_x + 12, sy, hdr_h, col_x + cw as i32 - 120, lang_lbl, 10.5 * s, theme.text_muted, true);

                let btn_h = (20.0 * s).round() as u32;
                let btn_y = sy + ((hdr_h.saturating_sub(btn_h)) / 2) as i32;
                let copy_w = (48.0 * s).round() as u32;
                let copy_x = col_x + cw as i32 - copy_w as i32 - 8;
                let copy_hit = MarkdownHit::CopyCode(*code_idx);
                let copy_hov = *hovered == Some(UiHit::MarkdownView(copy_hit));
                fill_rounded_rect(buffer, buf_w, buf_h, copy_x, btn_y, copy_w, btn_h, 4.0 * s, if copy_hov { theme.hover_bg } else { theme.input_bg });
                stroke_rounded_rect(buffer, buf_w, buf_h, copy_x, btn_y, copy_w, btn_h, 4.0 * s, theme.border_subtle);
                draw_text_vcenter(buffer, buf_w, buf_h, copy_x + 10, btn_y, btn_h, copy_x + copy_w as i32, "Copy", 10.0 * s, theme.text_primary, false);
                hits.push(MdHitRect { x: copy_x, y: btn_y, w: copy_w, h: btn_h, hit: copy_hit });

                let inner_w = (cw as f32 - 24.0 * s).max(40.0);
                if *max_line_w > inner_w {
                    let s_w = (22.0 * s).round() as u32;
                    let sr_x = copy_x - s_w as i32 - 6;
                    let sl_x = sr_x - s_w as i32 - 4;
                    let sl_hit = MarkdownHit::CodeScrollLeft(*code_idx);
                    let sr_hit = MarkdownHit::CodeScrollRight(*code_idx);
                    fill_rounded_rect(buffer, buf_w, buf_h, sl_x, btn_y, s_w, btn_h, 4.0 * s, theme.input_bg);
                    draw_text_vcenter(buffer, buf_w, buf_h, sl_x + 7, btn_y, btn_h, sl_x + s_w as i32, "<", 10.0 * s, theme.text_primary, false);
                    fill_rounded_rect(buffer, buf_w, buf_h, sr_x, btn_y, s_w, btn_h, 4.0 * s, theme.input_bg);
                    draw_text_vcenter(buffer, buf_w, buf_h, sr_x + 7, btn_y, btn_h, sr_x + s_w as i32, ">", 10.0 * s, theme.text_primary, false);
                    hits.push(MdHitRect { x: sl_x, y: btn_y, w: s_w, h: btn_h, hit: sl_hit });
                    hits.push(MdHitRect { x: sr_x, y: btn_y, w: s_w, h: btn_h, hit: sr_hit });
                }
            }
            let h_off = code_h_scroll.get(*code_idx).copied().unwrap_or(0.0);
            let max_off = (*max_line_w - (col_w - 24.0 * s)).max(0.0);
            let h_off = h_off.clamp(0.0, max_off);
            let text_x = col_x + (12.0 * s).round() as i32 - h_off.round() as i32;
            let text_y0 = sy + hdr_h as i32 + (10.0 * s).round() as i32;
            for (li, line) in lines.iter().enumerate() {
                let ly = text_y0 + (li as f32 * *line_h).round() as i32;
                if ly >= clip_top && ly + (*line_h).round() as i32 <= clip_bot {
                    draw_text_clipped(buffer, buf_w, buf_h, text_x.max(col_x + 8), ly, col_x + cw as i32 - 8, line, *font_sz, theme.text_primary, false);
                }
            }
        }
        LaidOutBlock::Blockquote { y, h, inner } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let ch = h.round() as u32;
            let draw_y0 = sy.max(clip_top);
            let draw_y1 = (sy + ch as i32).min(clip_bot);
            if draw_y1 > draw_y0 {
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, draw_y0, col_w.round() as u32, (draw_y1 - draw_y0) as u32, theme.surface_bg);
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, draw_y0, (3.0 * s).round().max(2.0) as u32, (draw_y1 - draw_y0) as u32, theme.accent);
            }
            for child in inner {
                draw_laid_out_block(buffer, buf_w, buf_h, child, col_x + (16.0 * s).round() as i32, col_w - 24.0 * s, clip_top, clip_bot, scroll_y, view_top, view_bot, code_h_scroll, images, theme, hovered, s, hits);
            }
        }
        LaidOutBlock::Table { y, h, col_widths, alignments, headers, header_h, rows, line_h, font_sz } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let tw = col_w.round() as u32;
            let hh = header_h.round() as u32;
            if sy + hh as i32 > clip_top && sy < clip_bot {
                let dy0 = sy.max(clip_top);
                let dy1 = (sy + hh as i32).min(clip_bot);
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, dy0, tw, (dy1 - dy0) as u32, theme.sidebar_header_bg);
            }
            let pad_x = 10.0 * s;
            let mut cx = col_x;
            for (c, cell_lines) in headers.iter().enumerate() {
                let cw = col_widths[c];
                let align = alignments.get(c).copied().unwrap_or(MdAlign::Left);
                draw_wrapped_lines(buffer, buf_w, buf_h, cell_lines, cx + pad_x.round() as i32, sy + (7.0 * s).round() as i32, *line_h, *font_sz, clip_top, clip_bot, theme.text_primary, theme, align, (cw - pad_x * 2.0).max(20.0), hits);
                cx += cw.round() as i32;
            }
            let mut ry = sy + hh as i32;
            for (ri, (row_cells, rh)) in rows.iter().enumerate() {
                let rih = rh.round() as u32;
                if ry + rih as i32 > clip_top && ry < clip_bot {
                    let dy0 = ry.max(clip_top);
                    let dy1 = (ry + rih as i32).min(clip_bot);
                    if ri % 2 == 1 {
                        fill_rect_clipped(buffer, buf_w, buf_h, col_x, dy0, tw, (dy1 - dy0) as u32, theme.surface_bg);
                    }
                    fill_rect_clipped(buffer, buf_w, buf_h, col_x, ry, tw, 1, theme.divider);
                    let mut rcx = col_x;
                    for (c, cell_lines) in row_cells.iter().enumerate() {
                        let cw = col_widths.get(c).copied().unwrap_or(80.0);
                        let align = alignments.get(c).copied().unwrap_or(MdAlign::Left);
                        draw_wrapped_lines(buffer, buf_w, buf_h, cell_lines, rcx + pad_x.round() as i32, ry + (7.0 * s).round() as i32, *line_h, *font_sz, clip_top, clip_bot, theme.text_primary, theme, align, (cw - pad_x * 2.0).max(20.0), hits);
                        rcx += cw.round() as i32;
                    }
                }
                ry += rih as i32;
            }
        }
        LaidOutBlock::ListItem { y, h, indent, marker, lines, line_h, font_sz } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let text_x = col_x + indent.round() as i32;
            if sy >= clip_top && sy + (*line_h).round() as i32 <= clip_bot {
                match marker {
                    ListMarker::Bullet(depth) => {
                        let mx = text_x - (14.0 * s).round() as i32;
                        let sym = if *depth % 2 == 0 { "•" } else { "◦" };
                        draw_text(buffer, buf_w, buf_h, mx, sy, sym, *font_sz, theme.accent, true);
                    }
                    ListMarker::Ordered(num) => {
                        let nw = measure_text(num, *font_sz, true);
                        let mx = text_x - nw.round() as i32 - (6.0 * s).round() as i32;
                        draw_text(buffer, buf_w, buf_h, mx, sy, num, *font_sz, theme.text_muted, true);
                    }
                    ListMarker::Task(checked) => {
                        let cb_sz = (12.0 * s).round() as u32;
                        let cx = text_x - cb_sz as i32 - (6.0 * s).round() as i32;
                        let cy = sy + (((*line_h - cb_sz as f32) * 0.4).round() as i32).max(1);
                        if *checked {
                            fill_rounded_rect(buffer, buf_w, buf_h, cx, cy, cb_sz, cb_sz, 3.0 * s, theme.accent);
                            draw_line_aa(buffer, buf_w, buf_h, cx as f32 + 2.5 * s, cy as f32 + 6.0 * s, cx as f32 + 5.0 * s, cy as f32 + 8.5 * s, 1.5 * s, theme.text_on_accent);
                            draw_line_aa(buffer, buf_w, buf_h, cx as f32 + 5.0 * s, cy as f32 + 8.5 * s, cx as f32 + 9.5 * s, cy as f32 + 3.5 * s, 1.5 * s, theme.text_on_accent);
                        } else {
                            stroke_rounded_rect(buffer, buf_w, buf_h, cx, cy, cb_sz, cb_sz, 3.0 * s, theme.border_subtle);
                        }
                    }
                }
            }
            draw_wrapped_lines(buffer, buf_w, buf_h, lines, text_x, sy, *line_h, *font_sz, clip_top, clip_bot, theme.text_primary, theme, MdAlign::Left, col_w, hits);
        }
        LaidOutBlock::HorizontalRule { y, h } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y + h * 0.5 - scroll_y).round() as i32;
            if sy >= clip_top && sy < clip_bot {
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, sy, col_w.round() as u32, 1, theme.divider);
            }
        }
        LaidOutBlock::MathBlock { y, h, tex, font_sz } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let box_h = h.round().max(1.0) as u32;
            let draw_y0 = sy.max(clip_top);
            let draw_y1 = (sy + box_h as i32).min(clip_bot);
            if draw_y1 > draw_y0 {
                fill_rounded_rect(buffer, buf_w, buf_h, col_x, draw_y0, col_w.round().max(1.0) as u32, (draw_y1 - draw_y0) as u32, 6.0 * s, theme.surface_bg);
            }
            let nodes = crate::math_render::parse_math_nodes(tex);
            let (mw, asc, _) = crate::math_render::measure_math_nodes(&nodes, *font_sz);
            let mx = col_x as f32 + ((col_w - mw) * 0.5).max(8.0 * s);
            let baseline_y = sy as f32 + 11.0 * s + asc;
            crate::math_render::draw_math_nodes(
                buffer,
                buf_w,
                buf_h,
                mx,
                baseline_y,
                &nodes,
                *font_sz,
                theme.text_primary,
                clip_top,
                clip_bot,
            );
        }
        LaidOutBlock::Image { y, h, img_idx, w_px, h_px, caption, fallback_path, font_sz } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let ix = col_x + ((col_w - *w_px as f32) * 0.5).max(0.0).round() as i32;
            if let Some(Some((sw, sh, src_px))) = images.get(*img_idx) {
                let dw = (*w_px).max(1);
                let dh = (*h_px).max(1);
                for dy in 0..dh {
                    let py = sy + dy as i32;
                    if py < clip_top || py >= clip_bot || py >= buf_h as i32 {
                        continue;
                    }
                    let src_y = ((dy as u64 * *sh as u64) / dh as u64).min(*sh as u64 - 1) as usize;
                    let row_off = py as usize * buf_w as usize;
                    let src_row = src_y * (*sw as usize);
                    for dx in 0..dw {
                        let px_x = ix + dx as i32;
                        if px_x < 0 || px_x >= buf_w as i32 {
                            continue;
                        }
                        let src_x = ((dx as u64 * *sw as u64) / dw as u64).min(*sw as u64 - 1) as usize;
                        buffer[row_off + px_x as usize] = src_px[src_row + src_x];
                    }
                }
            } else {
                let draw_y0 = sy.max(clip_top);
                let draw_y1 = (sy + *h_px as i32).min(clip_bot);
                if draw_y1 > draw_y0 {
                    fill_rounded_rect(buffer, buf_w, buf_h, ix, draw_y0, *w_px, (draw_y1 - draw_y0) as u32, 6.0 * s, theme.surface_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, ix, draw_y0, *w_px, (draw_y1 - draw_y0) as u32, 6.0 * s, theme.border_subtle);
                }
                if sy >= clip_top && sy + (*h_px as i32) <= clip_bot {
                    let msg = format!("[Image: {}]", fallback_path);
                    let clipped = truncate_ellipsis(&msg, (*w_px as f32 - 20.0 * s).max(40.0), *font_sz, false);
                    draw_text_vcenter(buffer, buf_w, buf_h, ix + (10.0 * s).round() as i32, sy, *h_px, ix + *w_px as i32 - 8, &clipped, *font_sz, theme.text_muted, false);
                }
            }
            if let Some(cap) = caption {
                let cap_y = sy + *h_px as i32 + (6.0 * s).round() as i32;
                if cap_y >= clip_top && cap_y + (*font_sz * 1.4).round() as i32 <= clip_bot {
                    let tw = measure_text(cap, *font_sz, false);
                    let tx = col_x + ((col_w - tw) * 0.5).max(0.0).round() as i32;
                    draw_text_clipped(buffer, buf_w, buf_h, tx, cap_y, col_x + col_w.round() as i32, cap, *font_sz, theme.text_secondary, false);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_wrapped_lines(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    lines: &[WrappedLine],
    x: i32,
    y: i32,
    line_h: f32,
    font_sz: f32,
    clip_top: i32,
    clip_bot: i32,
    base_color: u32,
    theme: &UiTheme,
    align: MdAlign,
    max_w: f32,
    hits: &mut Vec<MdHitRect>,
) {
    for (li, line) in lines.iter().enumerate() {
        let ly = y + (li as f32 * line_h).round() as i32;
        if ly < clip_top || ly + line_h.round() as i32 > clip_bot {
            continue;
        }
        let align_dx = match align {
            MdAlign::Center => ((max_w - line.total_w) * 0.5).max(0.0),
            MdAlign::Right => (max_w - line.total_w).max(0.0),
            MdAlign::Left | MdAlign::None => 0.0,
        };
        let mut cx = x as f32 + align_dx;
        for w in &line.spans {
            if w.math {
                let nodes = crate::math_render::parse_math_nodes(&w.text);
                crate::math_render::draw_math_nodes(
                    buffer,
                    buf_w,
                    buf_h,
                    cx,
                    ly as f32 + font_sz * 0.85,
                    &nodes,
                    font_sz,
                    base_color,
                    clip_top,
                    clip_bot,
                );
                cx += w.w;
                continue;
            }
            if w.code {
                let pw = w.w.round() as u32;
                let ph = (font_sz * 1.25).round() as u32;
                let py = ly - (font_sz * 0.08).round() as i32;
                if py >= clip_top && py + ph as i32 <= clip_bot {
                    fill_rounded_rect(buffer, buf_w, buf_h, cx.round() as i32, py, pw, ph, 3.5, theme.input_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, cx.round() as i32, py, pw, ph, 3.5, theme.border_subtle);
                }
                let pad = (font_sz * 0.32).round() as i32;
                draw_text_clipped(buffer, buf_w, buf_h, cx.round() as i32 + pad, ly, buf_w as i32, &w.text, font_sz * 0.92, theme.accent_soft_fg, false);
                cx += w.w;
                continue;
            }
            let col = if w.link_idx.is_some() {
                theme.accent
            } else if w.strike {
                theme.text_muted
            } else if w.italic {
                theme.text_secondary
            } else {
                base_color
            };
            draw_text_clipped(buffer, buf_w, buf_h, cx.round() as i32, ly, buf_w as i32, &w.text, font_sz, col, w.bold);
            if w.strike && w.text != " " {
                let sy_mid = ly + (font_sz * 0.56).round() as i32;
                if sy_mid >= clip_top && sy_mid < clip_bot {
                    fill_rect_clipped(buffer, buf_w, buf_h, cx.round() as i32, sy_mid, w.w.round().max(1.0) as u32, 1, theme.text_muted);
                }
            }
            if let Some(lidx) = w.link_idx {
                let uy = ly + (font_sz * 1.08).round() as i32;
                if uy >= clip_top && uy < clip_bot {
                    fill_rect_clipped(buffer, buf_w, buf_h, cx.round() as i32, uy, w.w.round().max(1.0) as u32, 1, theme.accent);
                }
                if ly >= clip_top && ly + line_h.round() as i32 <= clip_bot {
                    hits.push(MdHitRect {
                        x: cx.round() as i32,
                        y: ly,
                        w: w.w.round().max(4.0) as u32,
                        h: line_h.round().max(12.0) as u32,
                        hit: MarkdownHit::Link(lidx),
                    });
                }
            }
            cx += w.w;
        }
    }
}

fn md_reading_theme(base: UiTheme, filter: ReadingFilter, mode: UiThemeMode) -> (u32, UiTheme) {
    let mut t = base;
    match filter {
        ReadingFilter::Normal => {
            let bg = match mode {
                UiThemeMode::Dark => 0x001E_2128,
                UiThemeMode::Light => 0x00FB_FCFE,
            };
            (bg, t)
        }
        ReadingFilter::PaperEInk => {
            t.text_primary = 0x001C_1C1C;
            t.text_secondary = 0x003D_3D3D;
            t.text_muted = 0x0066_6666;
            t.surface_bg = 0x00E8_E7E1;
            t.sidebar_header_bg = 0x00DF_DED7;
            t.input_bg = 0x00E5_E4DE;
            t.border_subtle = 0x00C8_C7C0;
            t.divider = 0x00D0_CFC8;
            t.accent = 0x002B_59A2;
            t.accent_soft_fg = 0x002B_59A2;
            (0x00F2_F1EC, t)
        }
        ReadingFilter::WarmSepia => {
            t.text_primary = 0x003B_2B1A;
            t.text_secondary = 0x005A_442C;
            t.text_muted = 0x007E_6548;
            t.surface_bg = 0x00EC_DEC4;
            t.sidebar_header_bg = 0x00E3_D2B3;
            t.input_bg = 0x00E9_D9BC;
            t.border_subtle = 0x00CF_BC9A;
            t.divider = 0x00D8_C6A6;
            t.accent = 0x009A_4B16;
            t.accent_soft_fg = 0x008A_3F12;
            (0x00F6_ECD8, t)
        }
        ReadingFilter::SageEyeCare => {
            t.text_primary = 0x001E_2E24;
            t.text_secondary = 0x0036_4D3E;
            t.text_muted = 0x0056_7060;
            t.surface_bg = 0x00DA_E7DA;
            t.sidebar_header_bg = 0x00CE_DECE;
            t.input_bg = 0x00D5_E4D5;
            t.border_subtle = 0x00B4_C9B5;
            t.divider = 0x00C0_D4C1;
            t.accent = 0x001F_6F4A;
            t.accent_soft_fg = 0x001B_6140;
            (0x00E7_F0E7, t)
        }
        ReadingFilter::DarkInvert => {
            t.text_primary = 0x00DC_E1EA;
            t.text_secondary = 0x00B4_BDCC;
            t.text_muted = 0x0085_90A3;
            t.surface_bg = 0x0024_2832;
            t.sidebar_header_bg = 0x002C_313D;
            t.input_bg = 0x002A_2F3A;
            t.border_subtle = 0x003C_4352;
            t.divider = 0x0035_3B49;
            t.accent = 0x0060_A5FA;
            t.accent_soft_fg = 0x0093_C5FD;
            t.hover_bg = 0x0033_3947;
            (0x001B_1E24, t)
        }
        ReadingFilter::NightAmber => {
            t.text_primary = 0x00E8_C28B;
            t.text_secondary = 0x00C7_A06C;
            t.text_muted = 0x0099_784D;
            t.surface_bg = 0x0021_1A14;
            t.sidebar_header_bg = 0x002A_2119;
            t.input_bg = 0x0028_1F18;
            t.border_subtle = 0x0042_3427;
            t.divider = 0x003A_2D22;
            t.accent = 0x00F5_9E0B;
            t.accent_soft_fg = 0x00FB_BF24;
            t.hover_bg = 0x0035_291F;
            (0x0017_120E, t)
        }
    }
}
