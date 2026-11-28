//! CPU-rendered Styled Markdown (`.md` / `.markdown`) Reader Canvas.
//!
//! Zero GPU, zero GUI toolkit: renders directly into the `softbuffer` pixel slice
//! with viewport culling and a cached wrapped layout (only on the active tab).

use crate::markdown::{InlineSpan, MdAlign, MdBlock, MdListItem};
use std::collections::HashMap;

use super::book_ui::{EPUB_HIGHLIGHT_COLORS, EpubHighlight, EpubWordHit};
use super::render::fill_rect_clipped;
use super::state::*;
use super::text_editor::{MdEditorMode, TextEditorState};
use super::theme::{UiTheme, UiThemeMode};
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum MarkdownHit {
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
pub(super) struct StyledWord {
    pub(super) text: String,
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    math: bool,
    link_idx: Option<usize>,
    ast_block_idx: usize,
    word_idx: usize,
    w: f32,
}

#[derive(Clone)]
pub(super) struct WrappedLine {
    pub(super) spans: Vec<StyledWord>,
    pub(super) total_w: f32,
    pub(super) first_line_indent: f32,
    pub(super) is_last_in_para: bool,
}

#[derive(Clone)]
pub(super) enum LaidOutBlock {
    Heading {
        y: f32,
        h: f32,
        ast_block_idx: usize,
        level: u8,
        lines: Vec<WrappedLine>,
        line_h: f32,
        font_sz: f32,
        ornamental: bool,
    },
    Paragraph {
        y: f32,
        h: f32,
        ast_block_idx: usize,
        lines: Vec<WrappedLine>,
        line_h: f32,
        font_sz: f32,
        justify: bool,
    },
    CodeBlock {
        y: f32,
        h: f32,
        ast_block_idx: usize,
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
        ast_block_idx: usize,
        inner: Vec<LaidOutBlock>,
    },
    Table {
        y: f32,
        h: f32,
        ast_block_idx: usize,
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
        ast_block_idx: usize,
        indent: f32,
        marker: ListMarker,
        lines: Vec<WrappedLine>,
        line_h: f32,
        font_sz: f32,
        justify: bool,
    },
    HorizontalRule {
        y: f32,
        h: f32,
        ast_block_idx: usize,
    },
    PageBreak {
        y: f32,
        ast_block_idx: usize,
    },
    MathBlock {
        y: f32,
        h: f32,
        ast_block_idx: usize,
        tex: String,
        font_sz: f32,
    },
    Image {
        y: f32,
        h: f32,
        ast_block_idx: usize,
        w_px: u32,
        h_px: u32,
        is_cover: bool,
        caption: Option<String>,
        fallback_path: String,
        font_sz: f32,
    },
}

#[derive(Clone)]
pub(super) enum ListMarker {
    Bullet(usize),
    Ordered(String),
    Task(bool),
}

#[derive(Clone, Debug)]
pub(super) struct BookPageSlice {
    pub(super) block_idx: usize,
    pub(super) ast_block_idx: usize,
    pub(super) line_start: usize,
    pub(super) line_end: usize,
}

#[derive(Clone, Debug)]
pub(super) struct BookPage {
    pub(super) is_cover: bool,
    pub(super) slices: Vec<BookPageSlice>,
    pub(super) start_y: f32,
    pub(super) first_ast_block: usize,
}

#[derive(Clone, Copy)]
pub(super) struct BookLayoutOpts {
    pub(super) book_mode: bool,
    pub(super) font_family: FontFamily,
    pub(super) line_mult: f32,
    pub(super) word_spacing_px: f32,
    pub(super) para_mult: f32,
    pub(super) justify: bool,
    pub(super) hyphenate: bool,
    pub(super) hypher_lang: hypher::Lang,
}

impl Default for BookLayoutOpts {
    fn default() -> Self {
        Self {
            book_mode: false,
            font_family: FontFamily::UiSans,
            line_mult: 1.56,
            word_spacing_px: 0.0,
            para_mult: 1.0,
            justify: false,
            hyphenate: false,
            hypher_lang: hypher::Lang::English,
        }
    }
}

pub(super) struct MdLayoutCache {
    pub(super) col_w: u32,
    pub(super) zoom_bits: u32,
    pub(super) sf_bits: u64,
    pub(super) doc_gen: u64,
    pub(super) epub_sig: (bool, super::book_ui::EpubReaderPrefs, u32, u32, u32, u32),
    pub(super) book_mode: bool,
    pub(super) book_font_family: FontFamily,
    pub(super) total_h: f32,
    pub(super) word_count: usize,
    pub(super) cum_words_by_block: Vec<usize>,
    blocks: Vec<LaidOutBlock>,
    pub(super) block_y_by_ast_idx: HashMap<usize, f32>,
    pub(super) book_pages: Vec<BookPage>,
    pub(super) book_pages_h_bits: u32,
    pub(super) book_pages_spread_flag: bool,
    pub(super) links: Vec<String>,
    pub(super) code_blocks: Vec<String>,
    pub(super) code_h_scroll: Vec<f32>,
}

fn measure_word_for_opts(word: &str, font_sz: f32, bold: bool, italic: bool, code: bool, opts: BookLayoutOpts) -> f32 {
    if !opts.book_mode {
        return measure_text(word, font_sz, bold);
    }
    let family = if code { FontFamily::CodeMono } else { opts.font_family };
    measure_text_styled(
        word,
        TextStyle {
            family,
            size_px: if code { font_sz * 0.92 } else { font_sz },
            bold,
            italic,
        },
    )
}

fn hypher_lang_from_meta(lang: Option<&str>) -> hypher::Lang {
    let lower = lang.unwrap_or("").trim().to_ascii_lowercase();
    if lower.starts_with("fr") {
        hypher::Lang::French
    } else if lower.starts_with("de") {
        hypher::Lang::German
    } else if lower.starts_with("es") {
        hypher::Lang::Spanish
    } else {
        hypher::Lang::English
    }
}

pub(super) fn tokenize_spans(
    spans: &[InlineSpan],
    font_sz: f32,
    links: &mut Vec<String>,
    ast_block_idx: usize,
    opts: BookLayoutOpts,
) -> Vec<StyledWord> {
    let mut out = Vec::new();
    let mut next_word_idx = 0usize;
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
            let wi = next_word_idx;
            next_word_idx += 1;
            out.push(StyledWord {
                text: txt.to_string(),
                bold: false,
                italic: false,
                strike: false,
                code: false,
                math: true,
                link_idx,
                ast_block_idx,
                word_idx: wi,
                w: mw + font_sz * 0.15,
            });
            continue;
        }
        if code {
            let w = measure_word_for_opts(txt, font_sz * 0.92, false, false, true, opts) + font_sz * 0.65;
            let wi = next_word_idx;
            next_word_idx += 1;
            out.push(StyledWord {
                text: txt.to_string(),
                bold: false,
                italic: false,
                strike: false,
                code: true,
                math: false,
                link_idx,
                ast_block_idx,
                word_idx: wi,
                w,
            });
            continue;
        }
        // Compute space width once per span instead of re-measuring on every whitespace token
        let space_w = (measure_word_for_opts(" ", font_sz, bold, italic, false, opts) + opts.word_spacing_px).max(1.0);
        let mut cur_start = 0usize;
        let bytes = txt.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i].is_ascii_whitespace() {
                if i > cur_start {
                    let word = &txt[cur_start..i];
                    let w = measure_word_for_opts(word, font_sz, bold, italic, false, opts);
                    let wi = next_word_idx;
                    next_word_idx += 1;
                    out.push(StyledWord {
                        text: word.to_string(),
                        bold,
                        italic,
                        strike,
                        code: false,
                        math: false,
                        link_idx,
                        ast_block_idx,
                        word_idx: wi,
                        w,
                    });
                }
                let sp_start = i;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let is_nl = bytes[sp_start..i].contains(&b'\n');
                out.push(StyledWord {
                    text: if is_nl { "\n".to_string() } else { " ".to_string() },
                    bold,
                    italic,
                    strike,
                    code: false,
                    math: false,
                    link_idx,
                    ast_block_idx,
                    word_idx: usize::MAX,
                    w: if is_nl { 0.0 } else { space_w },
                });
                cur_start = i;
            } else {
                i += 1;
            }
        }
        if cur_start < bytes.len() {
            let word = &txt[cur_start..];
            let w = measure_word_for_opts(word, font_sz, bold, italic, false, opts);
            let wi = next_word_idx;
            next_word_idx += 1;
            out.push(StyledWord {
                text: word.to_string(),
                bold,
                italic,
                strike,
                code: false,
                math: false,
                link_idx,
                ast_block_idx,
                word_idx: wi,
                w,
            });
        }
    }
    out
}

fn try_hyphenate_word(
    word: &StyledWord,
    avail_w: f32,
    font_sz: f32,
    opts: BookLayoutOpts,
) -> Option<(StyledWord, StyledWord)> {
    if !opts.hyphenate || word.code || word.math || word.text.len() < 6 {
        return None;
    }
    let text = word.text.as_str();
    let (byte_start, byte_end) = if text.is_ascii() {
        let bytes = text.as_bytes();
        let mut s = 0usize;
        while s < bytes.len() && !bytes[s].is_ascii_alphabetic() {
            s += 1;
        }
        let mut e = bytes.len();
        while e > s && !bytes[e - 1].is_ascii_alphabetic() {
            e -= 1;
        }
        if e.saturating_sub(s) < 5 || !bytes[s..e].iter().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        (s, e)
    } else {
        let mut byte_start = text.len();
        let mut byte_end = 0usize;
        for (bi, ch) in text.char_indices() {
            if ch.is_alphabetic() {
                if byte_start == text.len() {
                    byte_start = bi;
                }
                byte_end = bi + ch.len_utf8();
            }
        }
        if byte_start >= byte_end {
            return None;
        }
        let core = &text[byte_start..byte_end];
        if core.chars().count() < 5 || !core.chars().all(|c| c.is_alphabetic()) {
            return None;
        }
        (byte_start, byte_end)
    };
    let lead = &text[..byte_start];
    let core = &text[byte_start..byte_end];
    let trail = &text[byte_end..];
    let mut break_offsets = [0usize; 16];
    let mut n_breaks = 0usize;
    let mut acc = 0usize;
    for syl in hypher::hyphenate(core, opts.hypher_lang) {
        acc += syl.len();
        if acc < core.len() && n_breaks < break_offsets.len() {
            break_offsets[n_breaks] = acc;
            n_breaks += 1;
        }
    }
    if n_breaks == 0 {
        return None;
    }
    let mut cand_buf = String::with_capacity(text.len() + 2);
    for &split_b in break_offsets[..n_breaks].iter().rev() {
        let left_core = &core[..split_b];
        let right_core = &core[split_b..];
        if left_core.chars().count() < 2 || right_core.chars().count() < 2 {
            continue;
        }
        cand_buf.clear();
        cand_buf.push_str(lead);
        cand_buf.push_str(left_core);
        cand_buf.push('-');
        let left_w = measure_word_for_opts(&cand_buf, font_sz, word.bold, word.italic, false, opts);
        if left_w <= avail_w {
            let right_txt = format!("{right_core}{trail}");
            let right_w = measure_word_for_opts(&right_txt, font_sz, word.bold, word.italic, false, opts);
            let mut left_wrd = word.clone();
            left_wrd.text = cand_buf;
            left_wrd.w = left_w;
            let mut right_wrd = word.clone();
            right_wrd.text = right_txt;
            right_wrd.w = right_w;
            return Some((left_wrd, right_wrd));
        }
    }
    None
}

pub(super) fn wrap_styled_words_ex(
    words: Vec<StyledWord>,
    max_w: f32,
    first_indent: f32,
    font_sz: f32,
    opts: BookLayoutOpts,
) -> Vec<WrappedLine> {
    let mut lines = Vec::new();
    let mut cur: Vec<StyledWord> = Vec::new();
    let mut cur_w = 0.0f32;
    let base_limit = max_w.max(60.0);
    let mut cur_indent = first_indent.max(0.0).min(base_limit * 0.3);

    for mut word in words {
        if word.text == "\n" {
            while cur.last().is_some_and(|w| w.text == " ") {
                if let Some(popped) = cur.pop() {
                    cur_w -= popped.w;
                }
            }
            lines.push(WrappedLine {
                spans: std::mem::take(&mut cur),
                total_w: cur_w.max(0.0),
                first_line_indent: cur_indent,
                is_last_in_para: true,
            });
            cur_w = 0.0;
            cur_indent = 0.0;
            continue;
        }
        let is_space = word.text == " ";
        if is_space && cur.is_empty() {
            continue;
        }
        let line_limit = (base_limit - cur_indent).max(40.0);
        if !cur.is_empty() && !is_space && cur_w + word.w > line_limit {
            if opts.book_mode && opts.hyphenate {
                let avail = line_limit - cur_w;
                if avail > font_sz * 1.4 {
                    if let Some((left_part, right_part)) = try_hyphenate_word(&word, avail, font_sz, opts) {
                        cur_w += left_part.w;
                        cur.push(left_part);
                        while cur.last().is_some_and(|w| w.text == " ") {
                            if let Some(popped) = cur.pop() {
                                cur_w -= popped.w;
                            }
                        }
                        lines.push(WrappedLine {
                            spans: std::mem::take(&mut cur),
                            total_w: cur_w.max(0.0),
                            first_line_indent: cur_indent,
                            is_last_in_para: false,
                        });
                        cur_w = 0.0;
                        cur_indent = 0.0;
                        word = right_part;
                    }
                }
            }
            if !cur.is_empty() {
                while cur.last().is_some_and(|w| w.text == " ") {
                    if let Some(popped) = cur.pop() {
                        cur_w -= popped.w;
                    }
                }
                lines.push(WrappedLine {
                    spans: std::mem::take(&mut cur),
                    total_w: cur_w.max(0.0),
                    first_line_indent: cur_indent,
                    is_last_in_para: false,
                });
                cur_w = 0.0;
                cur_indent = 0.0;
            }
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
            first_line_indent: cur_indent,
            is_last_in_para: true,
        });
    } else if let Some(last) = lines.last_mut() {
        last.is_last_in_para = true;
    }
    lines
}

#[allow(clippy::too_many_arguments)]
pub(super) fn layout_blocks(
    ast_blocks: &[MdBlock],
    col_w: f32,
    base_fs: f32,
    s: f32,
    mut y: f32,
    links: &mut Vec<String>,
    code_blocks: &mut Vec<String>,
    dims_cache: &mut HashMap<String, (u32, u32)>,
    embedded_images: &HashMap<String, Vec<u8>>,
    is_epub: bool,
    classic_cover_key: Option<&str>,
    opts: BookLayoutOpts,
    block_y_by_ast_idx: Option<&mut HashMap<usize, f32>>,
) -> (Vec<LaidOutBlock>, f32) {
    let mut out = Vec::new();
    let mut y_map = block_y_by_ast_idx;

    if !opts.book_mode && is_epub {
        if let Some(cover_key) = classic_cover_key {
            let already = matches!(ast_blocks.first(), Some(MdBlock::Image { path, .. }) if path == cover_key);
            if !already {
                let blk = layout_single_image(
                    usize::MAX,
                    cover_key,
                    None,
                    true,
                    y,
                    col_w,
                    base_fs,
                    s,
                    dims_cache,
                    embedded_images,
                );
                let (_, bh) = md_block_y_h(&blk);
                out.push(blk);
                y += bh + 18.0 * s;
            }
        }
    }

    let mut prev_was_body_para = false;
    for (idx, blk) in ast_blocks.iter().enumerate() {
        if let Some(ref mut map) = y_map {
            map.insert(idx, y);
        }
        match blk {
            MdBlock::Heading { level, spans } => {
                prev_was_body_para = false;
                let scale = match level {
                    1 => if opts.book_mode { 1.65 } else { 1.85 },
                    2 => if opts.book_mode { 1.36 } else { 1.48 },
                    3 => 1.22,
                    4 => 1.08,
                    5 => 0.98,
                    _ => 0.92,
                };
                let font_sz = base_fs * scale;
                let line_h = font_sz * 1.32;
                let mut words = tokenize_spans(spans, font_sz, links, idx, opts);
                for w in &mut words {
                    w.bold = true;
                    if !w.math && !w.code {
                        w.w = measure_word_for_opts(&w.text, font_sz, true, w.italic, false, opts);
                    }
                }
                let lines = wrap_styled_words_ex(words, col_w, 0.0, font_sz, opts);
                let ornamental = opts.book_mode && *level <= 2;
                let div_extra = if ornamental {
                    20.0 * s
                } else if *level <= 2 {
                    10.0 * s
                } else {
                    0.0
                };
                let h = lines.len() as f32 * line_h + div_extra;
                let top_pad = if idx == 0 {
                    0.0
                } else if ornamental {
                    22.0 * s
                } else {
                    10.0 * s
                };
                let by = y + top_pad;
                if let Some(ref mut map) = y_map {
                    map.insert(idx, by);
                }
                out.push(LaidOutBlock::Heading {
                    y: by,
                    h,
                    ast_block_idx: idx,
                    level: *level,
                    lines,
                    line_h,
                    font_sz,
                    ornamental,
                });
                y = by + h + if ornamental { 14.0 * s } else { 10.0 * s };
            }
            MdBlock::Paragraph(spans) => {
                let font_sz = base_fs;
                let line_h = font_sz * opts.line_mult.clamp(1.15, 2.50);
                let first_indent = if opts.book_mode && prev_was_body_para {
                    font_sz * 1.5
                } else {
                    0.0
                };
                let words = tokenize_spans(spans, font_sz, links, idx, opts);
                let lines = wrap_styled_words_ex(words, col_w, first_indent, font_sz, opts);
                let h = lines.len() as f32 * line_h;
                out.push(LaidOutBlock::Paragraph {
                    y,
                    h,
                    ast_block_idx: idx,
                    lines,
                    line_h,
                    font_sz,
                    justify: opts.book_mode && opts.justify,
                });
                let base_para_gap = if opts.book_mode { 5.0 * s } else { 11.0 * s };
                y += h + base_para_gap * opts.para_mult.clamp(0.4, 3.0);
                prev_was_body_para = true;
            }
            MdBlock::CodeBlock { lang, code } => {
                prev_was_body_para = false;
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
                    ast_block_idx: idx,
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
                prev_was_body_para = false;
                let inner_w = (col_w - 28.0 * s).max(80.0);
                let inner_opts = BookLayoutOpts {
                    justify: false,
                    ..opts
                };
                let (inner, end_y) = layout_blocks(
                    inner_ast,
                    inner_w,
                    base_fs * 0.97,
                    s,
                    y + 8.0 * s,
                    links,
                    code_blocks,
                    dims_cache,
                    embedded_images,
                    false,
                    None,
                    inner_opts,
                    None,
                );
                let h = (end_y - y).max(24.0 * s);
                out.push(LaidOutBlock::Blockquote {
                    y,
                    h,
                    ast_block_idx: idx,
                    inner,
                });
                y += h + 12.0 * s;
            }
            MdBlock::Table {
                headers,
                alignments,
                rows,
            } => {
                prev_was_body_para = false;
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
                    let mut words = tokenize_spans(spans, font_sz, links, idx, opts);
                    for w in &mut words {
                        w.bold = true;
                        w.w = measure_word_for_opts(&w.text, font_sz, true, w.italic, w.code, opts);
                    }
                    let lines = wrap_styled_words_ex(
                        words,
                        (col_widths[c] - cell_pad_x * 2.0).max(30.0),
                        0.0,
                        font_sz,
                        opts,
                    );
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
                        let words = tokenize_spans(spans, font_sz, links, idx, opts);
                        let lines = wrap_styled_words_ex(
                            words,
                            (col_widths[c] - cell_pad_x * 2.0).max(30.0),
                            0.0,
                            font_sz,
                            opts,
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
                    ast_block_idx: idx,
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
                prev_was_body_para = false;
                y = layout_list_items(
                    items,
                    *ordered,
                    *start,
                    0,
                    idx,
                    col_w,
                    base_fs,
                    s,
                    y,
                    links,
                    opts,
                    &mut out,
                );
                y += 6.0 * s;
            }
            MdBlock::HorizontalRule => {
                prev_was_body_para = false;
                let h = 18.0 * s;
                out.push(LaidOutBlock::HorizontalRule {
                    y,
                    h,
                    ast_block_idx: idx,
                });
                y += h + 8.0 * s;
            }
            MdBlock::PageBreak => {
                prev_was_body_para = false;
                out.push(LaidOutBlock::PageBreak {
                    y,
                    ast_block_idx: idx,
                });
                y += 18.0 * s;
            }
            MdBlock::MathBlock(tex) => {
                prev_was_body_para = false;
                let fs = base_fs * 1.15;
                let (body_tex, _) = split_math_eq_no(tex);
                let nodes = crate::math_render::parse_math_nodes(body_tex);
                let (_, asc, desc) = crate::math_render::measure_math_nodes(&nodes, fs);
                let h = (asc + desc + 22.0 * s).max(36.0 * s);
                out.push(LaidOutBlock::MathBlock {
                    y,
                    h,
                    ast_block_idx: idx,
                    tex: tex.clone(),
                    font_sz: fs,
                });
                y += h + 10.0 * s;
            }
            MdBlock::Image { path, caption } => {
                prev_was_body_para = false;
                let is_cover = !opts.book_mode && is_epub && idx == 0;
                let blk = layout_single_image(
                    idx,
                    path,
                    caption.as_deref(),
                    is_cover,
                    y,
                    col_w,
                    base_fs,
                    s,
                    dims_cache,
                    embedded_images,
                );
                let (_, h) = md_block_y_h(&blk);
                out.push(blk);
                y += h + if is_cover { 18.0 * s } else { 10.0 * s };
            }
        }
    }

    (out, y)
}

fn probe_image_dimensions(
    path: &str,
    dims_cache: &mut HashMap<String, (u32, u32)>,
    embedded_images: &HashMap<String, Vec<u8>>,
) -> Option<(u32, u32)> {
    if let Some(&dims) = dims_cache.get(path) {
        return if dims.0 > 0 && dims.1 > 0 { Some(dims) } else { None };
    }
    let dims = embedded_images
        .get(path)
        .and_then(|bytes| {
            image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok())
        })
        .or_else(|| image::image_dimensions(path).ok());
    dims_cache.insert(path.to_string(), dims.unwrap_or((0, 0)));
    dims.filter(|&(w, h)| w > 0 && h > 0)
}

fn ensure_md_image_decoded<'c>(
    path: &str,
    dw_u: u32,
    dh_u: u32,
    embedded_images: &HashMap<String, Vec<u8>>,
    cache: &'c mut MdDecodedImageCache,
) -> Option<&'c (u32, u32, Vec<u32>)> {
    let key = (path.to_string(), dw_u, dh_u);
    if !cache.entries.contains_key(&key) {
        let decoded_img = embedded_images
            .get(path)
            .and_then(|bytes| image::load_from_memory(bytes).ok())
            .or_else(|| image::open(path).ok())?;
        let rgb = decoded_img.to_rgb8();
        let (iw, ih) = rgb.dimensions();
        let resized = if dw_u == iw && dh_u == ih {
            rgb
        } else {
            image::imageops::resize(&rgb, dw_u, dh_u, image::imageops::FilterType::Triangle)
        };
        let mut px = Vec::with_capacity((dw_u * dh_u) as usize);
        for p in resized.pixels() {
            let [r, g, b] = p.0;
            px.push(((r as u32) << 16) | ((g as u32) << 8) | (b as u32));
        }
        cache.insert_decoded(key.clone(), dw_u, dh_u, px);
    }
    cache.entries.get(&key)
}

#[allow(clippy::too_many_arguments)]
fn layout_single_image(
    ast_block_idx: usize,
    path: &str,
    caption: Option<&str>,
    is_cover: bool,
    y: f32,
    col_w: f32,
    base_fs: f32,
    s: f32,
    dims_cache: &mut HashMap<String, (u32, u32)>,
    embedded_images: &HashMap<String, Vec<u8>>,
) -> LaidOutBlock {
    let (w_px, h_px) = if let Some((iw, ih)) = probe_image_dimensions(path, dims_cache, embedded_images) {
        let max_w = if is_cover { col_w.min(420.0 * s) } else { col_w.min(620.0 * s) };
        let max_h = if is_cover { 560.0 * s } else { 440.0 * s };
        let nat_w = (iw as f32) * s;
        let nat_h = (ih as f32) * s;
        let mut dw = if is_cover && nat_w < 260.0 * s && iw > 0 {
            (260.0 * s).min(max_w)
        } else {
            nat_w.min(max_w)
        };
        let mut dh = if nat_w > 0.0 { nat_h * (dw / nat_w) } else { 44.0 * s };
        if dh > max_h && dh > 0.0 {
            dw *= max_h / dh;
            dh = max_h;
        }
        (dw.round().max(1.0) as u32, dh.round().max(1.0) as u32)
    } else {
        (col_w.min(420.0 * s).round().max(120.0) as u32, (44.0 * s).round() as u32)
    };
    let cap_h = if caption.is_some() { 24.0 * s } else { 0.0 };
    let pad_v = if is_cover { 24.0 * s } else { 12.0 * s };
    let h = h_px as f32 + cap_h + pad_v;
    LaidOutBlock::Image {
        y,
        h,
        ast_block_idx,
        w_px,
        h_px,
        is_cover,
        caption: caption.map(|c| c.to_string()),
        fallback_path: path.to_string(),
        font_sz: base_fs * 0.88,
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_list_items(
    items: &[MdListItem],
    ordered: bool,
    start: u64,
    depth: usize,
    ast_block_idx: usize,
    col_w: f32,
    base_fs: f32,
    s: f32,
    mut y: f32,
    links: &mut Vec<String>,
    opts: BookLayoutOpts,
    out: &mut Vec<LaidOutBlock>,
) -> f32 {
    let indent = (depth as f32 + 1.0) * (22.0 * s);
    let line_h = base_fs * if opts.book_mode { opts.line_mult } else { 1.52 };
    let avail = (col_w - indent).max(80.0);

    for (i, item) in items.iter().enumerate() {
        let marker = if let Some(checked) = item.task_checked {
            ListMarker::Task(checked)
        } else if ordered {
            ListMarker::Ordered(format!("{}.", start + i as u64))
        } else {
            ListMarker::Bullet(depth)
        };
        let words = tokenize_spans(&item.spans, base_fs, links, ast_block_idx, opts);
        let lines = wrap_styled_words_ex(words, avail, 0.0, base_fs, opts);
        let h = lines.len() as f32 * line_h;
        out.push(LaidOutBlock::ListItem {
            y,
            h,
            ast_block_idx,
            indent,
            marker,
            lines,
            line_h,
            font_sz: base_fs,
            justify: opts.book_mode && opts.justify,
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
                    ast_block_idx,
                    col_w,
                    base_fs,
                    s,
                    y,
                    links,
                    opts,
                    out,
                );
            }
        }
    }
    y
}

/// Line-level book pagination: splits paragraphs/lists/headings cleanly across pages
/// with a 2-line widow/orphan guard and keeps headings attached to the following block.
pub(super) fn paginate_book_pages(
    blocks: &[LaidOutBlock],
    page_inner_h: f32,
    include_cover: bool,
) -> Vec<BookPage> {
    let mut pages = Vec::new();
    if include_cover {
        pages.push(BookPage {
            is_cover: true,
            slices: Vec::new(),
            start_y: 0.0,
            first_ast_block: 0,
        });
    }
    let page_h = page_inner_h.max(80.0);
    let mut cur_slices: Vec<BookPageSlice> = Vec::new();
    let mut cur_h = 0.0f32;
    let mut page_first_y: Option<f32> = None;
    let mut page_first_ast = 0usize;

    let flush_page = |pages: &mut Vec<BookPage>,
                      cur_slices: &mut Vec<BookPageSlice>,
                      cur_h: &mut f32,
                      page_first_y: &mut Option<f32>,
                      page_first_ast: &mut usize| {
        if !cur_slices.is_empty() {
            let sy = if pages.is_empty() {
                0.0
            } else {
                page_first_y.unwrap_or(1.0).max(1.0)
            };
            pages.push(BookPage {
                is_cover: false,
                slices: std::mem::take(cur_slices),
                start_y: sy,
                first_ast_block: *page_first_ast,
            });
            *cur_h = 0.0;
            *page_first_y = None;
        }
    };

    for (bi, blk) in blocks.iter().enumerate() {
        let (by, bh) = md_block_y_h(blk);
        let ast_idx = match blk {
            LaidOutBlock::Heading { ast_block_idx, .. }
            | LaidOutBlock::Paragraph { ast_block_idx, .. }
            | LaidOutBlock::CodeBlock { ast_block_idx, .. }
            | LaidOutBlock::Blockquote { ast_block_idx, .. }
            | LaidOutBlock::Table { ast_block_idx, .. }
            | LaidOutBlock::ListItem { ast_block_idx, .. }
            | LaidOutBlock::HorizontalRule { ast_block_idx, .. }
            | LaidOutBlock::PageBreak { ast_block_idx, .. }
            | LaidOutBlock::MathBlock { ast_block_idx, .. }
            | LaidOutBlock::Image { ast_block_idx, .. } => *ast_block_idx,
        };
        if matches!(blk, LaidOutBlock::PageBreak { .. }) {
            flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
            continue;
        }
        let next_gap = blocks
            .get(bi + 1)
            .map(|nb| {
                let (ny, _) = md_block_y_h(nb);
                (ny - (by + bh)).clamp(0.0, 40.0)
            })
            .unwrap_or(0.0);

        match blk {
            LaidOutBlock::Heading { lines, line_h, .. }
            | LaidOutBlock::Paragraph { lines, line_h, .. }
            | LaidOutBlock::ListItem { lines, line_h, .. } => {
                if let LaidOutBlock::Heading { level, .. } = blk {
                    if !cur_slices.is_empty() {
                        // Start every major chapter heading (H1) on a clean new page whenever
                        // body content already appeared on the current page.
                        let has_body_on_page = cur_slices.iter().any(|sl| {
                            matches!(
                                blocks.get(sl.block_idx),
                                Some(
                                    LaidOutBlock::Paragraph { .. }
                                        | LaidOutBlock::ListItem { .. }
                                        | LaidOutBlock::Blockquote { .. }
                                        | LaidOutBlock::Table { .. }
                                        | LaidOutBlock::CodeBlock { .. }
                                        | LaidOutBlock::Image { .. }
                                )
                            )
                        });
                        if *level == 1 && has_body_on_page {
                            flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                        } else {
                            let follow_h = blocks
                                .get(bi + 1)
                                .map(|nb| match nb {
                                    LaidOutBlock::Paragraph { line_h: lh, lines: lns, .. } => {
                                        (*lh) * (lns.len().min(2) as f32)
                                    }
                                    _ => md_block_y_h(nb).1.min(*line_h * 2.0),
                                })
                                .unwrap_or(0.0);
                            if cur_h + bh + follow_h > page_h {
                                flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                            }
                        }
                    }
                }
                let n = lines.len();
                if n == 0 {
                    continue;
                }
                let lh = (*line_h).max(10.0);
                let extra_h = (bh - (n as f32 * lh)).max(0.0);
                let mut l_start = 0usize;
                while l_start < n {
                    let avail = (page_h - cur_h).max(0.0);
                    let mut fit = ((avail + 0.5) / lh).floor() as usize;
                    // Orphan guard: don't leave a lone first line if block has >=4 lines
                    if l_start == 0 && n >= 4 && fit < 2 && !cur_slices.is_empty() {
                        flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                        continue;
                    }
                    if fit == 0 {
                        if cur_slices.is_empty() {
                            fit = 1;
                        } else {
                            flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                            continue;
                        }
                    }
                    let remaining = n - l_start;
                    // Widow guard: avoid leaving 1 lonely line on the next page when splitting
                    if fit < remaining && remaining >= 3 && (remaining - fit) == 1 && fit > 1 {
                        fit -= 1;
                    }
                    let l_end = (l_start + fit).min(n);
                    let slice_y = by + l_start as f32 * lh;
                    if page_first_y.is_none() {
                        page_first_y = Some(slice_y + 1.0);
                        page_first_ast = ast_idx;
                    }
                    let slice_h = (l_end - l_start) as f32 * lh + if l_end == n { extra_h } else { 0.0 };
                    cur_slices.push(BookPageSlice {
                        block_idx: bi,
                        ast_block_idx: ast_idx,
                        line_start: l_start,
                        line_end: l_end,
                    });
                    cur_h += slice_h;
                    l_start = l_end;
                    if l_start < n {
                        flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                    } else if cur_h + next_gap <= page_h {
                        cur_h += next_gap;
                    } else {
                        flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                    }
                }
            }
            _ => {
                if !cur_slices.is_empty() && cur_h + bh > page_h {
                    flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                }
                if page_first_y.is_none() {
                    page_first_y = Some(by + 1.0);
                    page_first_ast = ast_idx;
                }
                cur_slices.push(BookPageSlice {
                    block_idx: bi,
                    ast_block_idx: ast_idx,
                    line_start: 0,
                    line_end: usize::MAX,
                });
                cur_h += bh + next_gap;
                if cur_h >= page_h {
                    flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
                }
            }
        }
    }
    flush_page(&mut pages, &mut cur_slices, &mut cur_h, &mut page_first_y, &mut page_first_ast);
    if pages.is_empty() {
        pages.push(BookPage {
            is_cover: include_cover,
            slices: Vec::new(),
            start_y: 0.0,
            first_ast_block: 0,
        });
    }
    pages
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn md_header_bar_height_px(&self, _sf: f64) -> u32 {
        0
    }

    pub(super) fn ensure_md_layout(&mut self, canvas_w: u32, sf: f64) {
        let book_mode = self.is_distinct_epub_mode();
        let spread_mode = if book_mode {
            self.epub_prefs.two_page_spread
        } else {
            self.effective_scroll_mode() == super::state::ScrollMode::MagazineBook
        };
        let epub_sig = (
            book_mode,
            self.epub_prefs.clone(),
            self.md_preview_font_px.to_bits(),
            self.md_line_spacing.to_bits(),
            self.md_word_spacing_px.to_bits(),
            self.md_para_spacing_mult.to_bits(),
        );
        let font_family = if book_mode {
            self.epub_prefs.font_choice.resolve_font_family()
        } else {
            FontFamily::UiSans
        };
        let Some(ref doc) = self.markdown_doc else {
            self.md_layout = None;
            return;
        };
        let s = sf as f32;
        let is_magazine = spread_mode && self.md_editor_mode == MdEditorMode::Preview;
        let col_w = if book_mode {
            let outer_ratio = self.epub_prefs.effective_margin_ratio();
            let inner_ratio = self.epub_prefs.effective_inner_margin_ratio();
            if is_magazine {
                let spread_w = canvas_w
                    .saturating_sub((14.0 * s).round() as u32)
                    .min((1280.0 * s).round() as u32)
                    .max(320);
                let half_w = (spread_w / 2) as f32;
                let outer_pad = (half_w * outer_ratio).max(16.0 * s);
                let inner_pad = (half_w * inner_ratio).max(14.0 * s);
                (half_w - outer_pad - inner_pad).round().max(150.0) as u32
            } else {
                let page_w = canvas_w
                    .saturating_sub((24.0 * s).round() as u32)
                    .min((860.0 * s).round() as u32)
                    .max(220);
                let outer_pad = (page_w as f32 * outer_ratio).max(18.0 * s);
                (page_w as f32 - outer_pad * 2.0).round().max(180.0) as u32
            }
        } else if is_magazine {
            let spread_w = canvas_w
                .saturating_sub((24.0 * s).round() as u32)
                .min((1240.0 * s).round() as u32)
                .max(300);
            ((spread_w.saturating_sub((64.0 * s).round() as u32)) / 2).max(160)
        } else {
            let pad_x = (36.0 * s).round() as u32;
            let full_w = canvas_w.saturating_sub(pad_x * 2).max(200);
            if self.view_mode == ViewMode::FitPage {
                full_w.min((780.0 * s).round() as u32).max(200)
            } else {
                full_w
            }
        };
        let zoom_bits = self.zoom_level.to_bits();
        let sf_bits = sf.to_bits();
        let doc_gen = self.md_doc_gen;

        let cached_words = self
            .md_layout
            .as_ref()
            .filter(|c| c.doc_gen == doc_gen)
            .map(|c| (c.word_count, c.cum_words_by_block.clone()));

        if let Some(ref cache) = self.md_layout {
            if cache.col_w == col_w
                && cache.zoom_bits == zoom_bits
                && cache.sf_bits == sf_bits
                && cache.doc_gen == doc_gen
                && cache.epub_sig == epub_sig
            {
                return;
            }
        }

        let base_fs = if book_mode {
            (self.epub_prefs.font_size_px * s * self.zoom_level).clamp(10.0 * s, 64.0 * s)
        } else {
            (self.md_preview_font_px.clamp(10.0, 30.0) * s * self.zoom_level).clamp(9.0 * s, 64.0 * s)
        };
        let opts = if book_mode {
            BookLayoutOpts {
                book_mode: true,
                font_family,
                line_mult: self.epub_prefs.effective_line_mult(),
                word_spacing_px: self.epub_prefs.word_spacing_px * s * self.zoom_level,
                para_mult: self.epub_prefs.paragraph_spacing_mult,
                justify: self.epub_prefs.justify,
                hyphenate: self.epub_prefs.hyphenate,
                hypher_lang: hypher_lang_from_meta(
                    doc.book_meta.as_ref().and_then(|m| m.language.as_deref()),
                ),
            }
        } else {
            BookLayoutOpts {
                book_mode: false,
                font_family: FontFamily::UiSans,
                line_mult: self.md_line_spacing.clamp(1.20, 2.20),
                word_spacing_px: self.md_word_spacing_px.clamp(0.0, 8.0) * s * self.zoom_level,
                para_mult: self.md_para_spacing_mult.clamp(0.5, 2.5),
                justify: false,
                hyphenate: false,
                hypher_lang: hypher::Lang::English,
            }
        };
        let mut links = Vec::new();
        let mut code_blocks = Vec::new();
        let mut block_y_by_ast_idx = HashMap::new();
        let is_epub = Self::is_epub_path(&self.file_path);
        let classic_cover_key = doc
            .book_meta
            .as_ref()
            .and_then(|m| m.cover_image_key.as_deref());
        let initial_y = if book_mode && !is_magazine {
            self.epub_prefs.vertical_pad_px(760.0 * s, s).0
        } else {
            24.0 * s
        };
        let (blocks, end_y) = layout_blocks(
            &doc.blocks,
            col_w as f32,
            base_fs,
            s,
            initial_y,
            &mut links,
            &mut code_blocks,
            &mut self.md_image_cache.dims,
            &doc.embedded_images,
            is_epub,
            classic_cover_key,
            opts,
            Some(&mut block_y_by_ast_idx),
        );
        let code_h_scroll = vec![0.0f32; code_blocks.len()];
        let (word_count, cum_words_by_block) =
            cached_words.unwrap_or_else(|| build_cum_words_by_block(&doc.blocks));
        self.md_layout = Some(MdLayoutCache {
            col_w,
            zoom_bits,
            sf_bits,
            doc_gen,
            epub_sig,
            book_mode,
            book_font_family: font_family,
            total_h: end_y + 48.0 * s,
            blocks,
            block_y_by_ast_idx,
            book_pages: Vec::new(),
            book_pages_h_bits: 0,
            book_pages_spread_flag: false,
            links,
            code_blocks,
            code_h_scroll,
            word_count,
            cum_words_by_block,
        });
    }

    fn ensure_book_pages(&mut self, page_inner_h: f32, include_cover: bool) {
        let h_bits = page_inner_h.to_bits();
        let Some(ref mut cache) = self.md_layout else {
            return;
        };
        if !cache.book_pages.is_empty()
            && cache.book_pages_h_bits == h_bits
            && cache.book_pages_spread_flag == include_cover
        {
            return;
        }
        cache.book_pages = paginate_book_pages(&cache.blocks, page_inner_h, include_cover);
        cache.book_pages_h_bits = h_bits;
        cache.book_pages_spread_flag = include_cover;
    }

    pub(super) fn md_max_scroll_y(&self, canvas_h: u32, sf: f64) -> f32 {
        let hdr_h = self.md_header_bar_height_px(sf) as f32;
        let view_h = (canvas_h as f32 - hdr_h).max(60.0);
        self.md_layout
            .as_ref()
            .map(|c| (c.total_h - view_h).max(0.0))
            .unwrap_or(0.0)
    }

    pub(super) fn md_magazine_page_inner_h(&self, canvas_h: u32, sf: f64) -> f32 {
        let s = sf as f32;
        let hdr_h = self.md_header_bar_height_px(sf) as f32;
        let content_h = (canvas_h as f32 - hdr_h).max(160.0);
        let spread_h = (content_h - 24.0 * s).max(140.0);
        if self.is_distinct_epub_mode() {
            let (top_pad, bot_pad) = self.epub_prefs.vertical_pad_px(spread_h, s);
            (spread_h - top_pad - bot_pad - 22.0 * s).max(80.0)
        } else {
            (spread_h - 44.0 * s - 22.0 * s).max(80.0)
        }
    }

    /// Cached page info used by the Distinct EPUB bottom strip & sidebar (`1`-based).
    pub(super) fn epub_book_page_info_cached(&self) -> Option<(usize, Option<usize>, usize)> {
        let cache = self.md_layout.as_ref()?;
        if !cache.book_mode || cache.book_pages.is_empty() {
            return None;
        }
        let total = cache.book_pages.len().max(1);
        let mut page_idx = 0usize;
        for (i, pg) in cache.book_pages.iter().enumerate() {
            if pg.start_y <= self.scroll_y + 2.0 {
                page_idx = i;
            } else {
                break;
            }
        }
        if self.epub_prefs.two_page_spread {
            let spread_idx = md_magazine_spread_for_page(page_idx);
            let (left_idx, right_idx_opt) = md_magazine_pages_for_spread(spread_idx, total);
            Some((left_idx + 1, right_idx_opt.map(|r| r + 1), total))
        } else {
            Some((page_idx + 1, None, total))
        }
    }

    /// First AST block currently visible in Distinct EPUB mode (for active TOC highlight & bookmarking).
    pub(super) fn epub_book_current_first_block(&self) -> Option<usize> {
        let cache = self.md_layout.as_ref()?;
        if cache.book_mode && !cache.book_pages.is_empty() {
            let mut page_idx = 0usize;
            for (i, pg) in cache.book_pages.iter().enumerate() {
                if pg.start_y <= self.scroll_y + 2.0 {
                    page_idx = i;
                } else {
                    break;
                }
            }
            return cache.book_pages.get(page_idx).map(|pg| pg.first_ast_block);
        }
        let mut best: Option<(f32, usize)> = None;
        for (&ast_idx, &by) in &cache.block_y_by_ast_idx {
            if by <= self.scroll_y + 32.0 {
                if best.is_none_or(|(best_y, _)| by >= best_y) {
                    best = Some((by, ast_idx));
                }
            }
        }
        best.map(|(_, idx)| idx)
    }

    /// Jumps the Distinct EPUB reader directly to a `[0.0, 1.0]` scrubber ratio.
    pub(super) fn jump_epub_to_progress_ratio(&mut self, ratio: f32, buf_w: u32, buf_h: u32, sf: f64) {
        let r = ratio.clamp(0.0, 1.0);
        let sidebar_w = self.sidebar_width_px(sf).min(buf_w.saturating_sub(64));
        let top_bars_h = self.menu_bar_height_px(sf) + self.tab_bar_height_px(sf);
        let status_h = self.status_bar_height_px(sf);
        let canvas_w = buf_w.saturating_sub(sidebar_w).max(200);
        let canvas_h = buf_h.saturating_sub(top_bars_h + status_h).max(160);
        self.ensure_md_layout(canvas_w, sf);
        if self.is_distinct_epub_mode() && self.epub_prefs.two_page_spread {
            let page_inner_h = self.md_magazine_page_inner_h(canvas_h, sf);
            self.ensure_book_pages(page_inner_h, true);
            if let Some(ref cache) = self.md_layout {
                if !cache.book_pages.is_empty() {
                    let target_page = ((r * (cache.book_pages.len() - 1) as f32).round() as usize)
                        .min(cache.book_pages.len() - 1);
                    let spread_idx = md_magazine_spread_for_page(target_page);
                    let (left_idx, _) = md_magazine_pages_for_spread(spread_idx, cache.book_pages.len());
                    self.scroll_y = cache.book_pages[left_idx].start_y;
                }
            }
        } else {
            let max_s = self.md_max_scroll_y(canvas_h, sf);
            self.scroll_y = r * max_s;
        }
        self.epub_selection_drag = None;
        self.epub_selection_pill_rect = None;
        self.page_turn_anim = None;
        self.request_redraw();
    }

    /// Returns `(left_page_1based, right_page_1based_opt, total_pages)` when in MagazineBook mode.
    /// Spread 0 is Cover Page 1 alone (`(1, None, total)`), and spreads 1..N are `2–3`, `4–5`, ...
    pub(super) fn md_magazine_page_info(&self, canvas_h: u32, sf: f64) -> Option<(usize, Option<usize>, usize)> {
        if self.is_distinct_epub_mode() {
            if let Some(info) = self.epub_book_page_info_cached() {
                return Some(info);
            }
        }
        let cache = self.md_layout.as_ref()?;
        let page_inner_h = self.md_magazine_page_inner_h(canvas_h, sf);
        let pages = md_paginate_blocks(&cache.blocks, page_inner_h);
        let total = pages.len().max(1);
        let mut page_idx = 0usize;
        for (i, &sy) in pages.iter().enumerate() {
            if sy <= self.scroll_y + 2.0 {
                page_idx = i;
            } else {
                break;
            }
        }
        let spread_idx = md_magazine_spread_for_page(page_idx);
        let (left_idx, right_idx_opt) = md_magazine_pages_for_spread(spread_idx, total);
        Some((left_idx + 1, right_idx_opt.map(|r| r + 1), total))
    }

    /// Maps an AST block index to a 1-based book Location number (`Loc X`).
    pub(super) fn epub_loc_for_block(&self, ast_block_idx: usize) -> usize {
        if let Some(ref cache) = self.md_layout {
            if !cache.book_pages.is_empty() {
                if let Some((pi, _)) = cache.book_pages.iter().enumerate().find(|(_, pg)| {
                    pg.slices.iter().any(|sl| sl.ast_block_idx >= ast_block_idx)
                }) {
                    return pi.max(1);
                }
                for (pi, pg) in cache.book_pages.iter().enumerate() {
                    if pg.first_ast_block >= ast_block_idx {
                        return pi.max(1);
                    }
                }
                return cache.book_pages.len().saturating_sub(1).max(1);
            }
            if let Some(&by) = cache.block_y_by_ast_idx.get(&ast_block_idx) {
                let frac = (by / cache.total_h.max(1.0)).clamp(0.0, 1.0);
                return ((frac * (cache.word_count.max(1) as f32 / 220.0).max(1.0)).round() as usize).max(1);
            }
        }
        let total_blocks = self.markdown_doc.as_ref().map(|d| d.blocks.len()).unwrap_or(1).max(1);
        ((ast_block_idx * 100) / total_blocks).max(1)
    }

    /// Estimates remaining reading time in the current chapter (`"~6 min left"` or `"< 1 min left"`).
    pub(super) fn epub_chapter_time_left_label(&self) -> Option<String> {
        let doc = self.markdown_doc.as_ref()?;
        let cache = self.md_layout.as_ref()?;
        if doc.blocks.is_empty() || cache.cum_words_by_block.len() != doc.blocks.len() + 1 {
            return None;
        }
        let cur_blk = self.current_visible_epub_block_idx().min(doc.blocks.len().saturating_sub(1));
        // End of current chapter = next PageBreak or next level-1 TocEntry after cur_blk
        let mut chap_end = doc.blocks.len();
        for bi in (cur_blk + 1)..doc.blocks.len() {
            if matches!(doc.blocks[bi], MdBlock::PageBreak) {
                chap_end = bi;
                break;
            }
        }
        if let Some(ref meta) = doc.book_meta {
            fn find_next_top_toc(entries: &[crate::markdown::TocEntry], after: usize, best: &mut usize) {
                for e in entries {
                    if e.block_idx > after + 1 && e.block_idx < *best {
                        *best = e.block_idx;
                    }
                    if e.level == 0 {
                        find_next_top_toc(&e.children, after, best);
                    }
                }
            }
            find_next_top_toc(&meta.toc, cur_blk, &mut chap_end);
        }
        let w_now = cache.cum_words_by_block[cur_blk];
        let w_end = cache.cum_words_by_block[chap_end.min(doc.blocks.len())];
        let rem_words = w_end.saturating_sub(w_now);
        if rem_words == 0 {
            return Some("< 1 min left".to_string());
        }
        let mins = ((rem_words as f32) / 220.0).ceil() as usize;
        if mins <= 1 && rem_words < 180 {
            Some("< 1 min left".to_string())
        } else {
            Some(format!("{} min left", mins.max(1)))
        }
    }

    pub(super) fn step_md_magazine_spread(&mut self, dir: i8, buf_w: u32, canvas_h: u32, sf: f64) {
        let sidebar_w = self.sidebar_width_px(sf).min(buf_w.saturating_sub(64));
        let canvas_w = buf_w.saturating_sub(sidebar_w).max(200);
        self.ensure_md_layout(canvas_w, sf);
        if self.is_distinct_epub_mode() && self.epub_prefs.two_page_spread {
            let page_inner_h = self.md_magazine_page_inner_h(canvas_h, sf);
            self.ensure_book_pages(page_inner_h, true);
            let Some(ref cache) = self.md_layout else {
                return;
            };
            let total = cache.book_pages.len().max(1);
            let num_spreads = md_magazine_total_spreads(total);
            let mut page_idx = 0usize;
            for (i, pg) in cache.book_pages.iter().enumerate() {
                if pg.start_y <= self.scroll_y + 2.0 {
                    page_idx = i;
                } else {
                    break;
                }
            }
            let cur_spread = md_magazine_spread_for_page(page_idx).min(num_spreads.saturating_sub(1));
            let next_spread = if dir > 0 {
                (cur_spread + 1).min(num_spreads.saturating_sub(1))
            } else {
                cur_spread.saturating_sub(1)
            };
            if next_spread != cur_spread {
                let old_scroll_y = self.scroll_y;
                let (target_left_idx, _) = md_magazine_pages_for_spread(next_spread, total);
                self.scroll_y = cache.book_pages[target_left_idx.min(cache.book_pages.len().saturating_sub(1))].start_y;
                self.epub_selection_drag = None;
                self.epub_selection_pill_rect = None;
                self.trigger_epub_page_curl(dir, old_scroll_y);
                self.request_redraw();
            }
            return;
        }
        let Some(ref cache) = self.md_layout else {
            return;
        };
        let page_inner_h = self.md_magazine_page_inner_h(canvas_h, sf);
        let pages = md_paginate_blocks(&cache.blocks, page_inner_h);
        let total = pages.len().max(1);
        let num_spreads = md_magazine_total_spreads(total);
        let mut page_idx = 0usize;
        for (i, &sy) in pages.iter().enumerate() {
            if sy <= self.scroll_y + 2.0 {
                page_idx = i;
            } else {
                break;
            }
        }
        let cur_spread = md_magazine_spread_for_page(page_idx).min(num_spreads.saturating_sub(1));
        let next_spread = if dir > 0 {
            (cur_spread + 1).min(num_spreads.saturating_sub(1))
        } else {
            cur_spread.saturating_sub(1)
        };
        if next_spread != cur_spread {
            let old_scroll_y = self.scroll_y;
            let (target_left_idx, _) = md_magazine_pages_for_spread(next_spread, total);
            self.scroll_y = pages[target_left_idx.min(pages.len().saturating_sub(1))];
            if self.page_turn_anim_ms > 0 {
                self.page_turn_anim = Some(PageTurnAnim {
                    started_at: std::time::Instant::now(),
                    duration_ms: self.page_turn_anim_ms.clamp(220, 1400),
                    direction: dir,
                    from_scroll_y: Some(old_scroll_y),
                    prev_spread: None,
                });
            }
            self.request_redraw();
        }
    }

    /// Jumps to a `MdSearchMatch` by index (switching EPUB page spread or scrolling cleanly).
    pub(super) fn jump_to_md_search_match(&mut self, match_idx: usize) {
        let Some(m) = self.md_search_matches.get(match_idx).cloned() else {
            return;
        };
        self.page_turn_anim = None;
        self.scroll_to_md_heading(m.ast_block_idx);
        // If Markdown/LaTeX editor is visible, also scroll/jump the source editor to the query
        if self.md_editor_mode != MdEditorMode::Preview
            && !Self::is_epub_path(&self.file_path)
            && !Self::is_word_path(&self.file_path)
        {
            if let Some(ref mut ed) = self.md_editor {
                let q = self.search_query.clone();
                if !q.is_empty() {
                    let cs = self.search_case_sensitive;
                    let q_cmp = if cs { q.clone() } else { q.to_lowercase() };
                    let mut seen = 0usize;
                    for (li, line) in ed.lines.iter().enumerate() {
                        let hay = if cs { line.clone() } else { line.to_lowercase() };
                        if let Some(col) = hay.find(&q_cmp) {
                            if seen >= match_idx {
                                let start_b = if line.is_char_boundary(col) { col } else { 0 };
                                let end_b = (start_b + q.len()).min(line.len());
                                let end_b = if line.is_char_boundary(end_b) { end_b } else { line.len() };
                                ed.sel_anchor = Some((li, start_b));
                                ed.cursor = (li, end_b);
                                break;
                            }
                            seen += 1;
                        }
                    }
                }
            }
        }
    }

    pub(super) fn scroll_to_md_heading(&mut self, ast_block_idx: usize) {
        if self.is_distinct_epub_mode() && self.epub_prefs.two_page_spread {
            if let Some(ref cache) = self.md_layout {
                if !cache.book_pages.is_empty() {
                    if let Some((pi, _)) = cache.book_pages.iter().enumerate().find(|(_, pg)| {
                        pg.slices.iter().any(|sl| sl.ast_block_idx == ast_block_idx)
                    }) {
                        let spread_idx = md_magazine_spread_for_page(pi);
                        let (left_idx, _) = md_magazine_pages_for_spread(spread_idx, cache.book_pages.len());
                        self.scroll_y = cache.book_pages[left_idx].start_y;
                        self.epub_selection_drag = None;
                        self.epub_selection_pill_rect = None;
                        self.request_redraw();
                        return;
                    }
                }
            }
        }
        if let Some(ref cache) = self.md_layout {
            if let Some(&y) = cache.block_y_by_ast_idx.get(&ast_block_idx) {
                self.scroll_y = (y - 16.0).max(0.0);
                self.epub_selection_drag = None;
                self.epub_selection_pill_rect = None;
                self.request_redraw();
            }
        }
    }

    pub(super) fn md_studio_bar_height_px(&self, sf: f64) -> u32 {
        if self.ui_skin == super::theme::UiSkin::Distinct
            && self.is_markdown_tab()
            && !Self::is_epub_path(&self.file_path)
            && !Self::is_word_path(&self.file_path)
            && self.md_editor_mode != MdEditorMode::Preview
        {
            super::text_editor::editor_header_height_px(sf)
        } else {
            0
        }
    }

    pub(super) fn md_split_editor_width(&self, canvas_w: u32) -> u32 {
        match self.md_editor_mode {
            MdEditorMode::Edit => canvas_w,
            MdEditorMode::Split => {
                if self.ui_skin == super::theme::UiSkin::Distinct {
                    let ratio = Self::clamp_md_split_ratio(self.md_split_ratio);
                    let raw = (canvas_w as f32 * ratio).round() as u32;
                    let min_p = 140.min(canvas_w / 3);
                    raw.clamp(min_p, canvas_w.saturating_sub(min_p).max(min_p))
                } else {
                    canvas_w / 2
                }
            }
            MdEditorMode::Preview => 0,
        }
    }

    pub(super) fn md_editor_header_rect(
        &self,
        sidebar_w: u32,
        top_bars_h: u32,
        win_w: u32,
        win_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32)> {
        let studio_h = self.md_studio_bar_height_px(sf);
        if studio_h == 0 {
            return None;
        }
        let canvas_w = win_w.saturating_sub(sidebar_w);
        let canvas_h = win_h.saturating_sub(top_bars_h + status_bar_h);
        let hdr_h = self.md_header_bar_height_px(sf);
        if canvas_w == 0 || canvas_h <= hdr_h + studio_h {
            return None;
        }
        let ew = self.md_split_editor_width(canvas_w);
        if ew == 0 {
            return None;
        }
        Some((sidebar_w as i32, (top_bars_h + hdr_h) as i32, ew, studio_h))
    }

    pub(super) fn md_split_divider_x(&self, sidebar_w: u32, win_w: u32) -> Option<i32> {
        if !self.is_markdown_tab()
            || Self::is_epub_path(&self.file_path)
            || Self::is_word_path(&self.file_path)
            || self.md_editor_mode != MdEditorMode::Split
            || self.ui_skin != super::theme::UiSkin::Distinct
        {
            return None;
        }
        let canvas_w = win_w.saturating_sub(sidebar_w);
        if canvas_w < 80 {
            return None;
        }
        Some(sidebar_w as i32 + self.md_split_editor_width(canvas_w) as i32)
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
        let studio_h = self.md_studio_bar_height_px(sf);
        let content_h = canvas_h.saturating_sub(hdr_h + studio_h);
        if canvas_w == 0 || content_h == 0 {
            return None;
        }
        let ex = sidebar_w as i32;
        let ey = (top_bars_h + hdr_h + studio_h) as i32;
        let ew = self.md_split_editor_width(canvas_w);
        if ew == 0 {
            return None;
        }
        Some((ex, ey, ew, content_h))
    }

    pub(super) fn ensure_md_editor_initialized(&mut self) {
        if self.md_editor.is_none() {
            let src = self.md_source.as_deref().unwrap_or("");
            let mut ed = TextEditorState::from_text(src);
            ed.font_size_px = self.md_code_font_px.clamp(10.0, 28.0);
            self.md_editor = Some(ed);
        } else if let Some(ref mut ed) = self.md_editor {
            ed.font_size_px = self.md_code_font_px.clamp(10.0, 28.0);
        }
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
        self.epub_word_rects.clear();
        let canvas_w = buf_w.saturating_sub(sidebar_w);
        let canvas_h = buf_h.saturating_sub(top_bars_h + status_bar_h);
        if canvas_w == 0 || canvas_h == 0 {
            return;
        }
        let split_ew = self.md_split_editor_width(canvas_w);
        let preview_w = match self.md_editor_mode {
            MdEditorMode::Preview => canvas_w,
            MdEditorMode::Split => canvas_w.saturating_sub(split_ew + 1).max(140),
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

        let (page_bg, content_theme) = md_reading_theme(theme, self.reading_filter, self.ui_theme_mode);
        fill_rect_clipped(buffer, buf_w, buf_h, x0, y0, canvas_w, canvas_h, page_bg);

        let content_top = y0 + hdr_h as i32;
        let content_bottom = y0 + canvas_h as i32;
        self.md_active_hits.clear();

        let book_mode = self.is_distinct_epub_mode();
        let sel_range = self.epub_selection_drag.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });

        if preview_w > 0 {
            let max_scroll = self.md_max_scroll_y(canvas_h, sf);
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
            let scroll_y = self.scroll_y;
            let preview_x0 = match self.md_editor_mode {
                MdEditorMode::Split => x0 + split_ew as i32 + 1,
                _ => x0,
            };

            if book_mode && self.md_editor_mode == MdEditorMode::Preview {
                // Distinct EPUB Book Desk
                fill_rect_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    x0,
                    content_top,
                    preview_w,
                    (content_bottom - content_top).max(0) as u32,
                    theme.canvas_bg,
                );
                let book_meta_title = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref().map(|m| m.title.clone()))
                    .or_else(|| self.markdown_doc.as_ref().map(|d| d.title.clone()))
                    .unwrap_or_else(|| self.filename_display());
                let book_meta_author = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref().and_then(|m| m.author.clone()));
                let book_meta_pub = self
                    .markdown_doc
                    .as_ref()
                    .and_then(|d| d.book_meta.as_ref().and_then(|m| m.publisher.clone()));

                if self.epub_prefs.two_page_spread {
                    let avail_h = (content_bottom - content_top).max(160) as u32;
                    let spread_h = avail_h.saturating_sub((16.0 * s).round() as u32).max(140);
                    let spread_y = content_top + ((avail_h - spread_h) / 2) as i32;
                    let spread_w = preview_w
                        .saturating_sub((14.0 * s).round() as u32)
                        .min((1280.0 * s).round() as u32)
                        .max(320);
                    let half_w = (spread_w / 2) as f32;
                    let outer_pad = (half_w * self.epub_prefs.effective_margin_ratio()).max(16.0 * s).round() as i32;
                    let inner_pad = (half_w * self.epub_prefs.effective_inner_margin_ratio()).max(14.0 * s).round() as i32;
                    let (top_pad_f, bot_pad_f) = self.epub_prefs.vertical_pad_px(spread_h as f32, s);
                    let top_pad = top_pad_f.round() as i32;
                    let bot_pad = bot_pad_f.round() as i32;
                    let page_top = spread_y + top_pad;
                    let page_bot = (spread_y + spread_h as i32 - bot_pad - (22.0 * s).round() as i32).max(page_top + 40);
                    let page_inner_h = (page_bot - page_top) as f32;

                    self.ensure_book_pages(page_inner_h, true);
                    self.ensure_epub_cover_thumb();
                    let cover_thumb_opt = self.epub_cover_thumb.clone();
                    let highlights = self.epub_book_data.highlights.clone();
                    let bookmarks = self.epub_book_data.bookmarks.clone();
                    let spread_x = preview_x0 + ((preview_w.saturating_sub(spread_w)) / 2) as i32;
                    let active_search_idx = if !self.md_search_matches.is_empty() {
                        Some(self.current_search_match.min(self.md_search_matches.len() - 1))
                    } else {
                        None
                    };
                    let empty_imgs: HashMap<String, Vec<u8>> = HashMap::new();
                    let embedded_imgs = self
                        .markdown_doc
                        .as_ref()
                        .map(|d| &d.embedded_images)
                        .unwrap_or(&empty_imgs);

                    if let Some(ref cache) = self.md_layout {
                        let rctx_base = BookDrawCtx {
                            book_mode: true,
                            font_family: cache.book_font_family,
                            highlights: &highlights,
                            sel_range,
                            accent_color: content_theme.accent,
                            search_matches: &self.md_search_matches,
                            active_search_match: active_search_idx,
                        };

                        // Draw the live destination spread directly into `buffer`
                        render_distinct_epub_spread_surface(
                            buffer,
                            buf_w,
                            buf_h,
                            spread_x,
                            spread_y,
                            spread_w,
                            spread_h,
                            top_pad,
                            bot_pad,
                            outer_pad,
                            inner_pad,
                            scroll_y,
                            cache,
                            embedded_imgs,
                            &mut self.md_image_cache,
                            cover_thumb_opt.as_ref(),
                            &book_meta_title,
                            book_meta_author.as_deref(),
                            book_meta_pub.as_deref(),
                            &bookmarks,
                            theme.canvas_bg,
                            page_bg,
                            &content_theme,
                            &self.hovered_widget,
                            s,
                            Some(&mut self.md_active_hits),
                            Some(&mut self.epub_word_rects),
                            rctx_base,
                            true,
                        );

                        // Iconic Apple Books 3D curved hand page-curl animation (zero idle RAM once done)
                        let mut anim_done = false;
                        if let Some(ref mut anim) = self.page_turn_anim {
                            let elapsed_ms = anim.started_at.elapsed().as_secs_f32() * 1000.0;
                            let linear_t = (elapsed_ms / anim.duration_ms.max(1) as f32).clamp(0.0, 1.0);
                            if linear_t >= 1.0 {
                                anim_done = true;
                            } else if let Some(old_y) = anim.from_scroll_y {
                                let need_snap = anim
                                    .prev_spread
                                    .as_ref()
                                    .is_none_or(|(pw, ph, _)| *pw != spread_w || *ph != spread_h);
                                if need_snap {
                                    let mut snap = vec![theme.canvas_bg; (spread_w as usize) * (spread_h as usize)];
                                    let snap_ctx = BookDrawCtx {
                                        book_mode: true,
                                        font_family: cache.book_font_family,
                                        highlights: &highlights,
                                        sel_range: None,
                                        accent_color: content_theme.accent,
                                        search_matches: &self.md_search_matches,
                                        active_search_match: active_search_idx,
                                    };
                                    render_distinct_epub_spread_surface(
                                        &mut snap,
                                        spread_w,
                                        spread_h,
                                        0,
                                        0,
                                        spread_w,
                                        spread_h,
                                        top_pad,
                                        bot_pad,
                                        outer_pad,
                                        inner_pad,
                                        old_y,
                                        cache,
                                        embedded_imgs,
                                        &mut self.md_image_cache,
                                        cover_thumb_opt.as_ref(),
                                        &book_meta_title,
                                        book_meta_author.as_deref(),
                                        book_meta_pub.as_deref(),
                                        &bookmarks,
                                        theme.canvas_bg,
                                        page_bg,
                                        &content_theme,
                                        &None,
                                        s,
                                        None,
                                        None,
                                        snap_ctx,
                                        false,
                                    );
                                    anim.prev_spread = Some((spread_w, spread_h, snap));
                                }
                                if let Some((_, _, ref prev_px)) = anim.prev_spread {
                                    draw_apple_books_page_curl(
                                        buffer,
                                        buf_w,
                                        buf_h,
                                        prev_px,
                                        spread_x,
                                        spread_y,
                                        spread_w,
                                        spread_h,
                                        linear_t,
                                        anim.direction,
                                        page_bg,
                                        s,
                                    );
                                }
                            } else {
                                let eased = 1.0 - (1.0 - linear_t).powi(3);
                                draw_magazine_page_turn_overlay(
                                    buffer,
                                    buf_w,
                                    buf_h,
                                    spread_x,
                                    spread_y,
                                    spread_w,
                                    spread_h,
                                    eased,
                                    anim.direction,
                                    s,
                                );
                            }
                        }
                        if anim_done {
                            self.page_turn_anim = None;
                        }
                    }
                } else if let Some(ref cache) = self.md_layout {
                    // 1-Page centered Distinct Book Flow
                    let pad = ((preview_w.min((860.0 * s).round() as u32) as f32) * self.epub_prefs.effective_margin_ratio()).max(18.0 * s);
                    let sheet_w = (cache.col_w as f32 + pad * 2.0).round() as u32;
                    let sheet_x = preview_x0 + ((preview_w.saturating_sub(sheet_w)) / 2) as i32;
                    let sheet_y = content_top + (8.0 * s).round() as i32;
                    let sheet_h = (content_bottom - sheet_y - (8.0 * s).round() as i32).max(40) as u32;
                    draw_soft_shadow(buffer, buf_w, buf_h, sheet_x, sheet_y, sheet_w, sheet_h, 4.0 * s, (10.0 * s).round() as i32, (3.0 * s).round() as i32, (0, 0, 0), 56);
                    fill_rounded_rect(buffer, buf_w, buf_h, sheet_x, sheet_y, sheet_w, sheet_h, 4.0 * s, page_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, sheet_x, sheet_y, sheet_w, sheet_h, 4.0 * s, content_theme.border_subtle);

                    let col_x = sheet_x + ((sheet_w as i32 - cache.col_w as i32) / 2);
                    let view_top = scroll_y;
                    let view_bot = scroll_y + (content_bottom - content_top) as f32;
                    let highlights = self.epub_book_data.highlights.clone();
                    let active_search_idx = if !self.md_search_matches.is_empty() {
                        Some(self.current_search_match.min(self.md_search_matches.len() - 1))
                    } else {
                        None
                    };
                    let empty_imgs: HashMap<String, Vec<u8>> = HashMap::new();
                    let embedded_imgs = self
                        .markdown_doc
                        .as_ref()
                        .map(|d| &d.embedded_images)
                        .unwrap_or(&empty_imgs);
                    let rctx = BookDrawCtx {
                        book_mode: true,
                        font_family: cache.book_font_family,
                        highlights: &highlights,
                        sel_range,
                        accent_color: content_theme.accent,
                        search_matches: &self.md_search_matches,
                        active_search_match: active_search_idx,
                    };
                    for blk in &cache.blocks {
                        draw_laid_out_block(
                            buffer,
                            buf_w,
                            buf_h,
                            blk,
                            col_x,
                            cache.col_w as f32,
                            content_top + (12.0 * s).round() as i32,
                            content_bottom - (12.0 * s).round() as i32,
                            scroll_y,
                            view_top,
                            view_bot,
                            &cache.code_h_scroll,
                            embedded_imgs,
                            &mut self.md_image_cache,
                            &content_theme,
                            &self.hovered_widget,
                            s,
                            &mut self.md_active_hits,
                            Some(&mut self.epub_word_rects),
                            rctx,
                        );
                    }
                }
            } else {
                let is_magazine = self.effective_scroll_mode() == super::state::ScrollMode::MagazineBook
                    && self.md_editor_mode == MdEditorMode::Preview;
                let active_search_idx = if !self.md_search_matches.is_empty() {
                    Some(self.current_search_match.min(self.md_search_matches.len() - 1))
                } else {
                    None
                };
                let empty_imgs: HashMap<String, Vec<u8>> = HashMap::new();
                let embedded_imgs = self
                    .markdown_doc
                    .as_ref()
                    .map(|d| &d.embedded_images)
                    .unwrap_or(&empty_imgs);
                let search_ctx = BookDrawCtx {
                    search_matches: &self.md_search_matches,
                    active_search_match: active_search_idx,
                    accent_color: content_theme.accent,
                    ..BookDrawCtx::default()
                };

                if is_magazine {
                    // Warm matte desk around the open book (Markdown / LaTeX MagazineBook mode with 3D Curl!)
                    fill_rect_clipped(buffer, buf_w, buf_h, x0, content_top, preview_w, (content_bottom - content_top).max(0) as u32, theme.canvas_bg);
                    let avail_h = (content_bottom - content_top).max(160) as u32;
                    let spread_h = avail_h.saturating_sub((24.0 * s).round() as u32).max(140);
                    let spread_y = content_top + ((avail_h - spread_h) / 2) as i32;
                    let spread_w = preview_w.saturating_sub((24.0 * s).round() as u32).min((1240.0 * s).round() as u32).max(300);
                    let page_pad_y = (22.0 * s).round() as i32;
                    let page_top = spread_y + page_pad_y;
                    let page_bot = (spread_y + spread_h as i32 - page_pad_y - (18.0 * s).round() as i32).max(page_top + 40);
                    let page_inner_h = (page_bot - page_top) as f32;

                    if let Some(ref cache) = self.md_layout {
                        let pages = md_paginate_blocks(&cache.blocks, page_inner_h);
                        let spread_x = preview_x0 + ((preview_w.saturating_sub(spread_w)) / 2) as i32;
                        let anim_rect = render_classic_md_magazine_spread_surface(
                            buffer,
                            buf_w,
                            buf_h,
                            preview_x0,
                            preview_w,
                            spread_x,
                            spread_y,
                            spread_w,
                            spread_h,
                            page_top,
                            page_bot,
                            scroll_y,
                            cache,
                            &pages,
                            embedded_imgs,
                            &mut self.md_image_cache,
                            theme.canvas_bg,
                            page_bg,
                            &content_theme,
                            &self.hovered_widget,
                            s,
                            Some(&mut self.md_active_hits),
                            search_ctx,
                            true,
                        );

                        let mut anim_done = false;
                        if let Some(ref mut anim) = self.page_turn_anim {
                            let elapsed_ms = anim.started_at.elapsed().as_secs_f32() * 1000.0;
                            let linear_t = (elapsed_ms / anim.duration_ms.max(1) as f32).clamp(0.0, 1.0);
                            if linear_t >= 1.0 {
                                anim_done = true;
                            } else if let Some(old_y) = anim.from_scroll_y {
                                let (ax, ay, aw, ah) = anim_rect;
                                let need_snap = anim
                                    .prev_spread
                                    .as_ref()
                                    .is_none_or(|(pw, ph, _)| *pw != aw || *ph != ah);
                                if need_snap && aw > 0 && ah > 0 {
                                    let mut snap = vec![theme.canvas_bg; (aw as usize) * (ah as usize)];
                                    render_classic_md_magazine_spread_surface(
                                        &mut snap,
                                        aw,
                                        ah,
                                        0,
                                        aw,
                                        0,
                                        0,
                                        aw,
                                        ah,
                                        page_pad_y,
                                        (ah as i32 - page_pad_y - (18.0 * s).round() as i32).max(page_pad_y + 40),
                                        old_y,
                                        cache,
                                        &pages,
                                        embedded_imgs,
                                        &mut self.md_image_cache,
                                        theme.canvas_bg,
                                        page_bg,
                                        &content_theme,
                                        &None,
                                        s,
                                        None,
                                        search_ctx,
                                        false,
                                    );
                                    anim.prev_spread = Some((aw, ah, snap));
                                }
                                if let Some((_, _, ref prev_px)) = anim.prev_spread {
                                    draw_apple_books_page_curl(
                                        buffer, buf_w, buf_h, prev_px, ax, ay, aw, ah, linear_t, anim.direction, page_bg, s,
                                    );
                                }
                            } else {
                                let eased = 1.0 - (1.0 - linear_t).powi(3);
                                draw_magazine_page_turn_overlay(
                                    buffer, buf_w, buf_h,
                                    anim_rect.0, anim_rect.1, anim_rect.2, anim_rect.3,
                                    eased, anim.direction, s,
                                );
                            }
                        }
                        if anim_done {
                            self.page_turn_anim = None;
                        }
                    }
                } else if let Some(ref cache) = self.md_layout {
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
                            embedded_imgs,
                            &mut self.md_image_cache,
                            &content_theme,
                            &self.hovered_widget,
                            s,
                            &mut self.md_active_hits,
                            None,
                            search_ctx,
                        );
                    }
                }
            }
        }

        if let Some((hx, hy, hw, hh)) = self.md_editor_header_rect(sidebar_w, top_bars_h, buf_w, buf_h, status_bar_h, sf) {
            let hovered_act = match self.hovered_widget {
                Some(UiHit::MdEditorBar(a)) => Some(a),
                _ => None,
            };
            super::text_editor::draw_editor_header(
                buffer,
                buf_w,
                buf_h,
                hx,
                hy,
                hw,
                hh,
                self.active_editor_syntax(),
                self.md_editor_mode,
                false,
                hovered_act,
                &theme,
                sf,
            );
        }

        if let Some((ex, ey, ew, eh)) = self.md_editor_rect(sidebar_w, top_bars_h, buf_w, buf_h, status_bar_h, sf) {
            self.ensure_md_editor_initialized();
            let syntax = self.active_editor_syntax();
            let zoom = self.zoom_level;
            if let Some(ref mut ed) = self.md_editor {
                ed.draw(buffer, buf_w, buf_h, ex, ey, ew, eh, page_bg, &content_theme, sf, zoom, syntax);
            }
            if self.md_editor_mode == MdEditorMode::Split {
                let div_x = ex + ew as i32;
                let div_top = y0 + hdr_h as i32;
                let div_h = canvas_h.saturating_sub(hdr_h);
                let div_active = self.md_split_drag || self.hovered_widget == Some(UiHit::MdSplitDivider);
                let div_col = if div_active { theme.accent } else { theme.divider };
                fill_rect_clipped(buffer, buf_w, buf_h, div_x, div_top, 1, div_h, div_col);
                if self.ui_skin == super::theme::UiSkin::Distinct && div_h >= 40 {
                    let grip_h = (28.0 * s).round() as u32;
                    let grip_w = (4.0 * s).round().max(3.0) as u32;
                    let grip_y = div_top + ((div_h.saturating_sub(grip_h)) / 2) as i32;
                    let grip_col = if div_active { theme.accent } else { theme.text_muted };
                    fill_rounded_rect(
                        buffer,
                        buf_w,
                        buf_h,
                        div_x - (grip_w as i32) / 2,
                        grip_y,
                        grip_w,
                        grip_h,
                        grip_w as f32 * 0.5,
                        grip_col,
                    );
                }
            }
        }
    }

    pub(super) fn handle_editor_bar_action(&mut self, action: super::text_editor::EditorBarAction, editor_w: u32, editor_h: f32, sf: f64) {
        use super::text_editor::EditorBarAction;
        match action {
            EditorBarAction::CodeFontDown => {
                self.md_code_font_px = (self.md_code_font_px - 1.0).clamp(10.0, 28.0);
                if let Some(ref mut ed) = self.md_editor {
                    ed.font_size_px = self.md_code_font_px;
                }
                self.status_notice = Some(format!("Code Font: {:.0} px", self.md_code_font_px));
                self.save_session_to_ini();
            }
            EditorBarAction::CodeFontUp => {
                self.md_code_font_px = (self.md_code_font_px + 1.0).clamp(10.0, 28.0);
                if let Some(ref mut ed) = self.md_editor {
                    ed.font_size_px = self.md_code_font_px;
                }
                self.status_notice = Some(format!("Code Font: {:.0} px", self.md_code_font_px));
                self.save_session_to_ini();
            }
            EditorBarAction::ModeSplit => {
                if !Self::is_epub_path(&self.file_path) && !Self::is_word_path(&self.file_path) {
                    self.md_editor_mode = MdEditorMode::Split;
                    self.ensure_md_editor_initialized();
                    self.md_layout = None;
                }
            }
            EditorBarAction::ModeEdit => {
                if !Self::is_epub_path(&self.file_path) && !Self::is_word_path(&self.file_path) {
                    self.md_editor_mode = MdEditorMode::Edit;
                    self.ensure_md_editor_initialized();
                }
            }
            EditorBarAction::ModePreview => {
                self.md_editor_mode = MdEditorMode::Preview;
                self.md_layout = None;
            }
            EditorBarAction::PopOutOrDock => {
                if self.editor_popout.is_some() {
                    self.close_editor_popout(true);
                    return;
                } else if matches!(self.active_doc_kind(), super::theme::DocKind::Markdown | super::theme::DocKind::LaTeX) {
                    self.ensure_md_editor_initialized();
                    self.pending_open_editor_popout = true;
                }
            }
            _ => {
                self.ensure_md_editor_initialized();
                let zoom = self.zoom_level;
                let changed = if let Some(ref mut ed) = self.md_editor {
                    let c = ed.apply_editor_bar_action(action);
                    ed.ensure_cursor_visible(editor_w.max(120), editor_h.max(120.0), sf, zoom);
                    c
                } else {
                    false
                };
                if changed {
                    self.sync_markdown_from_editor();
                }
                return;
            }
        }
        self.request_redraw();
    }

    pub(super) fn hit_test_markdown_view(&self, mx: f64, my: f64) -> Option<MarkdownHit> {
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        for h in self.md_active_hits.iter().rev() {
            if h.contains(ix, iy) {
                return Some(h.hit);
            }
        }
        None
    }

    pub(super) fn handle_markdown_click(&mut self, hit: MarkdownHit) {
        match hit {
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

#[derive(Clone, Copy)]
struct BookDrawCtx<'b> {
    book_mode: bool,
    font_family: FontFamily,
    highlights: &'b [EpubHighlight],
    sel_range: Option<((usize, usize), (usize, usize))>,
    accent_color: u32,
    search_matches: &'b [MdSearchMatch],
    active_search_match: Option<usize>,
}

impl<'b> Default for BookDrawCtx<'b> {
    fn default() -> Self {
        Self {
            book_mode: false,
            font_family: FontFamily::UiSans,
            highlights: &[],
            sel_range: None,
            accent_color: 0x0025_63EB,
            search_matches: &[],
            active_search_match: None,
        }
    }
}

/// Renders a complete Distinct EPUB spread (either centered Cover page or 2-Page Open Book)
/// into `buffer` (`buf_w x buf_h`) at `(spread_x, spread_y, spread_w, spread_h)`.
/// Used both for live frame rendering and for capturing the outgoing spread snapshot on frame 0
/// of the Apple Books 3D page-curl animation.
#[allow(clippy::too_many_arguments)]
fn render_distinct_epub_spread_surface(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    spread_x: i32,
    spread_y: i32,
    spread_w: u32,
    spread_h: u32,
    top_pad: i32,
    bot_pad: i32,
    outer_pad: i32,
    inner_pad: i32,
    scroll_y: f32,
    cache: &MdLayoutCache,
    embedded_images: &HashMap<String, Vec<u8>>,
    image_cache: &mut MdDecodedImageCache,
    cover_thumb: Option<&(String, u32, u32, Vec<u32>)>,
    book_meta_title: &str,
    book_meta_author: Option<&str>,
    book_meta_pub: Option<&str>,
    bookmarks: &[super::book_ui::EpubBookmark],
    desk_bg: u32,
    page_bg: u32,
    content_theme: &UiTheme,
    hovered_widget: &Option<UiHit>,
    s: f32,
    mut hits: Option<&mut Vec<MdHitRect>>,
    mut word_hits: Option<&mut Vec<EpubWordHit>>,
    rctx_base: BookDrawCtx<'_>,
    draw_outer_shadow: bool,
) {
    let total_pages = cache.book_pages.len().max(1);
    let num_spreads = md_magazine_total_spreads(total_pages);
    let mut page_idx = 0usize;
    for (i, pg) in cache.book_pages.iter().enumerate() {
        if pg.start_y <= scroll_y + 2.0 {
            page_idx = i;
        } else {
            break;
        }
    }
    let spread_idx = md_magazine_spread_for_page(page_idx).min(num_spreads.saturating_sub(1));
    let (left_idx, right_idx_opt) = md_magazine_pages_for_spread(spread_idx, total_pages);
    let page_top = spread_y + top_pad;
    let page_bot = (spread_y + spread_h as i32 - bot_pad - (22.0 * s).round() as i32).max(page_top + 40);
    let num_sz = (10.5 * s).clamp(9.0, 17.0);
    let num_y = spread_y + spread_h as i32 - (22.0 * s).round() as i32;
    let total_content = total_pages.saturating_sub(1).max(1);

    if spread_idx == 0 && cache.book_pages.get(left_idx).is_some_and(|p| p.is_cover) {
        if !draw_outer_shadow {
            fill_rect_clipped(buffer, buf_w, buf_h, spread_x, spread_y, spread_w, spread_h, desk_bg);
        }
        let full_cover_key = cover_thumb
            .map(|(k, _, _, _)| k.as_str())
            .filter(|k| embedded_images.contains_key(*k));

        let mut drew_full_cover = false;
        if let Some(ckey) = full_cover_key {
            let nat_dims = embedded_images.get(ckey).and_then(|bytes| {
                image::ImageReader::new(std::io::Cursor::new(bytes))
                    .with_guessed_format()
                    .ok()
                    .and_then(|r| r.into_dimensions().ok())
            });
            let (nw, nh) = nat_dims.filter(|&(w, h)| w > 0 && h > 0).unwrap_or((600, 900));
            let aspect = (nw as f32 / nh as f32).clamp(0.42, 1.55);
            let max_h = spread_h.max(64);
            let max_w = spread_w.max(64);
            let mut dh = max_h as f32;
            let mut dw = dh * aspect;
            if dw > max_w as f32 {
                dw = max_w as f32;
                dh = (dw / aspect).min(max_h as f32);
            }
            let cover_w = dw.round().max(1.0) as u32;
            let cover_h = dh.round().max(1.0) as u32;
            let cover_x = spread_x + ((spread_w.saturating_sub(cover_w)) / 2) as i32;
            let cover_y = spread_y + ((spread_h.saturating_sub(cover_h)) / 2) as i32;

            if let Some((sw, sh, src_px)) =
                ensure_md_image_decoded(ckey, cover_w, cover_h, embedded_images, image_cache)
            {
                if draw_outer_shadow {
                    draw_soft_shadow(
                        buffer,
                        buf_w,
                        buf_h,
                        cover_x,
                        cover_y,
                        cover_w,
                        cover_h,
                        4.0 * s,
                        (16.0 * s).round().max(4.0) as i32,
                        (6.0 * s).round().max(2.0) as i32,
                        (0, 0, 0),
                        82,
                    );
                }
                for dy in 0..cover_h {
                    let py = cover_y + dy as i32;
                    if py < 0 || py >= buf_h as i32 {
                        continue;
                    }
                    let sy = if *sh == cover_h {
                        dy as usize
                    } else {
                        ((dy as u64 * *sh as u64) / cover_h as u64).min(*sh as u64 - 1) as usize
                    };
                    let dst_row = py as usize * buf_w as usize;
                    let src_row = sy * (*sw as usize);
                    for dx in 0..cover_w {
                        let px_x = cover_x + dx as i32;
                        if px_x < 0 || px_x >= buf_w as i32 {
                            continue;
                        }
                        let sx = if *sw == cover_w {
                            dx as usize
                        } else {
                            ((dx as u64 * *sw as u64) / cover_w as u64).min(*sw as u64 - 1) as usize
                        };
                        buffer[dst_row + px_x as usize] = src_px[src_row + sx];
                    }
                }
                // Subtle bookbinding groove near the left edge + crisp border
                let groove_w = (2.0 * s).round().max(1.0) as u32;
                let groove_x = (cover_x + (10.0 * s).round().max(4.0) as i32).max(0) as u32;
                let groove_y = cover_y.max(0) as u32;
                super::render::blend_rect_rgba(buffer, buf_w, buf_h, groove_x, groove_y, groove_w, cover_h, (0, 0, 0, 34));
                super::render::blend_rect_rgba(buffer, buf_w, buf_h, groove_x + groove_w, groove_y, 1, cover_h, (255, 255, 255, 28));
                stroke_rounded_rect(buffer, buf_w, buf_h, cover_x, cover_y, cover_w, cover_h, 2.0 * s, content_theme.border_subtle);
                drew_full_cover = true;
            }
        }

        if !drew_full_cover {
            let cover_w = (spread_w / 2)
                .max(cache.col_w + (40.0 * s).round() as u32)
                .min(spread_w);
            let cover_x = spread_x + ((spread_w.saturating_sub(cover_w)) / 2) as i32;

            if draw_outer_shadow {
                draw_soft_shadow(
                    buffer,
                    buf_w,
                    buf_h,
                    cover_x,
                    spread_y,
                    cover_w,
                    spread_h,
                    4.0 * s,
                    (14.0 * s).round().max(4.0) as i32,
                    (5.0 * s).round().max(2.0) as i32,
                    (0, 0, 0),
                    72,
                );
            }
            fill_rounded_rect(buffer, buf_w, buf_h, cover_x, spread_y, cover_w, spread_h, 4.0 * s, page_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, cover_x, spread_y, cover_w, spread_h, 4.0 * s, content_theme.border_subtle);

            draw_distinct_epub_cover_card(
                buffer,
                buf_w,
                buf_h,
                cover_x,
                page_top,
                cover_w,
                (page_bot - page_top).max(60) as u32,
                None,
                book_meta_title,
                book_meta_author,
                book_meta_pub,
                cache.book_font_family,
                content_theme,
                s,
            );
            let lbl = "Cover";
            let f_style = TextStyle {
                family: cache.book_font_family,
                size_px: num_sz,
                bold: false,
                italic: true,
            };
            let lw = measure_text_styled(lbl, f_style);
            draw_text_styled_clipped(
                buffer,
                buf_w,
                buf_h,
                cover_x + ((cover_w as f32 - lw) * 0.5).round() as i32,
                num_y,
                cover_x + cover_w as i32 - 8,
                lbl,
                f_style,
                content_theme.text_muted,
            );
        }
    } else {
        let spine_x = spread_x + (spread_w / 2) as i32;
        if draw_outer_shadow {
            draw_soft_shadow(
                buffer,
                buf_w,
                buf_h,
                spread_x,
                spread_y,
                spread_w,
                spread_h,
                4.0 * s,
                (14.0 * s).round().max(4.0) as i32,
                (5.0 * s).round().max(2.0) as i32,
                (0, 0, 0),
                70,
            );
        }
        fill_rounded_rect(buffer, buf_w, buf_h, spread_x, spread_y, spread_w, spread_h, 4.0 * s, page_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, spread_x, spread_y, spread_w, spread_h, 4.0 * s, content_theme.border_subtle);

        // Asymmetric symmetric-book margins: Left page respects `outer_pad` on left and `inner_pad` on spine;
        // Right page respects `inner_pad` on spine and `outer_pad` on right!
        let left_col_x = spread_x + outer_pad.max(12);
        let right_col_x = spine_x + inner_pad.max(10);

        let mut scratch_hits = Vec::new();
        let mut scratch_words = Vec::new();

        if let Some(left_pg) = cache.book_pages.get(left_idx) {
            draw_book_page_slices(
                buffer,
                buf_w,
                buf_h,
                left_pg,
                &cache.blocks,
                left_col_x,
                cache.col_w as f32,
                page_top,
                page_bot,
                &cache.code_h_scroll,
                embedded_images,
                image_cache,
                content_theme,
                hovered_widget,
                s,
                hits.as_deref_mut().unwrap_or(&mut scratch_hits),
                word_hits.as_deref_mut().unwrap_or(&mut scratch_words),
                rctx_base,
            );
            let l_bookmarked = bookmarks.iter().any(|b| b.page_num == left_idx);
            if l_bookmarked {
                draw_bookmark_corner_ribbon(buffer, buf_w, buf_h, spread_x + (14.0 * s).round() as i32, spread_y, s, content_theme.accent);
            }
            let l_lbl = format!("Loc. {} of {}", left_idx, total_content);
            draw_text_styled_clipped(
                buffer,
                buf_w,
                buf_h,
                left_col_x,
                num_y,
                spine_x - 10,
                &l_lbl,
                TextStyle {
                    family: cache.book_font_family,
                    size_px: num_sz,
                    bold: false,
                    italic: false,
                },
                content_theme.text_muted,
            );
        }

        if let Some(right_idx) = right_idx_opt {
            if let Some(right_pg) = cache.book_pages.get(right_idx) {
                draw_book_page_slices(
                    buffer,
                    buf_w,
                    buf_h,
                    right_pg,
                    &cache.blocks,
                    right_col_x,
                    cache.col_w as f32,
                    page_top,
                    page_bot,
                    &cache.code_h_scroll,
                    embedded_images,
                    image_cache,
                    content_theme,
                    hovered_widget,
                    s,
                    hits.as_deref_mut().unwrap_or(&mut scratch_hits),
                    word_hits.as_deref_mut().unwrap_or(&mut scratch_words),
                    rctx_base,
                );
                let r_bookmarked = bookmarks.iter().any(|b| b.page_num == right_idx);
                if r_bookmarked {
                    draw_bookmark_corner_ribbon(buffer, buf_w, buf_h, spread_x + spread_w as i32 - (26.0 * s).round() as i32, spread_y, s, content_theme.accent);
                }
                let r_lbl = format!("Loc. {} of {}", right_idx, total_content);
                let r_style = TextStyle {
                    family: cache.book_font_family,
                    size_px: num_sz,
                    bold: false,
                    italic: false,
                };
                let rw = measure_text_styled(&r_lbl, r_style);
                draw_text_styled_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    right_col_x + cache.col_w as i32 - rw.round() as i32,
                    num_y,
                    spread_x + spread_w as i32 - 8,
                    &r_lbl,
                    r_style,
                    content_theme.text_muted,
                );
            }
        }

        draw_book_spine_crease(
            buffer,
            buf_w,
            buf_h,
            spine_x,
            spread_y,
            spread_h,
            (18.0 * s).round().max(6.0) as i32,
        );
    }
}

/// Renders a Classic / Studio MagazineBook spread for Markdown & LaTeX into `buffer`
/// and returns `(anim_x, anim_y, anim_w, anim_h)` so the Apple Books 3D curl animation works seamlessly.
#[allow(clippy::too_many_arguments)]
fn render_classic_md_magazine_spread_surface(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    preview_x0: i32,
    preview_w: u32,
    spread_x: i32,
    spread_y: i32,
    spread_w: u32,
    spread_h: u32,
    page_top: i32,
    page_bot: i32,
    scroll_y: f32,
    cache: &MdLayoutCache,
    pages: &[f32],
    embedded_images: &HashMap<String, Vec<u8>>,
    image_cache: &mut MdDecodedImageCache,
    desk_bg: u32,
    page_bg: u32,
    content_theme: &UiTheme,
    hovered_widget: &Option<UiHit>,
    s: f32,
    mut hits: Option<&mut Vec<MdHitRect>>,
    search_ctx: BookDrawCtx<'_>,
    draw_outer_shadow: bool,
) -> (i32, i32, u32, u32) {
    let total_pages = pages.len().max(1);
    let num_spreads = md_magazine_total_spreads(total_pages);
    let mut page_idx = 0usize;
    for (i, &sy) in pages.iter().enumerate() {
        if sy <= scroll_y + 2.0 {
            page_idx = i;
        } else {
            break;
        }
    }
    let spread_idx = md_magazine_spread_for_page(page_idx).min(num_spreads.saturating_sub(1));
    let (left_idx, right_idx_opt) = md_magazine_pages_for_spread(spread_idx, total_pages);
    let num_sz = (10.5 * s).clamp(9.0, 18.0);
    let num_y = spread_y + spread_h as i32 - (20.0 * s).round() as i32;
    let mut scratch_hits = Vec::new();

    if spread_idx == 0 {
        let cover_w = if !draw_outer_shadow {
            spread_w
        } else {
            (spread_w / 2).max(cache.col_w + (32.0 * s).round() as u32).min(preview_w.saturating_sub(16))
        };
        let cover_x = if !draw_outer_shadow {
            spread_x
        } else {
            preview_x0 + ((preview_w.saturating_sub(cover_w)) / 2) as i32
        };

        if !draw_outer_shadow {
            fill_rect_clipped(buffer, buf_w, buf_h, cover_x, spread_y, cover_w, spread_h, desk_bg);
        } else {
            draw_soft_shadow(
                buffer, buf_w, buf_h,
                cover_x, spread_y, cover_w, spread_h,
                3.0 * s, (12.0 * s).round().max(4.0) as i32, (5.0 * s).round().max(2.0) as i32,
                (0, 0, 0), 72,
            );
        }
        fill_rect_clipped(buffer, buf_w, buf_h, cover_x, spread_y, cover_w, spread_h, page_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, cover_x, spread_y, cover_w, 1, content_theme.border_subtle);
        fill_rect_clipped(buffer, buf_w, buf_h, cover_x, spread_y + spread_h as i32 - 1, cover_w, 1, content_theme.border_subtle);
        fill_rect_clipped(buffer, buf_w, buf_h, cover_x, spread_y, 1, spread_h, content_theme.border_subtle);
        fill_rect_clipped(buffer, buf_w, buf_h, cover_x + cover_w as i32 - 1, spread_y, 1, spread_h, content_theme.border_subtle);

        let col_x = cover_x + ((cover_w as i32 - cache.col_w as i32) / 2).max(12);
        let start_y = pages[0];
        let end_y = pages.get(1).copied().unwrap_or(f32::MAX);
        for blk in &cache.blocks {
            let (by, _) = md_block_y_h(blk);
            if by >= start_y - 0.5 && by < end_y - 0.5 {
                draw_laid_out_block(
                    buffer, buf_w, buf_h, blk, col_x, cache.col_w as f32,
                    page_top, page_bot, start_y, start_y, end_y,
                    &cache.code_h_scroll, embedded_images, image_cache, content_theme,
                    hovered_widget, s, hits.as_deref_mut().unwrap_or(&mut scratch_hits),
                    None, search_ctx,
                );
            }
        }
        let lbl = "1";
        let lw = measure_text(lbl, num_sz, false);
        draw_text(buffer, buf_w, buf_h, cover_x + ((cover_w as f32 - lw) * 0.5).round() as i32, num_y, lbl, num_sz, content_theme.text_muted, false);
        (cover_x, spread_y, cover_w, spread_h)
    } else {
        let spine_x = spread_x + (spread_w / 2) as i32;
        if draw_outer_shadow {
            draw_soft_shadow(
                buffer, buf_w, buf_h,
                spread_x, spread_y, spread_w, spread_h,
                3.0 * s, (12.0 * s).round().max(4.0) as i32, (5.0 * s).round().max(2.0) as i32,
                (0, 0, 0), 68,
            );
        }
        fill_rect_clipped(buffer, buf_w, buf_h, spread_x, spread_y, spread_w, spread_h, page_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, spread_x, spread_y, spread_w, 1, content_theme.border_subtle);
        fill_rect_clipped(buffer, buf_w, buf_h, spread_x, spread_y + spread_h as i32 - 1, spread_w, 1, content_theme.border_subtle);
        fill_rect_clipped(buffer, buf_w, buf_h, spread_x, spread_y, 1, spread_h, content_theme.border_subtle);
        fill_rect_clipped(buffer, buf_w, buf_h, spread_x + spread_w as i32 - 1, spread_y, 1, spread_h, content_theme.border_subtle);

        let half_w = (spread_w / 2) as i32;
        let left_col_x = spread_x + ((half_w - cache.col_w as i32) / 2).max(12);
        let right_col_x = spine_x + ((half_w - cache.col_w as i32) / 2).max(12);

        let left_start = pages[left_idx];
        let left_end = pages.get(left_idx + 1).copied().unwrap_or(f32::MAX);
        for blk in &cache.blocks {
            let (by, _) = md_block_y_h(blk);
            if by >= left_start - 0.5 && by < left_end - 0.5 {
                draw_laid_out_block(
                    buffer, buf_w, buf_h, blk, left_col_x, cache.col_w as f32,
                    page_top, page_bot, left_start, left_start, left_end,
                    &cache.code_h_scroll, embedded_images, image_cache, content_theme,
                    hovered_widget, s, hits.as_deref_mut().unwrap_or(&mut scratch_hits),
                    None, search_ctx,
                );
            }
        }
        let l_lbl = format!("{}", left_idx + 1);
        draw_text(buffer, buf_w, buf_h, spread_x + (18.0 * s).round() as i32, num_y, &l_lbl, num_sz, content_theme.text_muted, false);

        if let Some(right_idx) = right_idx_opt {
            let right_start = pages[right_idx];
            let right_end = pages.get(right_idx + 1).copied().unwrap_or(f32::MAX);
            for blk in &cache.blocks {
                let (by, _) = md_block_y_h(blk);
                if by >= right_start - 0.5 && by < right_end - 0.5 {
                    draw_laid_out_block(
                        buffer, buf_w, buf_h, blk, right_col_x, cache.col_w as f32,
                        page_top, page_bot, right_start, right_start, right_end,
                        &cache.code_h_scroll, embedded_images, image_cache, content_theme,
                        hovered_widget, s, hits.as_deref_mut().unwrap_or(&mut scratch_hits),
                        None, search_ctx,
                    );
                }
            }
            let r_lbl = format!("{}", right_idx + 1);
            let rw = measure_text(&r_lbl, num_sz, false);
            draw_text(buffer, buf_w, buf_h, spread_x + spread_w as i32 - (18.0 * s).round() as i32 - rw.round() as i32, num_y, &r_lbl, num_sz, content_theme.text_muted, false);
        }

        draw_book_spine_crease(
            buffer, buf_w, buf_h, spine_x, spread_y, spread_h,
            (16.0 * s).round().max(6.0) as i32,
        );
        (spread_x, spread_y, spread_w, spread_h)
    }
}

fn draw_bookmark_corner_ribbon(buffer: &mut [u32], buf_w: u32, buf_h: u32, x: i32, y: i32, s: f32, color: u32) {
    let rw = (12.0 * s).round().max(8.0) as i32;
    let rh = (22.0 * s).round().max(14.0) as i32;
    for dy in 0..rh {
        let notch = if dy > rh - rw / 2 { (rh - dy) * 2 } else { rw };
        let half_cut = (rw - notch).max(0) / 2;
        for dx in 0..rw {
            if dx >= half_cut && dx < rw - half_cut {
                let px = x + dx;
                let py = y + dy;
                if px >= 0 && px < buf_w as i32 && py >= 0 && py < buf_h as i32 {
                    buffer[py as usize * buf_w as usize + px as usize] = color;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_distinct_epub_cover_card(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    card_x: i32,
    top_y: i32,
    card_w: u32,
    avail_h: u32,
    cover_thumb: Option<&(String, u32, u32, Vec<u32>)>,
    title: &str,
    author: Option<&str>,
    publisher: Option<&str>,
    font_family: FontFamily,
    theme: &UiTheme,
    s: f32,
) {
    let inner_pad = (28.0 * s).round() as i32;
    let max_text_w = (card_w as i32 - inner_pad * 2).max(80) as f32;
    let mut cur_y = top_y + (16.0 * s).round() as i32;

    if let Some((_, sw, sh, px)) = cover_thumb {
        let max_img_w = (card_w as f32 * 0.56).min(300.0 * s).max(110.0);
        let max_img_h = (avail_h as f32 * 0.55).min(380.0 * s).max(120.0);
        let aspect = if *sh > 0 { *sw as f32 / *sh as f32 } else { 0.68 };
        let mut dh = max_img_h;
        let mut dw = dh * aspect;
        if dw > max_img_w {
            dw = max_img_w;
            dh = dw / aspect.max(0.2);
        }
        let dw_u = dw.round().max(1.0) as u32;
        let dh_u = dh.round().max(1.0) as u32;
        let ix = card_x + ((card_w as i32 - dw_u as i32) / 2);
        draw_soft_shadow(buffer, buf_w, buf_h, ix, cur_y, dw_u, dh_u, 2.0 * s, (10.0 * s).round() as i32, (4.0 * s).round() as i32, (0, 0, 0), 68);
        for dy in 0..dh_u {
            let py = cur_y + dy as i32;
            if py < 0 || py >= buf_h as i32 {
                continue;
            }
            let sy = ((dy as u64 * *sh as u64) / dh_u as u64).min(*sh as u64 - 1) as usize;
            let dst_row = py as usize * buf_w as usize;
            let src_row = sy * (*sw as usize);
            for dx in 0..dw_u {
                let px_x = ix + dx as i32;
                if px_x < 0 || px_x >= buf_w as i32 {
                    continue;
                }
                let sx = ((dx as u64 * *sw as u64) / dw_u as u64).min(*sw as u64 - 1) as usize;
                buffer[dst_row + px_x as usize] = px[src_row + sx];
            }
        }
        stroke_rounded_rect(buffer, buf_w, buf_h, ix, cur_y, dw_u, dh_u, 2.0, theme.border_subtle);
        cur_y += dh_u as i32 + (24.0 * s).round() as i32;
    } else {
        cur_y += (avail_h as i32) / 5;
        let plate_w = (card_w as f32 * 0.72).round() as u32;
        let plate_x = card_x + ((card_w as i32 - plate_w as i32) / 2);
        stroke_rounded_rect(buffer, buf_w, buf_h, plate_x, cur_y - (16.0 * s).round() as i32, plate_w, (150.0 * s).round() as u32, 6.0 * s, theme.border_subtle);
    }

    let title_sz = (20.0 * s).clamp(15.0, 32.0);
    let title_style = TextStyle {
        family: font_family,
        size_px: title_sz,
        bold: true,
        italic: false,
    };
    let mut lines = Vec::new();
    let mut cur_line = String::new();
    for word in title.split_whitespace() {
        let cand = if cur_line.is_empty() {
            word.to_string()
        } else {
            format!("{cur_line} {word}")
        };
        if !cur_line.is_empty() && measure_text_styled(&cand, title_style) > max_text_w {
            lines.push(cur_line);
            cur_line = word.to_string();
            if lines.len() >= 3 {
                break;
            }
        } else {
            cur_line = cand;
        }
    }
    if !cur_line.is_empty() && lines.len() < 3 {
        lines.push(cur_line);
    }
    for ln in &lines {
        let lw = measure_text_styled(ln, title_style);
        let tx = card_x + ((card_w as f32 - lw) * 0.5).round() as i32;
        draw_text_styled_clipped(buffer, buf_w, buf_h, tx, cur_y, card_x + card_w as i32 - 8, ln, title_style, theme.text_primary);
        cur_y += (title_sz * 1.32).round() as i32;
    }

    cur_y += (8.0 * s).round() as i32;
    let orn_w = (84.0 * s).round() as u32;
    let orn_x = card_x + ((card_w as i32 - orn_w as i32) / 2);
    fill_rect_clipped(buffer, buf_w, buf_h, orn_x, cur_y, orn_w, 1, theme.divider);
    cur_y += (12.0 * s).round() as i32;

    if let Some(auth) = author {
        let a_sz = (13.5 * s).clamp(11.0, 22.0);
        let a_style = TextStyle {
            family: font_family,
            size_px: a_sz,
            bold: false,
            italic: true,
        };
        let a_clip = truncate_ellipsis_styled(auth, max_text_w, a_style);
        let aw = measure_text_styled(&a_clip, a_style);
        let ax = card_x + ((card_w as f32 - aw) * 0.5).round() as i32;
        draw_text_styled_clipped(buffer, buf_w, buf_h, ax, cur_y, card_x + card_w as i32 - 8, &a_clip, a_style, theme.text_secondary);
        cur_y += (a_sz * 1.45).round() as i32;
    }
    if let Some(pub_name) = publisher {
        let p_sz = (11.0 * s).clamp(9.5, 16.0);
        let p_style = TextStyle {
            family: font_family,
            size_px: p_sz,
            bold: false,
            italic: false,
        };
        let p_clip = truncate_ellipsis_styled(pub_name, max_text_w, p_style);
        let pw = measure_text_styled(&p_clip, p_style);
        let px_x = card_x + ((card_w as f32 - pw) * 0.5).round() as i32;
        draw_text_styled_clipped(buffer, buf_w, buf_h, px_x, cur_y, card_x + card_w as i32 - 8, &p_clip, p_style, theme.text_muted);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_book_page_slices(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    page: &BookPage,
    blocks: &[LaidOutBlock],
    col_x: i32,
    col_w: f32,
    page_top: i32,
    page_bot: i32,
    code_h_scroll: &[f32],
    embedded_images: &HashMap<String, Vec<u8>>,
    image_cache: &mut MdDecodedImageCache,
    theme: &UiTheme,
    hovered: &Option<UiHit>,
    s: f32,
    hits: &mut Vec<MdHitRect>,
    mut word_hits: &mut Vec<EpubWordHit>,
    rctx: BookDrawCtx<'_>,
) {
    let mut cur_y = 0.0f32;
    for (si, slice) in page.slices.iter().enumerate() {
        let Some(blk) = blocks.get(slice.block_idx) else {
            continue;
        };
        let (by, bh) = md_block_y_h(blk);
        let next_gap = blocks
            .get(slice.block_idx + 1)
            .map(|nb| {
                let (ny, _) = md_block_y_h(nb);
                (ny - (by + bh)).clamp(0.0, 40.0)
            })
            .unwrap_or(0.0);

        match blk {
            LaidOutBlock::Heading { level, lines, line_h, font_sz, ornamental, .. } => {
                let end = slice.line_end.min(lines.len());
                let start = slice.line_start.min(end);
                let sy = page_top + cur_y.round() as i32;
                let align = if *ornamental { MdAlign::Center } else { MdAlign::Left };
                draw_wrapped_lines(
                    buffer, buf_w, buf_h, &lines[start..end], col_x, sy, *line_h, *font_sz,
                    page_top, page_bot, theme.text_primary, theme, align, col_w, hits,
                    Some(&mut word_hits), rctx.with_justify(false),
                );
                let lines_h = (end - start) as f32 * *line_h;
                let extra = if end == lines.len() { (bh - lines.len() as f32 * *line_h).max(0.0) } else { 0.0 };
                if end == lines.len() && *ornamental {
                    let div_y = sy + (lines_h + 7.0 * s).round() as i32;
                    if div_y >= page_top && div_y < page_bot {
                        let rw = (col_w * 0.28).round().max(24.0) as u32;
                        let cx = col_x + (col_w * 0.5).round() as i32;
                        fill_rect_clipped(buffer, buf_w, buf_h, cx - rw as i32 - (10.0 * s).round() as i32, div_y, rw, 1, theme.divider);
                        fill_rect_clipped(buffer, buf_w, buf_h, cx + (10.0 * s).round() as i32, div_y, rw, 1, theme.divider);
                        let ds = (3.0 * s).round().max(2.0) as i32;
                        fill_rect_clipped(buffer, buf_w, buf_h, cx - ds / 2, div_y - ds / 2, ds as u32, ds as u32, theme.accent);
                    }
                } else if end == lines.len() && *level <= 2 {
                    let div_y = sy + (lines_h + 4.0 * s).round() as i32;
                    if div_y >= page_top && div_y < page_bot {
                        fill_rect_clipped(buffer, buf_w, buf_h, col_x, div_y, col_w.round() as u32, 1, theme.divider);
                    }
                }
                cur_y += lines_h + extra + if end == lines.len() { next_gap } else { 0.0 };
            }
            LaidOutBlock::Paragraph { lines, line_h, font_sz, justify, .. } => {
                let end = slice.line_end.min(lines.len());
                let start = slice.line_start.min(end);
                let sy = page_top + cur_y.round() as i32;
                draw_wrapped_lines(
                    buffer, buf_w, buf_h, &lines[start..end], col_x, sy, *line_h, *font_sz,
                    page_top, page_bot, theme.text_primary, theme, MdAlign::Left, col_w, hits,
                    Some(&mut word_hits), rctx.with_justify(*justify),
                );
                cur_y += (end - start) as f32 * *line_h + if end == lines.len() { next_gap } else { 0.0 };
            }
            LaidOutBlock::ListItem { indent, marker, lines, line_h, font_sz, justify, .. } => {
                let end = slice.line_end.min(lines.len());
                let start = slice.line_start.min(end);
                let sy = page_top + cur_y.round() as i32;
                let text_x = col_x + indent.round() as i32;
                if start == 0 && sy >= page_top && sy + (*line_h).round() as i32 <= page_bot {
                    draw_list_marker(buffer, buf_w, buf_h, marker, text_x, sy, *line_h, *font_sz, theme, s);
                }
                draw_wrapped_lines(
                    buffer, buf_w, buf_h, &lines[start..end], text_x, sy, *line_h, *font_sz,
                    page_top, page_bot, theme.text_primary, theme, MdAlign::Left, (col_w - *indent).max(60.0), hits,
                    Some(&mut word_hits), rctx.with_justify(*justify),
                );
                cur_y += (end - start) as f32 * *line_h + if end == lines.len() { next_gap } else { 0.0 };
            }
            _ => {
                let virtual_scroll = by - cur_y;
                draw_laid_out_block(
                    buffer, buf_w, buf_h, blk, col_x, col_w,
                    page_top, page_bot, virtual_scroll, by - 1.0, by + bh + 1.0,
                    code_h_scroll, embedded_images, image_cache, theme, hovered, s, hits,
                    Some(&mut word_hits), rctx,
                );
                cur_y += bh + if si + 1 < page.slices.len() { next_gap } else { 0.0 };
            }
        }
    }
}

#[derive(Clone, Copy)]
struct BookDrawCtxWithJustify<'a> {
    book_mode: bool,
    font_family: FontFamily,
    justify: bool,
    highlights: &'a [EpubHighlight],
    sel_range: Option<((usize, usize), (usize, usize))>,
    accent_color: u32,
    search_matches: &'a [MdSearchMatch],
    active_search_match: Option<usize>,
}

impl<'a> BookDrawCtx<'a> {
    fn with_justify(self, justify: bool) -> BookDrawCtxWithJustify<'a> {
        BookDrawCtxWithJustify {
            book_mode: self.book_mode,
            font_family: self.font_family,
            justify,
            highlights: self.highlights,
            sel_range: self.sel_range,
            accent_color: self.accent_color,
            search_matches: self.search_matches,
            active_search_match: self.active_search_match,
        }
    }
}

fn blend_rect_clipped(buffer: &mut [u32], buf_w: u32, buf_h: u32, x: i32, y: i32, w: u32, h: u32, rgb: u32, alpha: u8) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(buf_w as i32);
    let y1 = (y + h as i32).min(buf_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let a = alpha as u32;
    let inv = 255 - a;
    let sr = (rgb >> 16) & 0xFF;
    let sg = (rgb >> 8) & 0xFF;
    let sb = rgb & 0xFF;
    for py in y0..y1 {
        let row = py as usize * buf_w as usize;
        for px in x0..x1 {
            let dst = buffer[row + px as usize];
            let dr = (dst >> 16) & 0xFF;
            let dg = (dst >> 8) & 0xFF;
            let db = dst & 0xFF;
            let r = (sr * a + dr * inv) / 255;
            let g = (sg * a + dg * inv) / 255;
            let b = (sb * a + db * inv) / 255;
            buffer[row + px as usize] = (r << 16) | (g << 8) | b;
        }
    }
}

fn draw_list_marker(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    marker: &ListMarker,
    text_x: i32,
    sy: i32,
    line_h: f32,
    font_sz: f32,
    theme: &UiTheme,
    s: f32,
) {
    match marker {
        ListMarker::Bullet(depth) => {
            let mx = text_x - (14.0 * s).round() as i32;
            let sym = if *depth % 2 == 0 { "•" } else { "◦" };
            draw_text(buffer, buf_w, buf_h, mx, sy, sym, font_sz, theme.accent, true);
        }
        ListMarker::Ordered(num) => {
            let nw = measure_text(num, font_sz, true);
            let mx = text_x - nw.round() as i32 - (6.0 * s).round() as i32;
            draw_text(buffer, buf_w, buf_h, mx, sy, num, font_sz, theme.text_muted, true);
        }
        ListMarker::Task(checked) => {
            let cb_sz = (12.0 * s).round() as u32;
            let cx = text_x - cb_sz as i32 - (6.0 * s).round() as i32;
            let cy = sy + (((line_h - cb_sz as f32) * 0.4).round() as i32).max(1);
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
    embedded_images: &HashMap<String, Vec<u8>>,
    image_cache: &mut MdDecodedImageCache,
    theme: &UiTheme,
    hovered: &Option<UiHit>,
    s: f32,
    hits: &mut Vec<MdHitRect>,
    mut word_hits: Option<&mut Vec<EpubWordHit>>,
    rctx: BookDrawCtx<'_>,
) {
    match blk {
        LaidOutBlock::Heading { y, h, level, lines, line_h, font_sz, ornamental, .. } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let align = if *ornamental { MdAlign::Center } else { MdAlign::Left };
            draw_wrapped_lines(
                buffer, buf_w, buf_h, lines, col_x, sy, *line_h, *font_sz,
                clip_top, clip_bot, theme.text_primary, theme, align, col_w, hits,
                word_hits, rctx.with_justify(false),
            );
            if *ornamental {
                let div_y = sy + (lines.len() as f32 * *line_h + 7.0 * s).round() as i32;
                if div_y >= clip_top && div_y < clip_bot {
                    let rw = (col_w * 0.28).round().max(24.0) as u32;
                    let cx = col_x + (col_w * 0.5).round() as i32;
                    fill_rect_clipped(buffer, buf_w, buf_h, cx - rw as i32 - (10.0 * s).round() as i32, div_y, rw, 1, theme.divider);
                    fill_rect_clipped(buffer, buf_w, buf_h, cx + (10.0 * s).round() as i32, div_y, rw, 1, theme.divider);
                    let ds = (3.0 * s).round().max(2.0) as i32;
                    fill_rect_clipped(buffer, buf_w, buf_h, cx - ds / 2, div_y - ds / 2, ds as u32, ds as u32, theme.accent);
                }
            } else if *level <= 2 {
                let div_y = sy + (lines.len() as f32 * *line_h + 4.0 * s).round() as i32;
                if div_y >= clip_top && div_y < clip_bot {
                    fill_rect_clipped(buffer, buf_w, buf_h, col_x, div_y, col_w.round() as u32, 1, theme.divider);
                }
            }
        }
        LaidOutBlock::Paragraph { y, h, lines, line_h, font_sz, justify, .. } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            draw_wrapped_lines(
                buffer, buf_w, buf_h, lines, col_x, sy, *line_h, *font_sz,
                clip_top, clip_bot, theme.text_primary, theme, MdAlign::Left, col_w, hits,
                word_hits, rctx.with_justify(*justify),
            );
        }
        LaidOutBlock::CodeBlock { y, h, code_idx, lang, lines, line_h, font_sz, max_line_w, .. } => {
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
                    if rctx.book_mode {
                        draw_text_styled_clipped(buffer, buf_w, buf_h, text_x.max(col_x + 8), ly, col_x + cw as i32 - 8, line, TextStyle::mono(*font_sz), theme.text_primary);
                    } else {
                        draw_text_clipped(buffer, buf_w, buf_h, text_x.max(col_x + 8), ly, col_x + cw as i32 - 8, line, *font_sz, theme.text_primary, false);
                    }
                }
            }
        }
        LaidOutBlock::Blockquote { y, h, inner, .. } => {
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
                draw_laid_out_block(
                    buffer, buf_w, buf_h, child, col_x + (16.0 * s).round() as i32,
                    col_w - 24.0 * s, clip_top, clip_bot, scroll_y, view_top, view_bot,
                    code_h_scroll, embedded_images, image_cache, theme, hovered, s, hits,
                    word_hits.as_deref_mut(), rctx,
                );
            }
        }
        LaidOutBlock::Table { y, h, col_widths, alignments, headers, header_h, rows, line_h, font_sz, .. } => {
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
                draw_wrapped_lines(
                    buffer, buf_w, buf_h, cell_lines, cx + pad_x.round() as i32,
                    sy + (7.0 * s).round() as i32, *line_h, *font_sz, clip_top, clip_bot,
                    theme.text_primary, theme, align, (cw - pad_x * 2.0).max(20.0), hits,
                    None, rctx.with_justify(false),
                );
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
                        draw_wrapped_lines(
                            buffer, buf_w, buf_h, cell_lines, rcx + pad_x.round() as i32,
                            ry + (7.0 * s).round() as i32, *line_h, *font_sz, clip_top, clip_bot,
                            theme.text_primary, theme, align, (cw - pad_x * 2.0).max(20.0), hits,
                            None, rctx.with_justify(false),
                        );
                        rcx += cw.round() as i32;
                    }
                }
                ry += rih as i32;
            }
        }
        LaidOutBlock::ListItem { y, h, indent, marker, lines, line_h, font_sz, justify, .. } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let text_x = col_x + indent.round() as i32;
            if sy >= clip_top && sy + (*line_h).round() as i32 <= clip_bot {
                draw_list_marker(buffer, buf_w, buf_h, marker, text_x, sy, *line_h, *font_sz, theme, s);
            }
            draw_wrapped_lines(
                buffer, buf_w, buf_h, lines, text_x, sy, *line_h, *font_sz,
                clip_top, clip_bot, theme.text_primary, theme, MdAlign::Left,
                (col_w - *indent).max(60.0), hits, word_hits, rctx.with_justify(*justify),
            );
        }
        LaidOutBlock::HorizontalRule { y, h, .. } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y + h * 0.5 - scroll_y).round() as i32;
            if sy >= clip_top && sy < clip_bot {
                fill_rect_clipped(buffer, buf_w, buf_h, col_x, sy, col_w.round() as u32, 1, theme.divider);
            }
        }
        LaidOutBlock::PageBreak { y, .. } => {
            if *y + 18.0 * s < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y + 9.0 * s - scroll_y).round() as i32;
            if sy >= clip_top && sy < clip_bot {
                let rw = (col_w * 0.22).round().max(24.0) as u32;
                let cx = col_x + (col_w * 0.5).round() as i32;
                fill_rect_clipped(buffer, buf_w, buf_h, cx - (rw as i32) / 2, sy, rw, 1, theme.divider);
            }
        }
        LaidOutBlock::MathBlock { y, h, tex, font_sz, .. } => {
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
            let (body_tex, eq_no) = split_math_eq_no(tex);
            let nodes = crate::math_render::parse_math_nodes(body_tex);
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
            if let Some(num_lbl) = eq_no {
                let num_sz = (*font_sz * 0.86).clamp(9.5, 22.0);
                let nw = measure_text(num_lbl, num_sz, false);
                let nx = col_x + col_w.round() as i32 - nw.round() as i32 - (12.0 * s).round() as i32;
                if sy >= clip_top && sy + box_h as i32 <= clip_bot {
                    draw_text_vcenter(buffer, buf_w, buf_h, nx, sy, box_h, col_x + col_w.round() as i32 - 4, num_lbl, num_sz, theme.text_muted, false);
                }
            }
        }
        LaidOutBlock::Image { y, h, w_px, h_px, caption, fallback_path, font_sz, is_cover, .. } => {
            if y + h < view_top || *y > view_bot {
                return;
            }
            let sy = clip_top + (*y - scroll_y).round() as i32;
            let ix = col_x + ((col_w - *w_px as f32) * 0.5).max(0.0).round() as i32;
            let dw = (*w_px).max(1);
            let dh = (*h_px).max(1);
            if let Some((sw, sh, src_px)) = ensure_md_image_decoded(fallback_path, dw, dh, embedded_images, image_cache) {
                if *is_cover {
                    draw_soft_shadow(
                        buffer,
                        buf_w,
                        buf_h,
                        ix,
                        sy,
                        dw,
                        dh,
                        2.0 * s,
                        (10.0 * s).round().max(4.0) as i32,
                        (4.0 * s).round().max(2.0) as i32,
                        (0, 0, 0),
                        72,
                    );
                }
                if *sw == dw && *sh == dh {
                    for dy in 0..dh {
                        let py = sy + dy as i32;
                        if py < clip_top || py >= clip_bot || py >= buf_h as i32 {
                            continue;
                        }
                        let row_off = py as usize * buf_w as usize;
                        let src_row = dy as usize * (*sw as usize);
                        for dx in 0..dw {
                            let px_x = ix + dx as i32;
                            if px_x < 0 || px_x >= buf_w as i32 {
                                continue;
                            }
                            buffer[row_off + px_x as usize] = src_px[src_row + dx as usize];
                        }
                    }
                } else {
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
                }
                let bx0 = ix.max(0);
                let bx1 = (ix + dw as i32).min(buf_w as i32);
                let by0 = sy.max(clip_top);
                let by1 = (sy + dh as i32).min(clip_bot).min(buf_h as i32);
                if bx1 > bx0 && by1 > by0 {
                    let bw = (bx1 - bx0) as u32;
                    let bh = (by1 - by0) as u32;
                    let bcol = theme.border_subtle;
                    if sy >= clip_top && sy < clip_bot {
                        fill_rect_clipped(buffer, buf_w, buf_h, bx0, sy, bw, 1, bcol);
                    }
                    let bot_edge = sy + dh as i32 - 1;
                    if bot_edge >= clip_top && bot_edge < clip_bot {
                        fill_rect_clipped(buffer, buf_w, buf_h, bx0, bot_edge, bw, 1, bcol);
                    }
                    if ix >= 0 && ix < buf_w as i32 {
                        fill_rect_clipped(buffer, buf_w, buf_h, ix, by0, 1, bh, bcol);
                    }
                    let right_edge = ix + dw as i32 - 1;
                    if right_edge >= 0 && right_edge < buf_w as i32 {
                        fill_rect_clipped(buffer, buf_w, buf_h, right_edge, by0, 1, bh, bcol);
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
    mut word_hits: Option<&mut Vec<EpubWordHit>>,
    rctx: BookDrawCtxWithJustify<'_>,
) {
    for (li, line) in lines.iter().enumerate() {
        let ly = y + (li as f32 * line_h).round() as i32;
        if ly < clip_top || ly + line_h.round() as i32 > clip_bot {
            continue;
        }
        let align_dx = match align {
            MdAlign::Center => ((max_w - line.total_w) * 0.5).max(0.0),
            MdAlign::Right => (max_w - line.total_w).max(0.0),
            MdAlign::Left | MdAlign::None => line.first_line_indent,
        };
        let mut extra_per_space = 0.0f32;
        if rctx.justify && !line.is_last_in_para && matches!(align, MdAlign::Left | MdAlign::None) {
            let n_spaces = line.spans.iter().filter(|w| w.text == " ").count();
            let slack = (max_w - line.first_line_indent - line.total_w).max(0.0);
            if n_spaces > 0 && slack > 0.4 && slack <= max_w * 0.42 {
                extra_per_space = slack / n_spaces as f32;
            }
        }

        let mut cx = x as f32 + align_dx;
        let span_count = line.spans.len();
        for (wi, w) in line.spans.iter().enumerate() {
            let step_w = if w.text == " " { w.w + extra_per_space } else { w.w };
            let is_space = w.text == " ";
            let band_h = (font_sz * 1.24).round().max(12.0) as u32;
            let band_y = ly - (font_sz * 0.08).round() as i32;

            // Highlight full-text search matches in EPUB, Markdown, and LaTeX
            if !rctx.search_matches.is_empty() {
                let mut hit_active = false;
                let mut hit_any = false;
                let check_match = |blk_idx: usize, word_idx: usize| -> (bool, bool) {
                    let mut any = false;
                    let mut active = false;
                    for (mi, sm) in rctx.search_matches.iter().enumerate() {
                        if sm.ast_block_idx == blk_idx
                            && (sm.word_start == usize::MAX
                                || (word_idx >= sm.word_start && word_idx <= sm.word_end))
                        {
                            any = true;
                            if rctx.active_search_match == Some(mi) {
                                active = true;
                                break;
                            }
                        }
                    }
                    (any, active)
                };
                if !is_space && w.word_idx != usize::MAX {
                    let (a, act) = check_match(w.ast_block_idx, w.word_idx);
                    hit_any = a;
                    hit_active = act;
                } else if is_space && wi > 0 && wi + 1 < span_count {
                    let prev = &line.spans[wi - 1];
                    let next = &line.spans[wi + 1];
                    if prev.word_idx != usize::MAX && next.word_idx != usize::MAX {
                        let (pa, pact) = check_match(prev.ast_block_idx, prev.word_idx);
                        let (na, nact) = check_match(next.ast_block_idx, next.word_idx);
                        hit_any = pa && na;
                        hit_active = pact && nact;
                    }
                }
                if hit_active {
                    blend_rect_clipped(buffer, buf_w, buf_h, cx.round() as i32, band_y, step_w.ceil().max(1.0) as u32, band_h, 0x00F9_7316, 138);
                } else if hit_any {
                    blend_rect_clipped(buffer, buf_w, buf_h, cx.round() as i32, band_y, step_w.ceil().max(1.0) as u32, band_h, 0x00FA_CC15, 96);
                }
            }

            // Check active EPUB highlights / drag selection behind word (and bridging spaces)
            if rctx.book_mode {
                let mut hl_color: Option<u32> = None;
                let check_pos = |pos: (usize, usize)| -> Option<u32> {
                    for hl in rctx.highlights {
                        if pos.0 == hl.block_idx && pos.1 >= hl.start_word && pos.1 <= hl.end_word {
                            return Some(
                                EPUB_HIGHLIGHT_COLORS
                                    .get(hl.color_idx as usize)
                                    .map(|c| c.0)
                                    .unwrap_or(EPUB_HIGHLIGHT_COLORS[0].0),
                            );
                        }
                    }
                    None
                };
                let pos_in_sel = |pos: (usize, usize)| -> bool {
                    rctx.sel_range.is_some_and(|(a, b)| pos >= a && pos <= b)
                };

                let selected = if !is_space && w.word_idx != usize::MAX {
                    hl_color = check_pos((w.ast_block_idx, w.word_idx));
                    pos_in_sel((w.ast_block_idx, w.word_idx))
                } else if is_space && wi > 0 && wi + 1 < span_count {
                    let prev = &line.spans[wi - 1];
                    let next = &line.spans[wi + 1];
                    if prev.word_idx != usize::MAX && next.word_idx != usize::MAX {
                        hl_color = check_pos((prev.ast_block_idx, prev.word_idx))
                            .filter(|&c| check_pos((next.ast_block_idx, next.word_idx)) == Some(c));
                        pos_in_sel((prev.ast_block_idx, prev.word_idx))
                            && pos_in_sel((next.ast_block_idx, next.word_idx))
                    } else {
                        false
                    }
                } else {
                    false
                };

                if let Some(hc) = hl_color {
                    blend_rect_clipped(buffer, buf_w, buf_h, cx.round() as i32, band_y, step_w.ceil().max(1.0) as u32, band_h, hc, 92);
                }
                if selected {
                    blend_rect_clipped(buffer, buf_w, buf_h, cx.round() as i32, band_y, step_w.ceil().max(1.0) as u32, band_h, rctx.accent_color, 72);
                }
                if !is_space && w.word_idx != usize::MAX {
                    if let Some(ref mut out_rects) = word_hits {
                        out_rects.push(EpubWordHit {
                            x: cx.round() as i32,
                            y: ly,
                            w: step_w.ceil().max(4.0) as u32,
                            h: line_h.round().max(12.0) as u32,
                            block_idx: w.ast_block_idx,
                            word_idx: w.word_idx,
                            text: w.text.clone(),
                        });
                    }
                }
            }

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
                cx += step_w;
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
                if rctx.book_mode {
                    draw_text_styled_clipped(buffer, buf_w, buf_h, cx.round() as i32 + pad, ly, buf_w as i32, &w.text, TextStyle::mono(font_sz * 0.92), theme.accent_soft_fg);
                } else {
                    draw_text_clipped(buffer, buf_w, buf_h, cx.round() as i32 + pad, ly, buf_w as i32, &w.text, font_sz * 0.92, theme.accent_soft_fg, false);
                }
                cx += step_w;
                continue;
            }
            let col = if w.link_idx.is_some() {
                theme.accent
            } else if w.strike {
                theme.text_muted
            } else if w.italic && !rctx.book_mode {
                theme.text_secondary
            } else {
                base_color
            };
            if w.text != " " {
                if rctx.book_mode {
                    draw_text_styled_clipped(
                        buffer,
                        buf_w,
                        buf_h,
                        cx.round() as i32,
                        ly,
                        buf_w as i32,
                        &w.text,
                        TextStyle {
                            family: rctx.font_family,
                            size_px: font_sz,
                            bold: w.bold,
                            italic: w.italic,
                        },
                        col,
                    );
                } else {
                    draw_text_clipped(buffer, buf_w, buf_h, cx.round() as i32, ly, buf_w as i32, &w.text, font_sz, col, w.bold);
                }
            }
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
            cx += step_w;
        }
    }
}

pub(super) fn md_reading_theme(base: UiTheme, filter: ReadingFilter, mode: UiThemeMode) -> (u32, UiTheme) {
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

fn count_inline_words(spans: &[crate::markdown::InlineSpan]) -> usize {
    spans
        .iter()
        .map(|s| s.as_plain_text().split_whitespace().count())
        .sum()
}

fn count_doc_words(blocks: &[crate::markdown::MdBlock]) -> usize {
    use crate::markdown::MdBlock;
    let mut total = 0usize;
    for b in blocks {
        match b {
            MdBlock::Heading { spans, .. } | MdBlock::Paragraph(spans) => {
                total += count_inline_words(spans);
            }
            MdBlock::CodeBlock { code, .. } => {
                total += code.split_whitespace().count();
            }
            MdBlock::MathBlock(tex) => {
                total += tex.split_whitespace().count();
            }
            MdBlock::Image { caption: Some(cap), .. } => {
                total += cap.split_whitespace().count();
            }
            MdBlock::Image { caption: None, .. } | MdBlock::HorizontalRule | MdBlock::PageBreak => {}
            MdBlock::Blockquote(inner) => {
                total += count_doc_words(inner);
            }
            MdBlock::Table { headers, rows, .. } => {
                for h in headers {
                    total += count_inline_words(h);
                }
                for row in rows {
                    for cell in row {
                        total += count_inline_words(cell);
                    }
                }
            }
            MdBlock::List { items, .. } => {
                for item in items {
                    total += count_inline_words(&item.spans);
                    total += count_doc_words(&item.sublists);
                }
            }
        }
    }
    total
}

fn build_cum_words_by_block(blocks: &[crate::markdown::MdBlock]) -> (usize, Vec<usize>) {
    let mut cum = Vec::with_capacity(blocks.len() + 1);
    let mut running = 0usize;
    cum.push(0);
    for b in blocks {
        running += count_doc_words(std::slice::from_ref(b));
        cum.push(running);
    }
    (running, cum)
}

fn md_block_y_h(blk: &LaidOutBlock) -> (f32, f32) {
    match blk {
        LaidOutBlock::Heading { y, h, .. }
        | LaidOutBlock::Paragraph { y, h, .. }
        | LaidOutBlock::CodeBlock { y, h, .. }
        | LaidOutBlock::Blockquote { y, h, .. }
        | LaidOutBlock::Table { y, h, .. }
        | LaidOutBlock::ListItem { y, h, .. }
        | LaidOutBlock::HorizontalRule { y, h, .. }
        | LaidOutBlock::MathBlock { y, h, .. }
        | LaidOutBlock::Image { y, h, .. } => (*y, *h),
        LaidOutBlock::PageBreak { y, .. } => (*y, 0.0),
    }
}

fn md_paginate_blocks(blocks: &[LaidOutBlock], page_inner_h: f32) -> Vec<f32> {
    let mut pages = vec![0.0f32];
    let mut cur_start = 0.0f32;
    for (bi, blk) in blocks.iter().enumerate() {
        if matches!(blk, LaidOutBlock::PageBreak { .. }) {
            if let Some(next_blk) = blocks.get(bi + 1) {
                let (ny, _) = md_block_y_h(next_blk);
                if ny > cur_start + 1.0 {
                    cur_start = ny;
                    pages.push(cur_start);
                }
            }
            continue;
        }
        let (by, bh) = md_block_y_h(blk);
        if by > cur_start + 1.0 && (by - cur_start) + bh > page_inner_h {
            cur_start = by;
            pages.push(cur_start);
        }
    }
    pages
}

fn is_word_boundary_char(c: char) -> bool {
    !(c.is_alphanumeric() || c == '_')
}

fn extract_span_words_indexed(spans: &[InlineSpan]) -> Vec<(usize, String)> {
    let mut words = Vec::new();
    let mut wi = 0usize;
    for sp in spans {
        match sp {
            InlineSpan::Math(t) | InlineSpan::Code(t) => {
                words.push((wi, t.clone()));
                wi += 1;
            }
            _ => {
                let txt = sp.as_plain_text();
                let bytes = txt.as_bytes();
                let mut cur_start = 0usize;
                let mut i = 0usize;
                while i < bytes.len() {
                    if bytes[i].is_ascii_whitespace() {
                        if i > cur_start {
                            words.push((wi, txt[cur_start..i].to_string()));
                            wi += 1;
                        }
                        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                            i += 1;
                        }
                        cur_start = i;
                    } else {
                        i += 1;
                    }
                }
                if cur_start < bytes.len() {
                    words.push((wi, txt[cur_start..].to_string()));
                    wi += 1;
                }
            }
        }
    }
    words
}

fn search_in_indexed_spans(
    ast_block_idx: usize,
    spans: &[InlineSpan],
    q_chars: &[char],
    case_sensitive: bool,
    whole_word: bool,
    out: &mut Vec<MdSearchMatch>,
) {
    let words = extract_span_words_indexed(spans);
    if words.is_empty() || q_chars.is_empty() {
        return;
    }
    // Build a flattened char vector alongside each char's owning `word_idx`
    let mut flat_chars: Vec<char> = Vec::new();
    let mut char_word: Vec<usize> = Vec::new();
    let mut raw_text = String::new();
    for (idx_in_list, (wi, wtxt)) in words.iter().enumerate() {
        if idx_in_list > 0 {
            flat_chars.push(' ');
            char_word.push(*wi);
            raw_text.push(' ');
        }
        raw_text.push_str(wtxt);
        if case_sensitive {
            for c in wtxt.chars() {
                flat_chars.push(c);
                char_word.push(*wi);
            }
        } else {
            for c in wtxt.chars() {
                for lc in c.to_lowercase() {
                    flat_chars.push(lc);
                    char_word.push(*wi);
                }
            }
        }
    }
    let qlen = q_chars.len();
    if flat_chars.len() < qlen {
        return;
    }
    let mut i = 0usize;
    while i + qlen <= flat_chars.len() {
        if flat_chars[i..i + qlen] == *q_chars {
            let ok_boundary = if whole_word {
                let left_ok = i == 0 || is_word_boundary_char(flat_chars[i - 1]);
                let right_ok =
                    i + qlen == flat_chars.len() || is_word_boundary_char(flat_chars[i + qlen]);
                left_ok && right_ok
            } else {
                true
            };
            if ok_boundary {
                let w_start = char_word[i];
                let w_end = char_word[i + qlen - 1];
                let snippet: String = raw_text.chars().take(80).collect();
                out.push(MdSearchMatch {
                    ast_block_idx,
                    word_start: w_start,
                    word_end: w_end,
                    snippet,
                });
                i += qlen;
                continue;
            }
        }
        i += 1;
    }
}

fn search_in_plain_block(
    ast_block_idx: usize,
    text: &str,
    q_chars: &[char],
    case_sensitive: bool,
    whole_word: bool,
    out: &mut Vec<MdSearchMatch>,
) {
    if text.is_empty() || q_chars.is_empty() {
        return;
    }
    let flat_chars: Vec<char> = if case_sensitive {
        text.chars().collect()
    } else {
        text.chars().flat_map(|c| c.to_lowercase()).collect()
    };
    let qlen = q_chars.len();
    if flat_chars.len() < qlen {
        return;
    }
    let mut i = 0usize;
    while i + qlen <= flat_chars.len() {
        if flat_chars[i..i + qlen] == *q_chars {
            let ok_boundary = if whole_word {
                let left_ok = i == 0 || is_word_boundary_char(flat_chars[i - 1]);
                let right_ok =
                    i + qlen == flat_chars.len() || is_word_boundary_char(flat_chars[i + qlen]);
                left_ok && right_ok
            } else {
                true
            };
            if ok_boundary {
                let snippet: String = text.chars().take(80).collect();
                out.push(MdSearchMatch {
                    ast_block_idx,
                    word_start: usize::MAX,
                    word_end: usize::MAX,
                    snippet,
                });
                i += qlen;
                continue;
            }
        }
        i += 1;
    }
}

fn search_list_items(
    ast_block_idx: usize,
    items: &[MdListItem],
    q_chars: &[char],
    case_sensitive: bool,
    whole_word: bool,
    out: &mut Vec<MdSearchMatch>,
) {
    for it in items {
        search_in_indexed_spans(ast_block_idx, &it.spans, q_chars, case_sensitive, whole_word, out);
        for sub in &it.sublists {
            if let MdBlock::List { items: sub_items, .. } = sub {
                search_list_items(ast_block_idx, sub_items, q_chars, case_sensitive, whole_word, out);
            }
        }
    }
}

/// Full-text search across a `MarkdownDoc` (EPUB, Markdown, or LaTeX) supporting
/// case-sensitive (`Aa`), whole-word (`W`), single-word substrings, and multi-word phrases.
pub(super) fn search_markdown_doc(
    doc: &crate::markdown::MarkdownDoc,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<MdSearchMatch> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let q_chars: Vec<char> = if case_sensitive {
        trimmed.chars().collect()
    } else {
        trimmed.chars().flat_map(|c| c.to_lowercase()).collect()
    };
    let mut out = Vec::new();

    fn walk_block(
        ast_block_idx: usize,
        blk: &MdBlock,
        q_chars: &[char],
        case_sensitive: bool,
        whole_word: bool,
        out: &mut Vec<MdSearchMatch>,
    ) {
        match blk {
            MdBlock::Heading { spans, .. } | MdBlock::Paragraph(spans) => {
                search_in_indexed_spans(ast_block_idx, spans, q_chars, case_sensitive, whole_word, out);
            }
            MdBlock::CodeBlock { code, .. } => {
                search_in_plain_block(ast_block_idx, code, q_chars, case_sensitive, whole_word, out);
            }
            MdBlock::MathBlock(tex) => {
                search_in_plain_block(ast_block_idx, tex, q_chars, case_sensitive, whole_word, out);
            }
            MdBlock::Image { caption: Some(cap), .. } => {
                search_in_plain_block(ast_block_idx, cap, q_chars, case_sensitive, whole_word, out);
            }
            MdBlock::Blockquote(inner) => {
                for child in inner {
                    walk_block(ast_block_idx, child, q_chars, case_sensitive, whole_word, out);
                }
            }
            MdBlock::Table { headers, rows, .. } => {
                for h in headers {
                    search_in_indexed_spans(ast_block_idx, h, q_chars, case_sensitive, whole_word, out);
                }
                for row in rows {
                    for cell in row {
                        search_in_indexed_spans(ast_block_idx, cell, q_chars, case_sensitive, whole_word, out);
                    }
                }
            }
            MdBlock::List { items, .. } => {
                search_list_items(ast_block_idx, items, q_chars, case_sensitive, whole_word, out);
            }
            MdBlock::Image { caption: None, .. } | MdBlock::HorizontalRule | MdBlock::PageBreak => {}
        }
    }

    for (idx, blk) in doc.blocks.iter().enumerate() {
        walk_block(idx, blk, &q_chars, case_sensitive, whole_word, &mut out);
    }
    out
}

fn split_math_eq_no(tex: &str) -> (&str, Option<&str>) {
    if let Some(pos) = tex.rfind("\\eqno(") {
        let after = &tex[pos + "\\eqno".len()..];
        if after.starts_with('(') && after.ends_with(')') {
            return (tex[..pos].trim_end(), Some(after));
        }
    }
    (tex, None)
}

/// Cover page (index 0) is Spread 0 alone; remaining pages pair up as `(1,2)`, `(3,4)`, ...
fn md_magazine_total_spreads(total_pages: usize) -> usize {
    if total_pages <= 1 {
        1
    } else {
        1 + total_pages / 2
    }
}

fn md_magazine_spread_for_page(page_idx: usize) -> usize {
    if page_idx == 0 {
        0
    } else {
        1 + (page_idx - 1) / 2
    }
}

fn md_magazine_pages_for_spread(spread_idx: usize, total_pages: usize) -> (usize, Option<usize>) {
    if spread_idx == 0 || total_pages <= 1 {
        (0, None)
    } else {
        let left = (2 * spread_idx - 1).min(total_pages - 1);
        let right = if left + 1 < total_pages {
            Some(left + 1)
        } else {
            None
        };
        (left, right)
    }
}





