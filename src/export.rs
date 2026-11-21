//! Pure-Rust PDF -> Markdown (`.md`), LaTeX (`.tex`), and reflowable plain-text extractor.
//! Uses PDFium's existing text/font/annotation geometry with zero external dependencies and
//! zero idle RAM overhead (only one page's runs are held at a time during export).

use pdfium_render::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct TextRun {
    pub text: String,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportBlock {
    Heading { level: u8, text: String },
    Paragraph(Vec<StyledSpan>),
    BulletItem(Vec<StyledSpan>),
    NumberedItem { num: String, spans: Vec<StyledSpan> },
    Table(Vec<Vec<String>>),
}

#[derive(Clone, Debug)]
struct TextLine {
    runs: Vec<TextRun>,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    max_font: f32,
    avg_font: f32,
    all_bold: bool,
}

impl TextLine {
    fn plain_text(&self) -> String {
        join_runs_plain(&self.runs)
    }
}

fn join_runs_plain(runs: &[TextRun]) -> String {
    let mut out = String::new();
    let mut prev_right: Option<f32> = None;
    let mut prev_font = 10.0f32;
    for r in runs {
        let t = r.text.trim_matches('\r');
        if t.is_empty() {
            continue;
        }
        if let Some(pr) = prev_right {
            let gap = r.left - pr;
            let needs_space = gap > (prev_font * 0.16).max(1.2)
                && !out.ends_with(' ')
                && !t.starts_with(' ')
                && !matches!(t.chars().next(), Some(',' | '.' | ';' | ':' | ')' | ']' | '?' | '!'));
            if needs_space {
                out.push(' ');
            }
        }
        out.push_str(t);
        prev_right = Some(r.right);
        prev_font = r.font_size.max(6.0);
    }
    collapse_spaces(out.trim())
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_sp = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_sp {
                out.push(' ');
                last_sp = true;
            }
        } else {
            out.push(ch);
            last_sp = false;
        }
    }
    out
}

fn runs_to_styled_spans(runs: &[TextRun]) -> Vec<StyledSpan> {
    let mut spans: Vec<StyledSpan> = Vec::new();
    let mut prev_right: Option<f32> = None;
    let mut prev_font = 10.0f32;

    for r in runs {
        let t = r.text.replace('\r', "").replace('\n', " ");
        if t.trim().is_empty() {
            continue;
        }
        let mut piece = String::new();
        if let Some(pr) = prev_right {
            let gap = r.left - pr;
            let prev_ends_sp = spans.last().map(|s| s.text.ends_with(' ')).unwrap_or(true);
            let starts_punct = matches!(
                t.trim_start().chars().next(),
                Some(',' | '.' | ';' | ':' | ')' | ']' | '?' | '!')
            );
            if gap > (prev_font * 0.16).max(1.2) && !prev_ends_sp && !t.starts_with(' ') && !starts_punct {
                piece.push(' ');
            }
        }
        piece.push_str(&collapse_spaces(&t));
        prev_right = Some(r.right);
        prev_font = r.font_size.max(6.0);

        if let Some(last) = spans.last_mut() {
            if last.bold == r.bold && last.italic == r.italic && last.link == r.link {
                last.text.push_str(&piece);
                continue;
            }
        }
        spans.push(StyledSpan {
            text: piece,
            bold: r.bold,
            italic: r.italic,
            link: r.link.clone(),
        });
    }
    // Trim outer whitespace on first/last spans
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = spans.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    spans.retain(|s| !s.text.is_empty());
    spans
}

fn extract_page_runs(page: &PdfPage) -> (f32, f32, Vec<TextRun>) {
    let page_w = page.width().value.max(1.0);
    let page_h = page.height().value.max(1.0);

    // Collect hyperlink annotations on this page so text overlapping a link gets its URI.
    let mut links: Vec<(PdfRect, String)> = Vec::new();
    for annot in page.annotations().iter() {
        if let Some(link_annot) = annot.as_link_annotation() {
            if let Ok(bounds) = annot.bounds() {
                if let Ok(link) = link_annot.link() {
                    if let Some(action) = link.action() {
                        if let Some(uri_action) = action.as_uri_action() {
                            if let Ok(uri) = uri_action.uri() {
                                if !uri.trim().is_empty() {
                                    links.push((bounds, uri));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut runs = Vec::new();
    for obj in page.objects().iter() {
        if let Some(text_obj) = obj.as_text_object() {
            let txt = text_obj.text();
            if txt.trim().is_empty() {
                continue;
            }
            let Ok(b) = obj.bounds() else { continue };
            let left = b.left().value.min(b.right().value);
            let right = b.left().value.max(b.right().value);
            let bottom = b.bottom().value.min(b.top().value);
            let top = b.bottom().value.max(b.top().value);
            let font_size = text_obj.scaled_font_size().value.max((top - bottom).max(6.0) * 0.8);
            let font = text_obj.font();
            let fname = font.name().to_ascii_lowercase();
            let bold = font.is_bold_reenforced()
                || font
                    .weight()
                    .map(|w| matches!(w, PdfFontWeight::Weight600 | PdfFontWeight::Weight700Bold | PdfFontWeight::Weight800 | PdfFontWeight::Weight900))
                    .unwrap_or(false)
                || fname.contains("bold")
                || fname.contains("black")
                || fname.contains("heavy")
                || fname.contains("demi")
                || fname.contains("semibold");
            let italic = font.is_italic() || fname.contains("italic") || fname.contains("oblique");

            let cx = (left + right) * 0.5;
            let cy = (bottom + top) * 0.5;
            let link = links.iter().find_map(|(lb, uri)| {
                let l = lb.left().value.min(lb.right().value);
                let r = lb.left().value.max(lb.right().value);
                let bt = lb.bottom().value.min(lb.top().value);
                let tp = lb.bottom().value.max(lb.top().value);
                if cx >= l && cx <= r && cy >= bt && cy <= tp {
                    Some(uri.clone())
                } else {
                    None
                }
            });

            runs.push(TextRun {
                text: txt,
                left,
                right,
                bottom,
                top,
                font_size,
                bold,
                italic,
                link,
            });
        }
    }

    (page_w, page_h, runs)
}

/// Groups runs into lines sorted top-to-bottom (descending PDF `y`), respecting 2-column layouts.
fn group_runs_into_lines(page_w: f32, mut runs: Vec<TextRun>) -> Vec<TextLine> {
    if runs.is_empty() {
        return Vec::new();
    }

    // Detect clean 2-column split: many runs strictly on left half & strictly on right half,
    // with very few runs crossing the central gutter.
    let mid = page_w * 0.5;
    let gutter = page_w * 0.03;
    let mut left_col = Vec::new();
    let mut right_col = Vec::new();
    let mut spanning = Vec::new();
    for r in &runs {
        if r.right <= mid + gutter * 0.5 {
            left_col.push(r.clone());
        } else if r.left >= mid - gutter * 0.5 {
            right_col.push(r.clone());
        } else {
            spanning.push(r.clone());
        }
    }
    let is_two_col = left_col.len() >= 8
        && right_col.len() >= 8
        && spanning.len() * 5 < runs.len();

    if is_two_col {
        // Put spanning headers above the 2-column body, then left column, then right column.
        let mut lines = group_single_column_lines(spanning);
        let col_top = left_col
            .iter()
            .chain(right_col.iter())
            .map(|r| r.top)
            .fold(0.0f32, f32::max);
        let (mut header_lines, mut footer_lines): (Vec<_>, Vec<_>) =
            lines.drain(..).partition(|l| l.bottom >= col_top - 4.0);
        header_lines.extend(group_single_column_lines(left_col));
        header_lines.extend(group_single_column_lines(right_col));
        header_lines.append(&mut footer_lines);
        return header_lines;
    }

    runs.sort_by(|a, b| {
        b.top
            .partial_cmp(&a.top)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.left.partial_cmp(&b.left).unwrap_or(std::cmp::Ordering::Equal))
    });
    group_single_column_lines(runs)
}

fn group_single_column_lines(mut runs: Vec<TextRun>) -> Vec<TextLine> {
    if runs.is_empty() {
        return Vec::new();
    }
    runs.sort_by(|a, b| {
        b.top
            .partial_cmp(&a.top)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.left.partial_cmp(&b.left).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lines: Vec<TextLine> = Vec::new();
    for r in runs {
        let r_mid_y = (r.bottom + r.top) * 0.5;
        let tol = (r.font_size * 0.55).max(3.5);
        let mut placed = false;
        for line in lines.iter_mut().rev().take(4) {
            let l_mid_y = (line.bottom + line.top) * 0.5;
            if (l_mid_y - r_mid_y).abs() <= tol {
                line.left = line.left.min(r.left);
                line.right = line.right.max(r.right);
                line.bottom = line.bottom.min(r.bottom);
                line.top = line.top.max(r.top);
                line.max_font = line.max_font.max(r.font_size);
                line.runs.push(r.clone());
                placed = true;
                break;
            }
        }
        if !placed {
            lines.push(TextLine {
                left: r.left,
                right: r.right,
                bottom: r.bottom,
                top: r.top,
                max_font: r.font_size,
                avg_font: r.font_size,
                all_bold: r.bold,
                runs: vec![r],
            });
        }
    }

    for l in &mut lines {
        l.runs.sort_by(|a, b| {
            a.left
                .partial_cmp(&b.left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut weighted = 0.0f32;
        let mut chars = 0usize;
        let mut all_bold = true;
        for r in &l.runs {
            let c = r.text.trim().chars().count();
            if c > 0 {
                weighted += r.font_size * c as f32;
                chars += c;
                if !r.bold {
                    all_bold = false;
                }
            }
        }
        l.avg_font = if chars > 0 { weighted / chars as f32 } else { l.max_font };
        l.all_bold = all_bold && chars > 0;
    }

    lines.sort_by(|a, b| {
        b.top
            .partial_cmp(&a.top)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    lines
}

fn split_line_into_table_cells(line: &TextLine) -> Option<Vec<String>> {
    if line.runs.len() < 2 {
        return None;
    }
    let gap_thresh = (line.avg_font * 1.45).max(16.0);
    let mut cells: Vec<Vec<TextRun>> = vec![vec![line.runs[0].clone()]];
    for r in line.runs.iter().skip(1) {
        let prev_r = cells.last().unwrap().last().unwrap().right;
        if r.left - prev_r >= gap_thresh {
            cells.push(vec![r.clone()]);
        } else {
            cells.last_mut().unwrap().push(r.clone());
        }
    }
    if cells.len() >= 2 {
        Some(cells.into_iter().map(|c| join_runs_plain(&c)).collect())
    } else {
        None
    }
}

fn parse_bullet_prefix(text: &str) -> Option<&str> {
    let trimmed = text.trim_start();
    for prefix in ["• ", "● ", "◦ ", "▪ ", "▫ ", "– ", "— ", "- ", "* "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    if let Some(rest) = trimmed.strip_prefix('•') {
        if !rest.is_empty() {
            return Some(rest.trim_start());
        }
    }
    None
}

fn parse_numbered_prefix(text: &str) -> Option<(String, &str)> {
    let trimmed = text.trim_start();
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() && i < 4 && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= bytes.len() {
        return None;
    }
    if (bytes[i] == b'.' || bytes[i] == b')') && i + 1 < bytes.len() && bytes[i + 1] == b' ' {
        let num = trimmed[..i].to_string();
        let rest = trimmed[i + 2..].trim_start();
        if !rest.is_empty() {
            return Some((num, rest));
        }
    }
    None
}

fn strip_leading_chars_from_spans(mut spans: Vec<StyledSpan>, mut drop_chars: usize) -> Vec<StyledSpan> {
    while drop_chars > 0 && !spans.is_empty() {
        let first = &mut spans[0];
        let count = first.text.chars().count();
        if count <= drop_chars {
            drop_chars -= count;
            spans.remove(0);
        } else {
            first.text = first.text.chars().skip(drop_chars).collect();
            drop_chars = 0;
        }
    }
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    spans.retain(|s| !s.text.is_empty());
    spans
}

fn classify_lines_into_blocks(lines: &[TextLine], body_font: f32) -> Vec<ExportBlock> {
    let mut blocks: Vec<ExportBlock> = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = &lines[i];
        let plain = line.plain_text();
        if plain.is_empty() {
            i += 1;
            continue;
        }

        // Check for a multi-row simple table (>= 2 consecutive lines with identical cell count >= 2)
        if let Some(first_row) = split_line_into_table_cells(line) {
            let cols = first_row.len();
            let mut rows = vec![first_row];
            let mut j = i + 1;
            while j < lines.len() {
                let vgap = lines[j - 1].bottom - lines[j].top;
                if vgap > body_font * 2.5 {
                    break;
                }
                if let Some(next_row) = split_line_into_table_cells(&lines[j]) {
                    if next_row.len() == cols {
                        rows.push(next_row);
                        j += 1;
                        continue;
                    }
                }
                break;
            }
            if rows.len() >= 2 {
                blocks.push(ExportBlock::Table(rows));
                i = j;
                continue;
            }
        }

        // Heading detection by font ratio or short standalone bold line
        let ratio = line.avg_font / body_font.max(6.0);
        let is_short = plain.chars().count() <= 110 && !plain.ends_with('.');
        if ratio >= 1.55 && is_short {
            blocks.push(ExportBlock::Heading { level: 1, text: plain });
            i += 1;
            continue;
        }
        if ratio >= 1.28 && is_short {
            blocks.push(ExportBlock::Heading { level: 2, text: plain });
            i += 1;
            continue;
        }
        if (ratio >= 1.12 || (line.all_bold && ratio >= 0.98 && plain.chars().count() <= 75))
            && is_short
            && parse_bullet_prefix(&plain).is_none()
        {
            blocks.push(ExportBlock::Heading { level: 3, text: plain });
            i += 1;
            continue;
        }

        // Bullet item
        if let Some(rest) = parse_bullet_prefix(&plain) {
            let drop_n = plain.chars().count() - rest.chars().count();
            let spans = strip_leading_chars_from_spans(runs_to_styled_spans(&line.runs), drop_n);
            blocks.push(ExportBlock::BulletItem(spans));
            i += 1;
            continue;
        }

        // Numbered list item
        if let Some((num, rest)) = parse_numbered_prefix(&plain) {
            let drop_n = plain.chars().count() - rest.chars().count();
            let spans = strip_leading_chars_from_spans(runs_to_styled_spans(&line.runs), drop_n);
            blocks.push(ExportBlock::NumberedItem { num, spans });
            i += 1;
            continue;
        }

        // Paragraph: merge consecutive body lines when vertical spacing is normal
        let mut para_runs = line.runs.clone();
        let mut j = i + 1;
        while j < lines.len() {
            let prev = &lines[j - 1];
            let next = &lines[j];
            let next_plain = next.plain_text();
            if next_plain.is_empty() {
                break;
            }
            let vgap = prev.bottom - next.top;
            let next_ratio = next.avg_font / body_font.max(6.0);
            if vgap > body_font * 0.95
                || next_ratio >= 1.15
                || parse_bullet_prefix(&next_plain).is_some()
                || parse_numbered_prefix(&next_plain).is_some()
                || split_line_into_table_cells(next).is_some()
            {
                break;
            }
            // Join hyphenated words across line breaks cleanly
            if let Some(last_r) = para_runs.last_mut() {
                if last_r.text.ends_with('-') && !last_r.text.ends_with(" -") {
                    last_r.text.pop();
                } else {
                    last_r.text.push(' ');
                }
            }
            para_runs.extend(next.runs.iter().cloned());
            j += 1;
        }

        let spans = runs_to_styled_spans(&para_runs);
        if !spans.is_empty() {
            blocks.push(ExportBlock::Paragraph(spans));
        }
        i = j;
    }

    blocks
}

fn estimate_body_font_size(all_pages: &[Vec<TextLine>]) -> f32 {
    let mut hist: HashMap<i32, usize> = HashMap::new();
    for lines in all_pages {
        for l in lines {
            let key = (l.avg_font * 2.0).round() as i32;
            let chars = l.plain_text().chars().count();
            *hist.entry(key).or_insert(0) += chars;
        }
    }
    hist.into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(k, _)| (k as f32 * 0.5).clamp(7.0, 24.0))
        .unwrap_or(11.0)
}

/// Identifies running headers/footers repeated verbatim on >= 3 pages (and >= 40% of pages).
fn detect_repeating_margin_text(all_pages: &[Vec<TextLine>]) -> std::collections::HashSet<String> {
    let mut repeated = std::collections::HashSet::new();
    if all_pages.len() < 3 {
        return repeated;
    }
    let mut top_counts: HashMap<String, usize> = HashMap::new();
    let mut bot_counts: HashMap<String, usize> = HashMap::new();
    for lines in all_pages {
        if let Some(first) = lines.first() {
            let t = first.plain_text();
            if !t.is_empty() && t.len() < 120 {
                *top_counts.entry(t).or_insert(0) += 1;
            }
        }
        if let Some(last) = lines.last() {
            let t = last.plain_text();
            if !t.is_empty() && t.len() < 120 {
                *bot_counts.entry(t).or_insert(0) += 1;
            }
        }
    }
    let threshold = (all_pages.len() * 2 / 5).max(3);
    for (k, c) in top_counts.into_iter().chain(bot_counts) {
        if c >= threshold {
            repeated.insert(k);
        }
    }
    repeated
}

fn extract_document_blocks(doc: &PdfDocument) -> Vec<Vec<ExportBlock>> {
    let total = doc.pages().len();
    let mut page_lines: Vec<Vec<TextLine>> = Vec::with_capacity(total as usize);
    for idx in 0..total {
        if let Ok(page) = doc.pages().get(idx) {
            let (pw, _ph, runs) = extract_page_runs(&page);
            page_lines.push(group_runs_into_lines(pw, runs));
        } else {
            page_lines.push(Vec::new());
        }
    }

    let body_font = estimate_body_font_size(&page_lines);
    let repeated = detect_repeating_margin_text(&page_lines);

    page_lines
        .into_iter()
        .map(|mut lines| {
            // Strip repeating running headers/footers and lone page-number lines
            if let Some(first) = lines.first() {
                let t = first.plain_text();
                if repeated.contains(&t) || (t.len() <= 5 && t.chars().all(|c| c.is_ascii_digit())) {
                    lines.remove(0);
                }
            }
            if let Some(last) = lines.last() {
                let t = last.plain_text();
                if repeated.contains(&t) || (t.len() <= 5 && t.chars().all(|c| c.is_ascii_digit())) {
                    lines.pop();
                }
            }
            classify_lines_into_blocks(&lines, body_font)
        })
        .collect()
}

fn format_spans_markdown(spans: &[StyledSpan]) -> String {
    let mut out = String::new();
    for s in spans {
        let trimmed = s.text.trim();
        if trimmed.is_empty() {
            if s.text.contains(' ') && !out.ends_with(' ') {
                out.push(' ');
            }
            continue;
        }
        let lead_sp = s.text.starts_with(' ');
        let trail_sp = s.text.ends_with(' ');
        if lead_sp && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        let mut core = trimmed.to_string();
        if s.bold && s.italic {
            core = format!("***{core}***");
        } else if s.bold {
            core = format!("**{core}**");
        } else if s.italic {
            core = format!("*{core}*");
        }
        if let Some(ref url) = s.link {
            core = format!("[{core}]({url})");
        }
        out.push_str(&core);
        if trail_sp {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Renders structured `ExportBlock`s per page into clean Markdown (`.md`).
pub fn render_blocks_to_markdown(pages: &[Vec<ExportBlock>]) -> String {
    let mut out = String::new();
    for (p_idx, blocks) in pages.iter().enumerate() {
        if p_idx > 0 && !out.is_empty() {
            out.push_str("\n---\n\n");
        }
        let mut prev_was_list = false;
        for block in blocks {
            match block {
                ExportBlock::Heading { level, text } => {
                    if prev_was_list {
                        out.push('\n');
                    }
                    let hashes = match level {
                        1 => "#",
                        2 => "##",
                        _ => "###",
                    };
                    out.push_str(&format!("{hashes} {text}\n\n"));
                    prev_was_list = false;
                }
                ExportBlock::Paragraph(spans) => {
                    if prev_was_list {
                        out.push('\n');
                    }
                    let line = format_spans_markdown(spans);
                    if !line.is_empty() {
                        out.push_str(&line);
                        out.push_str("\n\n");
                    }
                    prev_was_list = false;
                }
                ExportBlock::BulletItem(spans) => {
                    let line = format_spans_markdown(spans);
                    if !line.is_empty() {
                        out.push_str(&format!("- {line}\n"));
                        prev_was_list = true;
                    }
                }
                ExportBlock::NumberedItem { num, spans } => {
                    let line = format_spans_markdown(spans);
                    if !line.is_empty() {
                        out.push_str(&format!("{num}. {line}\n"));
                        prev_was_list = true;
                    }
                }
                ExportBlock::Table(rows) => {
                    if prev_was_list {
                        out.push('\n');
                    }
                    if let Some(header) = rows.first() {
                        let cols = header.len();
                        out.push('|');
                        for c in header {
                            out.push_str(&format!(" {} |", c.replace('|', "\\|")));
                        }
                        out.push('\n');
                        out.push('|');
                        for _ in 0..cols {
                            out.push_str(" --- |");
                        }
                        out.push('\n');
                        for r in rows.iter().skip(1) {
                            out.push('|');
                            for c in r {
                                out.push_str(&format!(" {} |", c.replace('|', "\\|")));
                            }
                            out.push('\n');
                        }
                        out.push('\n');
                    }
                    prev_was_list = false;
                }
            }
        }
        if prev_was_list {
            out.push('\n');
        }
    }
    out.trim_end().to_string() + "\n"
}

pub fn escape_latex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '$' => out.push_str("\\$"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '^' => out.push_str("\\textasciicircum{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '—' => out.push_str("---"),
            '–' => out.push_str("--"),
            '“' => out.push_str("``"),
            '”' => out.push_str("''"),
            '‘' => out.push_str("`"),
            '’' => out.push('\''),
            '±' => out.push_str("$\\pm$"),
            '×' => out.push_str("$\\times$"),
            '÷' => out.push_str("$\\div$"),
            '≤' => out.push_str("$\\leq$"),
            '≥' => out.push_str("$\\geq$"),
            '≠' => out.push_str("$\\neq$"),
            '≈' => out.push_str("$\\approx$"),
            '∞' => out.push_str("$\\infty$"),
            '→' => out.push_str("$\\rightarrow$"),
            '←' => out.push_str("$\\leftarrow$"),
            '∑' => out.push_str("$\\sum$"),
            '∫' => out.push_str("$\\int$"),
            'α' => out.push_str("$\\alpha$"),
            'β' => out.push_str("$\\beta$"),
            'γ' => out.push_str("$\\gamma$"),
            'δ' => out.push_str("$\\delta$"),
            'λ' => out.push_str("$\\lambda$"),
            'μ' => out.push_str("$\\mu$"),
            'π' => out.push_str("$\\pi$"),
            'σ' => out.push_str("$\\sigma$"),
            'Ω' => out.push_str("$\\Omega$"),
            _ => out.push(ch),
        }
    }
    out
}

fn format_spans_latex(spans: &[StyledSpan]) -> String {
    let mut out = String::new();
    for s in spans {
        let trimmed = s.text.trim();
        if trimmed.is_empty() {
            if s.text.contains(' ') && !out.ends_with(' ') {
                out.push(' ');
            }
            continue;
        }
        let lead_sp = s.text.starts_with(' ');
        let trail_sp = s.text.ends_with(' ');
        if lead_sp && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        let mut core = escape_latex(trimmed);
        if s.bold && s.italic {
            core = format!("\\textbf{{\\textit{{{core}}}}}");
        } else if s.bold {
            core = format!("\\textbf{{{core}}}");
        } else if s.italic {
            core = format!("\\textit{{{core}}}");
        }
        if let Some(ref url) = s.link {
            let safe_url = url.replace('%', "\\%").replace('#', "\\#");
            core = format!("\\href{{{safe_url}}}{{{core}}}");
        }
        out.push_str(&core);
        if trail_sp {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Renders structured `ExportBlock`s per page into a standalone compilable LaTeX (`.tex`) document.
pub fn render_blocks_to_latex(doc_title: &str, pages: &[Vec<ExportBlock>]) -> String {
    let mut out = String::new();
    out.push_str("% Exported by MicroPDF (pure-Rust layout extractor)\n");
    out.push_str("\\documentclass[11pt]{article}\n");
    out.push_str("\\usepackage[utf8]{inputenc}\n");
    out.push_str("\\usepackage[T1]{fontenc}\n");
    out.push_str("\\usepackage[margin=1in]{geometry}\n");
    out.push_str("\\usepackage{amsmath,amssymb}\n");
    out.push_str("\\usepackage{hyperref}\n");
    out.push_str(&format!("\\title{{{}}}\n", escape_latex(doc_title)));
    out.push_str("\\date{}\n\n");
    out.push_str("\\begin{document}\n\n");

    #[derive(Copy, Clone, PartialEq, Eq)]
    enum ListKind {
        None,
        Itemize,
        Enumerate,
    }

    let close_list = |out: &mut String, list: &mut ListKind| {
        match *list {
            ListKind::Itemize => out.push_str("\\end{itemize}\n\n"),
            ListKind::Enumerate => out.push_str("\\end{enumerate}\n\n"),
            ListKind::None => {}
        }
        *list = ListKind::None;
    };

    for (p_idx, blocks) in pages.iter().enumerate() {
        if p_idx > 0 {
            out.push_str(&format!("% --- Page {} ---\n", p_idx + 1));
        }
        let mut list = ListKind::None;
        for block in blocks {
            match block {
                ExportBlock::Heading { level, text } => {
                    close_list(&mut out, &mut list);
                    let cmd = match level {
                        1 => "\\section",
                        2 => "\\subsection",
                        _ => "\\subsubsection",
                    };
                    out.push_str(&format!("{cmd}{{{}}}\n\n", escape_latex(text)));
                }
                ExportBlock::Paragraph(spans) => {
                    close_list(&mut out, &mut list);
                    let line = format_spans_latex(spans);
                    if !line.is_empty() {
                        out.push_str(&line);
                        out.push_str("\n\n");
                    }
                }
                ExportBlock::BulletItem(spans) => {
                    if list != ListKind::Itemize {
                        close_list(&mut out, &mut list);
                        out.push_str("\\begin{itemize}\n");
                        list = ListKind::Itemize;
                    }
                    let line = format_spans_latex(spans);
                    if !line.is_empty() {
                        out.push_str(&format!("  \\item {line}\n"));
                    }
                }
                ExportBlock::NumberedItem { spans, .. } => {
                    if list != ListKind::Enumerate {
                        close_list(&mut out, &mut list);
                        out.push_str("\\begin{enumerate}\n");
                        list = ListKind::Enumerate;
                    }
                    let line = format_spans_latex(spans);
                    if !line.is_empty() {
                        out.push_str(&format!("  \\item {line}\n"));
                    }
                }
                ExportBlock::Table(rows) => {
                    close_list(&mut out, &mut list);
                    if let Some(header) = rows.first() {
                        let cols = header.len().max(1);
                        let spec = "l".repeat(cols);
                        out.push_str(&format!("\\begin{{tabular}}{{{spec}}}\n\\hline\n"));
                        for (r_idx, r) in rows.iter().enumerate() {
                            let cells: Vec<String> = r.iter().map(|c| escape_latex(c)).collect();
                            out.push_str(&cells.join(" & "));
                            out.push_str(" \\\\\n");
                            if r_idx == 0 {
                                out.push_str("\\hline\n");
                            }
                        }
                        out.push_str("\\hline\n\\end{tabular}\n\n");
                    }
                }
            }
        }
        close_list(&mut out, &mut list);
    }

    out.push_str("\\end{document}\n");
    out
}

/// Converts an open `PdfDocument` to Markdown (`.md`) using pure-Rust layout heuristics.
pub fn export_document_to_markdown(doc: &PdfDocument) -> String {
    let pages = extract_document_blocks(doc);
    render_blocks_to_markdown(&pages)
}

/// Converts an open `PdfDocument` to standalone LaTeX (`.tex`) using pure-Rust layout heuristics.
pub fn export_document_to_latex(doc: &PdfDocument, doc_title: &str) -> String {
    let pages = extract_document_blocks(doc);
    render_blocks_to_latex(doc_title, &pages)
}

/// Extracts reflowable plain-text paragraphs for the single-page Text View mode.
pub fn extract_page_text_blocks(page: &PdfPage) -> Vec<ExportBlock> {
    let (pw, _ph, runs) = extract_page_runs(page);
    if !runs.is_empty() {
        let lines = group_runs_into_lines(pw, runs);
        let body = estimate_body_font_size(std::slice::from_ref(&lines));
        return classify_lines_into_blocks(&lines, body);
    }
    // Fallback to PDFium's `page.text().all()` if objects returned nothing
    if let Ok(tp) = page.text() {
        let all = tp.all();
        let trimmed = all.trim();
        if !trimmed.is_empty() {
            return trimmed
                .split("\n\n")
                .map(|p| {
                    ExportBlock::Paragraph(vec![StyledSpan {
                        text: collapse_spaces(p),
                        bold: false,
                        italic: false,
                        link: None,
                    }])
                })
                .collect();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_and_latex_rendering_from_blocks() {
        let pages = vec![vec![
            ExportBlock::Heading {
                level: 1,
                text: "Annual Report 2026".to_string(),
            },
            ExportBlock::Paragraph(vec![
                StyledSpan { text: "Revenue grew by ".to_string(), bold: false, italic: false, link: None },
                StyledSpan { text: "25%".to_string(), bold: true, italic: false, link: None },
                StyledSpan { text: " (α ≈ 0.05) — see ".to_string(), bold: false, italic: false, link: None },
                StyledSpan {
                    text: "MicroPDF".to_string(),
                    bold: false,
                    italic: true,
                    link: Some("https://example.com".to_string()),
                },
            ]),
            ExportBlock::BulletItem(vec![StyledSpan {
                text: "Zero GPU & tiny footprint".to_string(),
                bold: false,
                italic: false,
                link: None,
            }]),
            ExportBlock::Table(vec![
                vec!["Metric".to_string(), "Value".to_string()],
                vec!["RAM".to_string(), "16 MB".to_string()],
            ]),
        ]];

        let md = render_blocks_to_markdown(&pages);
        assert!(md.contains("# Annual Report 2026"));
        assert!(md.contains("**25%**"));
        assert!(md.contains("[*MicroPDF*](https://example.com)"));
        assert!(md.contains("- Zero GPU & tiny footprint"));
        assert!(md.contains("| Metric | Value |"));
        assert!(md.contains("| RAM | 16 MB |"));

        let tex = render_blocks_to_latex("Report_2026", &pages);
        assert!(tex.contains("\\documentclass[11pt]{article}"));
        assert!(tex.contains("\\title{Report\\_2026}"));
        assert!(tex.contains("\\section{Annual Report 2026}"));
        assert!(tex.contains("\\textbf{25\\%}"));
        assert!(tex.contains("$\\alpha$"));
        assert!(tex.contains("$\\approx$"));
        assert!(tex.contains("\\href{https://example.com}{\\textit{MicroPDF}}"));
        assert!(tex.contains("\\begin{itemize}"));
        assert!(tex.contains("\\begin{tabular}{ll}"));
        assert!(tex.contains("\\end{document}"));
    }

    #[test]
    fn test_escape_latex_special_characters() {
        assert_eq!(escape_latex("50% & $10 #1 _a {b}"), "50\\% \\& \\$10 \\#1 \\_a \\{b\\}");
    }
}
