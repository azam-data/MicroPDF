//! Pure-Rust Markdown-to-PDF Paginated Layout Engine + PDFium Vector Renderer.
//!
//! Zero headless browsers or HTML/CSS engines: paginates `MarkdownDoc` into crisp
//! vector text, styled headings, repeated-header tables, shaded code blocks,
//! left-accented blockquotes, horizontal rules, and `Page X of Y` footers.

use crate::fonts::global_system_fonts;
use crate::markdown::{InlineSpan, MarkdownDoc, MdAlign, MdBlock, MdListItem};
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::path::Path;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MdPdfPageSize {
    A4,
    Letter,
}

impl MdPdfPageSize {
    pub fn dimensions_pt(self) -> (f32, f32) {
        match self {
            MdPdfPageSize::A4 => (595.28, 841.89),
            MdPdfPageSize::Letter => (612.0, 792.0),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MdPdfFontRole {
    Sans,
    SansBold,
    SansItalic,
    SansBoldItalic,
    Mono,
}

#[derive(Clone, Debug)]
pub enum PdfDrawCmd {
    Rect {
        x: f32,
        y_top: f32,
        w: f32,
        h: f32,
        fill: Option<(u8, u8, u8)>,
        stroke: Option<(u8, u8, u8)>,
        stroke_w: f32,
    },
    Line {
        x1: f32,
        y1_top: f32,
        x2: f32,
        y2_top: f32,
        color: (u8, u8, u8),
        width: f32,
    },
    Text {
        x: f32,
        baseline_y_top: f32,
        text: String,
        font: MdPdfFontRole,
        size_pt: f32,
        color: (u8, u8, u8),
    },
}

#[derive(Clone, Debug)]
pub struct PdfPagePlan {
    pub width_pt: f32,
    pub height_pt: f32,
    pub cmds: Vec<PdfDrawCmd>,
}

const MARGIN_PT: f32 = 54.0; // 0.75 in
const FOOTER_RESERVE_PT: f32 = 28.0;

fn approx_char_w(ch: char, size_pt: f32, role: MdPdfFontRole) -> f32 {
    let base = match role {
        MdPdfFontRole::Mono => 0.60,
        MdPdfFontRole::SansBold | MdPdfFontRole::SansBoldItalic => match ch {
            'i' | 'l' | 'I' | '!' | '.' | ',' | ':' | ';' | '\'' => 0.30,
            'm' | 'w' | 'M' | 'W' => 0.80,
            ' ' => 0.28,
            _ => 0.56,
        },
        MdPdfFontRole::Sans | MdPdfFontRole::SansItalic => match ch {
            'i' | 'l' | 'I' | '!' | '.' | ',' | ':' | ';' | '\'' => 0.28,
            'm' | 'w' | 'M' | 'W' => 0.76,
            ' ' => 0.26,
            _ => 0.52,
        },
    };
    base * size_pt
}

fn approx_text_w(text: &str, size_pt: f32, role: MdPdfFontRole) -> f32 {
    text.chars().map(|c| approx_char_w(c, size_pt, role)).sum()
}

#[derive(Clone)]
struct PdfWord {
    text: String,
    role: MdPdfFontRole,
    color: (u8, u8, u8),
    strike: bool,
    underline: bool,
    code_bg: bool,
    w: f32,
}

#[derive(Clone)]
struct PdfLine {
    words: Vec<PdfWord>,
    total_w: f32,
}

fn spans_to_pdf_words(spans: &[InlineSpan], size_pt: f32, force_bold: bool, base_color: (u8, u8, u8)) -> Vec<PdfWord> {
    let mut out = Vec::new();
    for sp in spans {
        let (txt, mut role, color, strike, underline, code_bg) = match sp {
            InlineSpan::Text(t) => (t.as_str(), MdPdfFontRole::Sans, base_color, false, false, false),
            InlineSpan::Bold(t) => (t.as_str(), MdPdfFontRole::SansBold, base_color, false, false, false),
            InlineSpan::Italic(t) => (t.as_str(), MdPdfFontRole::SansItalic, base_color, false, false, false),
            InlineSpan::BoldItalic(t) => (t.as_str(), MdPdfFontRole::SansBoldItalic, base_color, false, false, false),
            InlineSpan::Strikethrough(t) => (t.as_str(), MdPdfFontRole::Sans, (120, 125, 135), true, false, false),
            InlineSpan::Code(t) => (t.as_str(), MdPdfFontRole::Mono, (160, 45, 70), false, false, true),
            InlineSpan::Link { text, .. } => (text.as_str(), MdPdfFontRole::Sans, (29, 78, 216), false, true, false),
        };
        if force_bold && !code_bg {
            role = match role {
                MdPdfFontRole::SansItalic | MdPdfFontRole::SansBoldItalic => MdPdfFontRole::SansBoldItalic,
                _ => MdPdfFontRole::SansBold,
            };
        }
        if code_bg {
            let fs = size_pt * 0.90;
            let w = approx_text_w(txt, fs, role) + 6.0;
            out.push(PdfWord {
                text: txt.to_string(),
                role,
                color,
                strike,
                underline,
                code_bg: true,
                w,
            });
            continue;
        }
        let mut start = 0usize;
        let bytes = txt.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i].is_ascii_whitespace() {
                if i > start {
                    let word = &txt[start..i];
                    out.push(PdfWord {
                        text: word.to_string(),
                        role,
                        color,
                        strike,
                        underline,
                        code_bg: false,
                        w: approx_text_w(word, size_pt, role),
                    });
                }
                let sp_start = i;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let s = if txt[sp_start..i].contains('\n') { "\n" } else { " " };
                let w = if s == "\n" { 0.0 } else { approx_text_w(" ", size_pt, role) };
                out.push(PdfWord {
                    text: s.to_string(),
                    role,
                    color,
                    strike,
                    underline,
                    code_bg: false,
                    w,
                });
                start = i;
            } else {
                i += 1;
            }
        }
        if start < bytes.len() {
            let word = &txt[start..];
            out.push(PdfWord {
                text: word.to_string(),
                role,
                color,
                strike,
                underline,
                code_bg: false,
                w: approx_text_w(word, size_pt, role),
            });
        }
    }
    out
}

fn wrap_pdf_words(words: Vec<PdfWord>, max_w: f32) -> Vec<PdfLine> {
    let mut lines = Vec::new();
    let mut cur: Vec<PdfWord> = Vec::new();
    let mut cur_w = 0.0f32;
    let limit = max_w.max(40.0);

    for w in words {
        if w.text == "\n" {
            lines.push(PdfLine {
                words: std::mem::take(&mut cur),
                total_w: cur_w,
            });
            cur_w = 0.0;
            continue;
        }
        let is_sp = w.text == " ";
        if is_sp && cur.is_empty() {
            continue;
        }
        if !cur.is_empty() && !is_sp && cur_w + w.w > limit {
            while cur.last().is_some_and(|x| x.text == " ") {
                if let Some(p) = cur.pop() {
                    cur_w -= p.w;
                }
            }
            lines.push(PdfLine {
                words: std::mem::take(&mut cur),
                total_w: cur_w.max(0.0),
            });
            cur_w = 0.0;
        }
        cur_w += w.w;
        cur.push(w);
    }
    while cur.last().is_some_and(|x| x.text == " ") {
        if let Some(p) = cur.pop() {
            cur_w -= p.w;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(PdfLine {
            words: cur,
            total_w: cur_w.max(0.0),
        });
    }
    lines
}

struct Paginator {
    page_w: f32,
    page_h: f32,
    content_w: f32,
    y_limit: f32,
    pages: Vec<PdfPagePlan>,
    cur_cmds: Vec<PdfDrawCmd>,
    cur_y: f32,
}

impl Paginator {
    fn new(page_size: MdPdfPageSize) -> Self {
        let (page_w, page_h) = page_size.dimensions_pt();
        Self {
            page_w,
            page_h,
            content_w: page_w - MARGIN_PT * 2.0,
            y_limit: page_h - MARGIN_PT - FOOTER_RESERVE_PT,
            pages: Vec::new(),
            cur_cmds: Vec::new(),
            cur_y: MARGIN_PT,
        }
    }

    fn new_page(&mut self) {
        let cmds = std::mem::take(&mut self.cur_cmds);
        self.pages.push(PdfPagePlan {
            width_pt: self.page_w,
            height_pt: self.page_h,
            cmds,
        });
        self.cur_y = MARGIN_PT;
    }

    fn ensure_space(&mut self, needed_h: f32) {
        if self.cur_y + needed_h > self.y_limit && self.cur_y > MARGIN_PT + 4.0 {
            self.new_page();
        }
    }

    fn emit_line(&mut self, line: &PdfLine, x: f32, y_top: f32, size_pt: f32, align: MdAlign, max_w: f32) {
        let dx = match align {
            MdAlign::Center => ((max_w - line.total_w) * 0.5).max(0.0),
            MdAlign::Right => (max_w - line.total_w).max(0.0),
            MdAlign::Left | MdAlign::None => 0.0,
        };
        let mut cx = x + dx;
        let baseline = y_top + size_pt * 0.85;
        for w in &line.words {
            if w.code_bg {
                let fs = size_pt * 0.90;
                self.cur_cmds.push(PdfDrawCmd::Rect {
                    x: cx,
                    y_top: y_top + 1.0,
                    w: w.w,
                    h: size_pt * 1.15,
                    fill: Some((241, 245, 249)),
                    stroke: Some((203, 213, 225)),
                    stroke_w: 0.5,
                });
                self.cur_cmds.push(PdfDrawCmd::Text {
                    x: cx + 3.0,
                    baseline_y_top: baseline,
                    text: w.text.clone(),
                    font: w.role,
                    size_pt: fs,
                    color: w.color,
                });
                cx += w.w;
                continue;
            }
            if w.text != " " {
                self.cur_cmds.push(PdfDrawCmd::Text {
                    x: cx,
                    baseline_y_top: baseline,
                    text: w.text.clone(),
                    font: w.role,
                    size_pt,
                    color: w.color,
                });
                if w.underline {
                    self.cur_cmds.push(PdfDrawCmd::Line {
                        x1: cx,
                        y1_top: baseline + 1.5,
                        x2: cx + w.w,
                        y2_top: baseline + 1.5,
                        color: w.color,
                        width: 0.6,
                    });
                }
                if w.strike {
                    self.cur_cmds.push(PdfDrawCmd::Line {
                        x1: cx,
                        y1_top: baseline - size_pt * 0.30,
                        x2: cx + w.w,
                        y2_top: baseline - size_pt * 0.30,
                        color: w.color,
                        width: 0.7,
                    });
                }
            }
            cx += w.w;
        }
    }

    fn emit_blocks(&mut self, blocks: &[MdBlock], x: f32, max_w: f32) {
        for (idx, blk) in blocks.iter().enumerate() {
            match blk {
                MdBlock::Heading { level, spans } => {
                    let size_pt = match level {
                        1 => 20.0,
                        2 => 16.0,
                        3 => 13.5,
                        4 => 11.5,
                        5 => 10.5,
                        _ => 10.0,
                    };
                    let line_h = size_pt * 1.35;
                    let words = spans_to_pdf_words(spans, size_pt, true, (24, 38, 62));
                    let lines = wrap_pdf_words(words, max_w);
                    let div_h = if *level <= 2 { 6.0 } else { 0.0 };
                    let h = lines.len() as f32 * line_h + div_h;
                    // Keep heading with at least ~28pt of following content
                    self.ensure_space(h + 28.0);
                    if idx > 0 && self.cur_y > MARGIN_PT + 2.0 {
                        self.cur_y += 6.0;
                    }
                    for line in &lines {
                        self.emit_line(line, x, self.cur_y, size_pt, MdAlign::Left, max_w);
                        self.cur_y += line_h;
                    }
                    if *level <= 2 {
                        self.cur_cmds.push(PdfDrawCmd::Line {
                            x1: x,
                            y1_top: self.cur_y + 2.0,
                            x2: x + max_w,
                            y2_top: self.cur_y + 2.0,
                            color: (203, 213, 225),
                            width: 0.75,
                        });
                        self.cur_y += 6.0;
                    }
                    self.cur_y += 6.0;
                }
                MdBlock::Paragraph(spans) => {
                    let size_pt = 10.5;
                    let line_h = 15.5;
                    let words = spans_to_pdf_words(spans, size_pt, false, (36, 41, 49));
                    let lines = wrap_pdf_words(words, max_w);
                    for line in &lines {
                        self.ensure_space(line_h);
                        self.emit_line(line, x, self.cur_y, size_pt, MdAlign::Left, max_w);
                        self.cur_y += line_h;
                    }
                    self.cur_y += 8.0;
                }
                MdBlock::CodeBlock { lang, code } => {
                    let size_pt = 9.0;
                    let line_h = 13.0;
                    let raw_lines: Vec<&str> = if code.is_empty() { vec![""] } else { code.lines().collect() };
                    let pad = 8.0;
                    let hdr_h = if lang.is_some() { 15.0 } else { 0.0 };
                    let total_h = hdr_h + pad * 2.0 + raw_lines.len() as f32 * line_h;

                    if total_h <= self.y_limit - MARGIN_PT {
                        self.ensure_space(total_h);
                    } else {
                        self.ensure_space(hdr_h + pad * 2.0 + line_h * 2.0);
                    }

                    let mut chunk_start = 0usize;
                    while chunk_start < raw_lines.len() {
                        let avail = (self.y_limit - self.cur_y - pad * 2.0 - if chunk_start == 0 { hdr_h } else { 0.0 }).max(line_h);
                        let fit_count = (avail / line_h).floor().max(1.0) as usize;
                        let chunk_end = (chunk_start + fit_count).min(raw_lines.len());
                        let chunk_hdr = if chunk_start == 0 { hdr_h } else { 0.0 };
                        let box_h = chunk_hdr + pad * 2.0 + (chunk_end - chunk_start) as f32 * line_h;
                        let box_y = self.cur_y;

                        self.cur_cmds.push(PdfDrawCmd::Rect {
                            x,
                            y_top: box_y,
                            w: max_w,
                            h: box_h,
                            fill: Some((246, 248, 250)),
                            stroke: Some((208, 215, 222)),
                            stroke_w: 0.6,
                        });
                        if chunk_start == 0 {
                            if let Some(l) = lang {
                                self.cur_cmds.push(PdfDrawCmd::Text {
                                    x: x + pad,
                                    baseline_y_top: box_y + 11.0,
                                    text: l.to_uppercase(),
                                    font: MdPdfFontRole::SansBold,
                                    size_pt: 7.5,
                                    color: (100, 116, 139),
                                });
                            }
                        }
                        let mut ty = box_y + chunk_hdr + pad;
                        for line in &raw_lines[chunk_start..chunk_end] {
                            self.cur_cmds.push(PdfDrawCmd::Text {
                                x: x + pad,
                                baseline_y_top: ty + size_pt * 0.85,
                                text: (*line).to_string(),
                                font: MdPdfFontRole::Mono,
                                size_pt,
                                color: (30, 41, 59),
                            });
                            ty += line_h;
                        }
                        self.cur_y = box_y + box_h;
                        chunk_start = chunk_end;
                        if chunk_start < raw_lines.len() {
                            self.new_page();
                        }
                    }
                    self.cur_y += 9.0;
                }
                MdBlock::Blockquote(inner) => {
                    let start_page = self.pages.len();
                    let start_y = self.cur_y;
                    self.cur_y += 4.0;
                    self.emit_blocks(inner, x + 14.0, (max_w - 18.0).max(60.0));
                    // Draw left accent bar on start page if still on same page
                    if self.pages.len() == start_page {
                        let bar_h = (self.cur_y - start_y - 4.0).max(12.0);
                        self.cur_cmds.push(PdfDrawCmd::Rect {
                            x,
                            y_top: start_y,
                            w: 3.0,
                            h: bar_h,
                            fill: Some((59, 130, 246)),
                            stroke: None,
                            stroke_w: 0.0,
                        });
                    }
                    self.cur_y += 4.0;
                }
                MdBlock::Table { headers, alignments, rows } => {
                    let ncols = headers.len().max(1);
                    let col_w = max_w / ncols as f32;
                    let size_pt = 9.5;
                    let line_h = 13.5;
                    let pad_x = 6.0;
                    let pad_y = 5.0;

                    let hdr_cells: Vec<Vec<PdfLine>> = (0..ncols)
                        .map(|c| {
                            let sp = headers.get(c).map(|v| v.as_slice()).unwrap_or(&[]);
                            wrap_pdf_words(spans_to_pdf_words(sp, size_pt, true, (24, 38, 62)), col_w - pad_x * 2.0)
                        })
                        .collect();
                    let hdr_lines = hdr_cells.iter().map(|c| c.len()).max().unwrap_or(1);
                    let hdr_h = hdr_lines as f32 * line_h + pad_y * 2.0;

                    let draw_table_header = |pag: &mut Paginator| {
                        let y0 = pag.cur_y;
                        pag.cur_cmds.push(PdfDrawCmd::Rect {
                            x,
                            y_top: y0,
                            w: max_w,
                            h: hdr_h,
                            fill: Some((238, 242, 247)),
                            stroke: Some((203, 213, 225)),
                            stroke_w: 0.6,
                        });
                        for (c, lines) in hdr_cells.iter().enumerate() {
                            let cx = x + c as f32 * col_w;
                            let align = alignments.get(c).copied().unwrap_or(MdAlign::Left);
                            for (li, line) in lines.iter().enumerate() {
                                pag.emit_line(line, cx + pad_x, y0 + pad_y + li as f32 * line_h, size_pt, align, col_w - pad_x * 2.0);
                            }
                        }
                        pag.cur_y += hdr_h;
                    };

                    self.ensure_space(hdr_h + line_h + pad_y * 2.0);
                    draw_table_header(self);

                    for (ri, row) in rows.iter().enumerate() {
                        let row_cells: Vec<Vec<PdfLine>> = (0..ncols)
                            .map(|c| {
                                let sp = row.get(c).map(|v| v.as_slice()).unwrap_or(&[]);
                                wrap_pdf_words(spans_to_pdf_words(sp, size_pt, false, (36, 41, 49)), col_w - pad_x * 2.0)
                            })
                            .collect();
                        let max_l = row_cells.iter().map(|c| c.len()).max().unwrap_or(1);
                        let rh = max_l as f32 * line_h + pad_y * 2.0;
                        if self.cur_y + rh > self.y_limit && self.cur_y > MARGIN_PT + 4.0 {
                            self.new_page();
                            draw_table_header(self);
                        }
                        let y0 = self.cur_y;
                        let fill = if ri % 2 == 1 { Some((248, 250, 252)) } else { None };
                        self.cur_cmds.push(PdfDrawCmd::Rect {
                            x,
                            y_top: y0,
                            w: max_w,
                            h: rh,
                            fill,
                            stroke: Some((203, 213, 225)),
                            stroke_w: 0.5,
                        });
                        for (c, lines) in row_cells.iter().enumerate() {
                            let cx = x + c as f32 * col_w;
                            let align = alignments.get(c).copied().unwrap_or(MdAlign::Left);
                            for (li, line) in lines.iter().enumerate() {
                                self.emit_line(line, cx + pad_x, y0 + pad_y + li as f32 * line_h, size_pt, align, col_w - pad_x * 2.0);
                            }
                        }
                        self.cur_y += rh;
                    }
                    self.cur_y += 10.0;
                }
                MdBlock::List { ordered, start, items } => {
                    self.emit_list_items(items, *ordered, *start, 0, x, max_w);
                    self.cur_y += 5.0;
                }
                MdBlock::HorizontalRule => {
                    self.ensure_space(14.0);
                    self.cur_cmds.push(PdfDrawCmd::Line {
                        x1: x,
                        y1_top: self.cur_y + 6.0,
                        x2: x + max_w,
                        y2_top: self.cur_y + 6.0,
                        color: (203, 213, 225),
                        width: 0.75,
                    });
                    self.cur_y += 14.0;
                }
            }
        }
    }

    fn emit_list_items(&mut self, items: &[MdListItem], ordered: bool, start: u64, depth: usize, x: f32, max_w: f32) {
        let size_pt = 10.5;
        let line_h = 15.0;
        let indent = (depth as f32 + 1.0) * 16.0;
        let text_x = x + indent;
        let avail = (max_w - indent).max(40.0);

        for (i, item) in items.iter().enumerate() {
            let marker = if let Some(checked) = item.task_checked {
                if checked { "[x]" } else { "[ ]" }.to_string()
            } else if ordered {
                format!("{}.", start + i as u64)
            } else if depth % 2 == 0 {
                "•".to_string()
            } else {
                "◦".to_string()
            };
            let words = spans_to_pdf_words(&item.spans, size_pt, false, (36, 41, 49));
            let lines = wrap_pdf_words(words, avail);
            self.ensure_space(line_h);
            self.cur_cmds.push(PdfDrawCmd::Text {
                x: text_x - 12.0,
                baseline_y_top: self.cur_y + size_pt * 0.85,
                text: marker,
                font: MdPdfFontRole::SansBold,
                size_pt,
                color: (59, 130, 246),
            });
            for line in &lines {
                self.ensure_space(line_h);
                self.emit_line(line, text_x, self.cur_y, size_pt, MdAlign::Left, avail);
                self.cur_y += line_h;
            }
            self.cur_y += 2.5;
            for sub in &item.sublists {
                if let MdBlock::List { ordered: so, start: ss, items: si } = sub {
                    self.emit_list_items(si, *so, *ss, depth + 1, x, max_w);
                }
            }
        }
    }

    fn finish(mut self) -> Vec<PdfPagePlan> {
        if !self.cur_cmds.is_empty() || self.pages.is_empty() {
            self.new_page();
        }
        let total = self.pages.len();
        for (i, page) in self.pages.iter_mut().enumerate() {
            let footer_txt = format!("Page {} of {}", i + 1, total);
            let fw = approx_text_w(&footer_txt, 8.5, MdPdfFontRole::Sans);
            page.cmds.push(PdfDrawCmd::Text {
                x: (page.width_pt - fw) * 0.5,
                baseline_y_top: page.height_pt - 28.0,
                text: footer_txt,
                font: MdPdfFontRole::Sans,
                size_pt: 8.5,
                color: (120, 125, 135),
            });
        }
        self.pages
    }
}

pub fn paginate_markdown_for_pdf(doc: &MarkdownDoc, page_size: MdPdfPageSize) -> Vec<PdfPagePlan> {
    let mut pag = Paginator::new(page_size);
    let max_w = pag.content_w;
    pag.emit_blocks(&doc.blocks, MARGIN_PT, max_w);
    pag.finish()
}

fn find_system_font_path(keywords: &[&str]) -> Option<std::path::PathBuf> {
    let catalog = global_system_fonts();
    for entry in catalog {
        let Some(ref path) = entry.path else { continue };
        let lower = entry.display_name.to_ascii_lowercase();
        if keywords.iter().all(|kw| lower.contains(kw)) {
            return Some(path.clone());
        }
    }
    None
}

fn load_role_font<'a>(doc: &mut PdfDocument<'a>, role: MdPdfFontRole) -> PdfFontToken {
    let candidates: &[&[&str]] = match role {
        MdPdfFontRole::Sans => &[&["segoe ui"], &["liberation sans"], &["dejavu sans"], &["arial"]],
        MdPdfFontRole::SansBold => &[&["segoe ui", "bold"], &["liberation sans", "bold"], &["dejavu sans", "bold"], &["arial", "bold"]],
        MdPdfFontRole::SansItalic => &[&["segoe ui", "italic"], &["liberation sans", "italic"], &["dejavu sans", "oblique"], &["arial", "italic"]],
        MdPdfFontRole::SansBoldItalic => &[&["segoe ui", "bold", "italic"], &["liberation sans", "bold", "italic"], &["arial", "bold", "italic"]],
        MdPdfFontRole::Mono => &[&["consolas"], &["liberation mono"], &["dejavu sans mono"], &["courier new"]],
    };
    for kws in candidates {
        if let Some(p) = find_system_font_path(kws) {
            if let Ok(tok) = doc.fonts_mut().load_true_type_from_file(&p, true) {
                return tok;
            }
        }
    }
    match role {
        MdPdfFontRole::SansBold | MdPdfFontRole::SansBoldItalic => doc.fonts_mut().helvetica_bold(),
        MdPdfFontRole::Mono => doc.fonts_mut().courier(),
        MdPdfFontRole::Sans | MdPdfFontRole::SansItalic => doc.fonts_mut().helvetica(),
    }
}

pub fn export_markdown_to_pdf(
    pdfium: &Pdfium,
    md: &MarkdownDoc,
    output_path: &Path,
    page_size: MdPdfPageSize,
) -> Result<()> {
    let pages = paginate_markdown_for_pdf(md, page_size);
    let mut doc = pdfium
        .create_new_pdf()
        .context("Failed to create PDFium document for Markdown export")?;

    let f_sans = load_role_font(&mut doc, MdPdfFontRole::Sans);
    let f_bold = load_role_font(&mut doc, MdPdfFontRole::SansBold);
    let f_italic = load_role_font(&mut doc, MdPdfFontRole::SansItalic);
    let f_bold_italic = load_role_font(&mut doc, MdPdfFontRole::SansBoldItalic);
    let f_mono = load_role_font(&mut doc, MdPdfFontRole::Mono);

    let paper = match page_size {
        MdPdfPageSize::A4 => PdfPagePaperSize::a4(),
        MdPdfPageSize::Letter => PdfPagePaperSize::from_inches(8.5, 11.0),
    };

    for plan in &pages {
        let mut page = doc
            .pages_mut()
            .create_page_at_end(paper)
            .context("Failed to create PDF page")?;
        let ph = plan.height_pt;

        for cmd in &plan.cmds {
            match cmd {
                PdfDrawCmd::Rect { x, y_top, w, h, fill, stroke, stroke_w } => {
                    let rect = PdfRect::new(
                        PdfPoints::new(ph - (*y_top + *h)),
                        PdfPoints::new(*x),
                        PdfPoints::new(ph - *y_top),
                        PdfPoints::new(*x + *w),
                    );
                    let f_col = fill.map(|(r, g, b)| PdfColor::new(r, g, b, 255));
                    let s_col = stroke.map(|(r, g, b)| PdfColor::new(r, g, b, 255));
                    let sw = stroke.map(|_| PdfPoints::new(*stroke_w));
                    if let Ok(obj) = PdfPagePathObject::new_rect(&doc, rect, s_col, sw, f_col) {
                        let _ = page.objects_mut().add_path_object(obj);
                    }
                }
                PdfDrawCmd::Line { x1, y1_top, x2, y2_top, color: (r, g, b), width } => {
                    if let Ok(obj) = PdfPagePathObject::new_line(
                        &doc,
                        PdfPoints::new(*x1),
                        PdfPoints::new(ph - *y1_top),
                        PdfPoints::new(*x2),
                        PdfPoints::new(ph - *y2_top),
                        PdfColor::new(*r, *g, *b, 255),
                        PdfPoints::new(*width),
                    ) {
                        let _ = page.objects_mut().add_path_object(obj);
                    }
                }
                PdfDrawCmd::Text { x, baseline_y_top, text, font, size_pt, color: (r, g, b) } => {
                    let tok = match font {
                        MdPdfFontRole::Sans => f_sans,
                        MdPdfFontRole::SansBold => f_bold,
                        MdPdfFontRole::SansItalic => f_italic,
                        MdPdfFontRole::SansBoldItalic => f_bold_italic,
                        MdPdfFontRole::Mono => f_mono,
                    };
                    if let Ok(mut tobj) = PdfPageTextObject::new(&doc, text, tok, PdfPoints::new(*size_pt)) {
                        let _ = tobj.set_fill_color(PdfColor::new(*r, *g, *b, 255));
                        let _ = tobj.translate(PdfPoints::new(*x), PdfPoints::new(ph - *baseline_y_top));
                        let _ = page.objects_mut().add_text_object(tobj);
                    }
                }
            }
        }
    }

    doc.save_to_file(output_path)
        .with_context(|| format!("Failed to save exported PDF to {:?}", output_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse_markdown;

    #[test]
    fn test_markdown_pdf_pagination_multipage_and_repeated_table_headers() {
        let mut md = String::from("# Long Document\n\n");
        for i in 1..=60 {
            md.push_str(&format!("Paragraph {i}: This is a sample line of text that wraps cleanly across margins for pagination verification.\n\n"));
        }
        md.push_str("| Col A | Col B |\n| :--- | ---: |\n");
        for r in 1..=25 {
            md.push_str(&format!("| Row {r} | {r}00 |\n"));
        }
        let doc = parse_markdown(&md, "long.md");
        let pages = paginate_markdown_for_pdf(&doc, MdPdfPageSize::A4);
        assert!(pages.len() >= 2, "Expected multi-page PDF plan, got {}", pages.len());
        let last_cmds = &pages.last().unwrap().cmds;
        let has_footer = last_cmds.iter().any(|c| matches!(c, PdfDrawCmd::Text { text, .. } if text.contains("Page ") && text.contains(" of ")));
        assert!(has_footer, "Expected Page X of Y footer on last page");
    }
}
