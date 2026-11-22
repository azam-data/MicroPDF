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

    fn collect_object_runs(
        obj: &PdfPageObject,
        links: &[(PdfRect, String)],
        runs: &mut Vec<TextRun>,
        depth: u8,
    ) {
        if let Some(text_obj) = obj.as_text_object() {
            let txt = text_obj.text();
            if txt.trim().is_empty() {
                return;
            }
            let Ok(b) = obj.bounds() else { return };
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
                    .map(|w| {
                        matches!(
                            w,
                            PdfFontWeight::Weight600
                                | PdfFontWeight::Weight700Bold
                                | PdfFontWeight::Weight800
                                | PdfFontWeight::Weight900
                        )
                    })
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
        } else if depth < 4 {
            if let Some(form) = obj.as_x_object_form_object() {
                for child in form.iter() {
                    collect_object_runs(&child, links, runs, depth + 1);
                }
            }
        }
    }

    let mut runs = Vec::new();
    for obj in page.objects().iter() {
        collect_object_runs(&obj, &links, &mut runs, 0);
    }

    (page_w, page_h, runs)
}

/// Returns true if a trimmed token looks like a numeric/tabular value (numbers, currency,
/// percentages, dates, dashes, or accounting parentheses like `(1,234.50)`).
pub(crate) fn is_numeric_cell(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if matches!(t, "-" | "–" | "—" | "N/A" | "n/a") {
        return true;
    }
    let mut digits = 0usize;
    let mut letters = 0usize;
    for ch in t.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
        } else if ch.is_alphabetic() {
            letters += 1;
        } else if !matches!(ch, '.' | ',' | '%' | '$' | '€' | '£' | '¥' | '₹' | '+' | '-' | '−' | '(' | ')' | '/' | ':' | ' ') {
            return false;
        }
    }
    digits > 0 && letters <= 2
}

/// Groups runs into lines sorted top-to-bottom (descending PDF `y`), respecting 2-column prose
/// layouts WITHOUT accidentally splitting multi-column tables (e.g. Excel/Word tables with
/// text labels on the left and numbers on the right).
fn group_runs_into_lines(page_w: f32, mut runs: Vec<TextRun>) -> Vec<TextLine> {
    if runs.is_empty() {
        return Vec::new();
    }

    // First group horizontally across the full page width to inspect whether rows are tabular.
    let full_rows = group_single_column_lines(runs.clone());

    // Detect clean 2-column prose split ONLY when left & right halves are genuinely independent
    // wrapped prose columns (NOT aligned table rows with numbers or 3+ cells per row!).
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

    let candidate_two_col = left_col.len() >= 8
        && right_col.len() >= 8
        && spanning.len() * 5 < runs.len();

    let is_two_col = if candidate_two_col {
        let mut rows_3plus_cells = 0usize;
        let mut rows_both_sides = 0usize;
        let mut right_numeric_runs = 0usize;
        let mut right_short_rows = 0usize;
        let mut left_lines_count = 0usize;

        for row in &full_rows {
            let cells = split_line_into_cell_spans(row);
            if cells.len() >= 3 {
                rows_3plus_cells += 1;
            }
            let has_left = row.runs.iter().any(|r| r.right <= mid + gutter * 0.5);
            let has_right = row.runs.iter().any(|r| r.left >= mid - gutter * 0.5);
            if has_left {
                left_lines_count += 1;
            }
            if has_left && has_right {
                rows_both_sides += 1;
                let right_txt: String = row
                    .runs
                    .iter()
                    .filter(|r| r.left >= mid - gutter * 0.5)
                    .map(|r| r.text.trim())
                    .collect::<Vec<_>>()
                    .join(" ");
                if right_txt.chars().count() <= 22 {
                    right_short_rows += 1;
                }
            }
        }
        for r in &right_col {
            if is_numeric_cell(&r.text) {
                right_numeric_runs += 1;
            }
        }
        let right_lines = group_single_column_lines(right_col.clone());
        let avg_right_chars: f32 = if right_lines.is_empty() {
            0.0
        } else {
            right_lines
                .iter()
                .map(|l| l.plain_text().chars().count() as f32)
                .sum::<f32>()
                / right_lines.len() as f32
        };

        // Tabular indicators:
        // 1. Multiple rows have >= 3 separated cells (Excel/Word multi-column table), OR
        // 2. Right half is predominantly numeric values (financial/summary tables), OR
        // 3. Most rows align across left & right AND right-side entries are short labels/values (not wrapped prose).
        let looks_tabular = rows_3plus_cells >= 2
            || right_numeric_runs * 3 >= right_col.len()
            || (left_lines_count >= 3
                && rows_both_sides * 2 >= left_lines_count
                && (right_short_rows * 2 >= rows_both_sides || avg_right_chars < 24.0));

        !looks_tabular
    } else {
        false
    };

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

fn split_line_into_cell_spans(line: &TextLine) -> Vec<(f32, f32, String)> {
    if line.runs.is_empty() {
        return Vec::new();
    }
    let gap_thresh = (line.avg_font * 1.35).max(12.0);
    let mut groups: Vec<Vec<TextRun>> = vec![vec![line.runs[0].clone()]];
    for r in line.runs.iter().skip(1) {
        let prev_r = groups.last().unwrap().last().unwrap().right;
        if r.left - prev_r >= gap_thresh {
            groups.push(vec![r.clone()]);
        } else {
            groups.last_mut().unwrap().push(r.clone());
        }
    }
    let mut spans = Vec::with_capacity(groups.len());
    for g in groups {
        let left = g.iter().map(|r| r.left).fold(f32::INFINITY, f32::min);
        let right = g.iter().map(|r| r.right).fold(f32::NEG_INFINITY, f32::max);
        let txt = join_runs_plain(&g);
        if !txt.is_empty() {
            spans.push((left, right, txt));
        }
    }
    spans
}

/// Extracts a multi-row table starting at `start` by clustering cell X-intervals across
/// consecutive rows into canonical columns. Preserves blank cells and right-aligned numbers.
fn try_extract_column_table(
    lines: &[TextLine],
    start: usize,
    body_font: f32,
) -> Option<(Vec<Vec<String>>, usize)> {
    let first_spans = split_line_into_cell_spans(&lines[start]);
    if first_spans.len() < 2 {
        return None;
    }

    let mut row_spans: Vec<Vec<(f32, f32, String)>> = vec![first_spans];
    let mut j = start + 1;
    while j < lines.len() {
        let vgap = lines[j - 1].bottom - lines[j].top;
        if vgap > body_font * 2.8 || vgap < -body_font * 2.0 {
            break;
        }
        let ratio = lines[j].avg_font / body_font.max(6.0);
        if ratio >= 1.38 {
            break;
        }
        let spans = split_line_into_cell_spans(&lines[j]);
        if spans.len() >= 2 {
            row_spans.push(spans);
            j += 1;
        } else if spans.len() == 1 {
            // Allow a single-cell section/blank-tail row ONLY if another multi-cell row immediately follows.
            let next_is_multi = j + 1 < lines.len()
                && (lines[j].bottom - lines[j + 1].top) <= body_font * 2.8
                && split_line_into_cell_spans(&lines[j + 1]).len() >= 2;
            if next_is_multi {
                row_spans.push(spans);
                j += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Trim any trailing single-span rows.
    while row_spans.len() > 1 && row_spans.last().is_some_and(|r| r.len() < 2) {
        row_spans.pop();
    }
    let multi_rows = row_spans.iter().filter(|r| r.len() >= 2).count();
    if row_spans.len() < 2 || multi_rows < 2 {
        return None;
    }

    // Seed column X-intervals from multi-cell rows so wide 1-cell subheaders don't fuse columns.
    let mut seed_intervals: Vec<(f32, f32)> = Vec::new();
    for spans in &row_spans {
        if spans.len() >= 2 {
            for &(l, r, _) in spans {
                seed_intervals.push((l, r));
            }
        }
    }
    seed_intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let merge_tol = (body_font * 0.35).max(3.0);
    let mut cols: Vec<(f32, f32)> = Vec::new();
    for (l, r) in seed_intervals {
        if let Some(last) = cols.last_mut() {
            if l <= last.1 + merge_tol {
                last.1 = last.1.max(r);
                continue;
            }
        }
        cols.push((l, r));
    }

    if cols.len() < 2 || cols.len() > 14 {
        return None;
    }

    let mut table_rows: Vec<Vec<String>> = Vec::with_capacity(row_spans.len());
    for spans in &row_spans {
        let mut cells = vec![String::new(); cols.len()];
        for (l, r, txt) in spans {
            let mut best_col = 0usize;
            let mut best_overlap = f32::NEG_INFINITY;
            for (ci, &(cl, cr)) in cols.iter().enumerate() {
                let overlap = r.min(cr) - l.max(cl);
                if overlap > best_overlap {
                    best_overlap = overlap;
                    best_col = ci;
                }
            }
            if best_overlap <= 0.0 {
                // Fallback to nearest column center
                let mid = (l + r) * 0.5;
                let mut best_dist = f32::INFINITY;
                for (ci, &(cl, cr)) in cols.iter().enumerate() {
                    let cmid = (cl + cr) * 0.5;
                    let d = (mid - cmid).abs();
                    if d < best_dist {
                        best_dist = d;
                        best_col = ci;
                    }
                }
            }
            if cells[best_col].is_empty() {
                cells[best_col] = txt.clone();
            } else {
                cells[best_col].push(' ');
                cells[best_col].push_str(txt);
            }
        }
        table_rows.push(cells);
    }

    let end_idx = start + row_spans.len();
    Some((table_rows, end_idx))
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

        // Check for a multi-row column-aligned table (handles blank cells & right-aligned numbers)
        if let Some((rows, next_i)) = try_extract_column_table(lines, i, body_font) {
            blocks.push(ExportBlock::Table(rows));
            i = next_i;
            continue;
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
                || try_extract_column_table(lines, j, body_font).is_some()
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
    let mut fallback_text: Vec<Option<String>> = Vec::with_capacity(total as usize);
    for idx in 0..total {
        if let Ok(page) = doc.pages().get(idx) {
            let (pw, _ph, runs) = extract_page_runs(&page);
            if runs.is_empty() {
                let raw = page
                    .text()
                    .ok()
                    .map(|t| t.all())
                    .filter(|s| !s.trim().is_empty());
                page_lines.push(Vec::new());
                fallback_text.push(raw);
            } else {
                page_lines.push(group_runs_into_lines(pw, runs));
                fallback_text.push(None);
            }
        } else {
            page_lines.push(Vec::new());
            fallback_text.push(None);
        }
    }

    let body_font = estimate_body_font_size(&page_lines);
    let repeated = detect_repeating_margin_text(&page_lines);
    let strip_page_nums = total > 1;

    page_lines
        .into_iter()
        .zip(fallback_text)
        .map(|(mut lines, fb)| {
            if lines.is_empty() {
                if let Some(raw) = fb {
                    return raw
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(|l| {
                            ExportBlock::Paragraph(vec![StyledSpan {
                                text: l.to_string(),
                                bold: false,
                                italic: false,
                                link: None,
                            }])
                        })
                        .collect();
                }
                return Vec::new();
            }
            // Strip repeating running headers/footers and multi-page lone page-number lines
            if let Some(first) = lines.first() {
                let t = first.plain_text();
                if repeated.contains(&t)
                    || (strip_page_nums && t.len() <= 5 && t.chars().all(|c| c.is_ascii_digit()))
                {
                    lines.remove(0);
                }
            }
            if let Some(last) = lines.last() {
                let t = last.plain_text();
                if repeated.contains(&t)
                    || (strip_page_nums && t.len() <= 5 && t.chars().all(|c| c.is_ascii_digit()))
                {
                    lines.pop();
                }
            }
            classify_lines_into_blocks(&lines, body_font)
        })
        .collect()
}

/// Detects likely document headings `(0-based page_index, heading_level 0|1, title)` across `doc`
/// for one-click Bookmark generation on PDFs that lack an outline tree.
pub fn detect_document_headings(doc: &PdfDocument) -> Vec<(usize, u8, String)> {
    let pages = extract_document_blocks(doc);
    let mut out = Vec::new();
    for (p_idx, blocks) in pages.into_iter().enumerate() {
        for block in blocks {
            if let ExportBlock::Heading { level, text } = block {
                let clean = text.trim().to_string();
                if clean.len() >= 2 && clean.len() <= 140 {
                    let bm_lvl = if level <= 1 { 0 } else { 1 };
                    out.push((p_idx, bm_lvl, clean));
                }
            }
        }
    }
    out
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
            let safe_url = url
                .replace(' ', "%20")
                .replace('(', "%28")
                .replace(')', "%29");
            core = format!("[{core}]({safe_url})");
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
            let safe_url = url
                .replace('\\', "\\textbackslash{}")
                .replace('{', "\\{")
                .replace('}', "\\}")
                .replace('%', "\\%")
                .replace('#', "\\#")
                .replace('~', "\\~{}");
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
                    let line = format_spans_latex(spans);
                    if !line.is_empty() {
                        if list != ListKind::Itemize {
                            close_list(&mut out, &mut list);
                            out.push_str("\\begin{itemize}\n");
                            list = ListKind::Itemize;
                        }
                        out.push_str(&format!("  \\item {line}\n"));
                    }
                }
                ExportBlock::NumberedItem { spans, .. } => {
                    let line = format_spans_latex(spans);
                    if !line.is_empty() {
                        if list != ListKind::Enumerate {
                            close_list(&mut out, &mut list);
                            out.push_str("\\begin{enumerate}\n");
                            list = ListKind::Enumerate;
                        }
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
                            let mut row_line = cells.join(" & ");
                            if row_line.starts_with('[') {
                                row_line.insert_str(0, "{}");
                            }
                            out.push_str(&row_line);
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

/// Converts an open `PdfDocument` to an editable Microsoft Word OOXML (`.docx`) archive.
/// Uses pure-Rust ZIP + XML generation with existing `flate2` (zero new dependencies, zero idle RAM).
pub fn export_document_to_docx(doc: &PdfDocument, doc_title: &str) -> Vec<u8> {
    let pages = extract_document_blocks(doc);
    render_blocks_to_docx(doc_title, &pages)
}

/// Converts an open `PdfDocument` to a Microsoft Excel OOXML (`.xlsx`) workbook using the
/// Hybrid Lattice + Stream engine (`AutoHybrid`, stacked sheet, comma-decimal normalization on).
#[allow(dead_code)]
pub fn export_document_to_xlsx(doc: &PdfDocument) -> Vec<u8> {
    export_document_to_xlsx_with_options(doc, ExcelExportOptions::default())
}

// ---------------------------------------------------------------------------
// Minimal Pure-Rust ZIP Writer (IEEE CRC32 + flate2 raw Deflate)
// ---------------------------------------------------------------------------

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

struct MiniZipWriter {
    buf: Vec<u8>,
    entries: Vec<(String, u32, u32, u32, u16, u32)>,
}

impl MiniZipWriter {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(8192),
            entries: Vec::new(),
        }
    }

    fn add_file(&mut self, name: &str, data: &[u8]) {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;

        let crc = crc32_ieee(data);
        let uncomp_len = data.len() as u32;
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        let deflated = if enc.write_all(data).is_ok() {
            enc.finish().ok()
        } else {
            None
        };
        let (method, payload): (u16, &[u8]) = match deflated.as_deref() {
            Some(c) if c.len() < data.len() => (8, c),
            _ => (0, data),
        };
        let comp_len = payload.len() as u32;
        let offset = self.buf.len() as u32;
        let name_bytes = name.as_bytes();

        // Local file header (PK\x03\x04)
        self.buf.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        self.buf.extend_from_slice(&20u16.to_le_bytes()); // version needed 2.0
        self.buf.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 filename flag
        self.buf.extend_from_slice(&method.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
        self.buf.extend_from_slice(&0x5821u16.to_le_bytes()); // mod date (2024-01-01)
        self.buf.extend_from_slice(&crc.to_le_bytes());
        self.buf.extend_from_slice(&comp_len.to_le_bytes());
        self.buf.extend_from_slice(&uncomp_len.to_le_bytes());
        self.buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // extra len
        self.buf.extend_from_slice(name_bytes);
        self.buf.extend_from_slice(payload);

        self.entries.push((name.to_string(), crc, comp_len, uncomp_len, method, offset));
    }

    fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.buf.len() as u32;
        for (name, crc, comp_len, uncomp_len, method, offset) in &self.entries {
            let nb = name.as_bytes();
            // Central directory file header (PK\x01\x02)
            self.buf.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            self.buf.extend_from_slice(&20u16.to_le_bytes()); // version made by
            self.buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
            self.buf.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
            self.buf.extend_from_slice(&method.to_le_bytes());
            self.buf.extend_from_slice(&0u16.to_le_bytes());
            self.buf.extend_from_slice(&0x5821u16.to_le_bytes());
            self.buf.extend_from_slice(&crc.to_le_bytes());
            self.buf.extend_from_slice(&comp_len.to_le_bytes());
            self.buf.extend_from_slice(&uncomp_len.to_le_bytes());
            self.buf.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // extra
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // comment
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // disk start
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // internal attr
            self.buf.extend_from_slice(&0u32.to_le_bytes()); // external attr
            self.buf.extend_from_slice(&offset.to_le_bytes());
            self.buf.extend_from_slice(nb);
        }
        let cd_size = (self.buf.len() as u32).saturating_sub(cd_offset);
        let count = self.entries.len() as u16;
        // End of central directory record (PK\x05\x06)
        self.buf.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(&count.to_le_bytes());
        self.buf.extend_from_slice(&count.to_le_bytes());
        self.buf.extend_from_slice(&cd_size.to_le_bytes());
        self.buf.extend_from_slice(&cd_offset.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Strip illegal XML 1.0 control characters and non-characters (U+FFFE, U+FFFF)
            c if ((c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r')
                || c == '\u{FFFE}'
                || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pure-Rust Word (.docx) OOXML Generator
// ---------------------------------------------------------------------------

pub fn render_blocks_to_docx(doc_title: &str, pages: &[Vec<ExportBlock>]) -> Vec<u8> {
    let mut links: Vec<(String, String)> = Vec::new();
    let mut body = String::with_capacity(4096);

    let push_run = |dst: &mut String, text: &str, bold: bool, italic: bool, is_link: bool| {
        if text.is_empty() {
            return;
        }
        dst.push_str("<w:r>");
        if bold || italic || is_link {
            dst.push_str("<w:rPr>");
            if bold {
                dst.push_str("<w:b/>");
            }
            if italic {
                dst.push_str("<w:i/>");
            }
            if is_link {
                dst.push_str("<w:color w:val=\"1D4ED8\"/><w:u w:val=\"single\"/>");
            }
            dst.push_str("</w:rPr>");
        }
        dst.push_str("<w:t xml:space=\"preserve\">");
        dst.push_str(&xml_escape(text));
        dst.push_str("</w:t></w:r>");
    };

    let mut push_spans = |dst: &mut String, spans: &[StyledSpan]| {
        for sp in spans {
            if let Some(ref url) = sp.link {
                let rid = format!("rId{}", links.len() + 2); // rId1 is styles.xml
                links.push((rid.clone(), url.clone()));
                dst.push_str(&format!("<w:hyperlink r:id=\"{rid}\">"));
                push_run(dst, &sp.text, sp.bold, sp.italic, true);
                dst.push_str("</w:hyperlink>");
            } else {
                push_run(dst, &sp.text, sp.bold, sp.italic, false);
            }
        }
    };

    let mut any_content = false;
    for (p_idx, blocks) in pages.iter().enumerate() {
        if p_idx > 0 && any_content && !blocks.is_empty() {
            body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
        }
        for block in blocks {
            any_content = true;
            match block {
                ExportBlock::Heading { level, text } => {
                    let style = match level {
                        1 => "Heading1",
                        2 => "Heading2",
                        _ => "Heading3",
                    };
                    body.push_str(&format!(
                        "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>"
                    ));
                    push_run(&mut body, text, true, false, false);
                    body.push_str("</w:p>");
                }
                ExportBlock::Paragraph(spans) => {
                    body.push_str("<w:p>");
                    push_spans(&mut body, spans);
                    body.push_str("</w:p>");
                }
                ExportBlock::BulletItem(spans) => {
                    body.push_str("<w:p><w:pPr><w:ind w:left=\"360\" w:hanging=\"180\"/></w:pPr>");
                    push_run(&mut body, "• ", true, false, false);
                    push_spans(&mut body, spans);
                    body.push_str("</w:p>");
                }
                ExportBlock::NumberedItem { num, spans } => {
                    body.push_str("<w:p><w:pPr><w:ind w:left=\"360\" w:hanging=\"220\"/></w:pPr>");
                    let prefix = if num.ends_with('.') || num.ends_with(')') {
                        format!("{num} ")
                    } else {
                        format!("{num}. ")
                    };
                    push_run(&mut body, &prefix, true, false, false);
                    push_spans(&mut body, spans);
                    body.push_str("</w:p>");
                }
                ExportBlock::Table(rows) => {
                    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                    if cols == 0 {
                        continue;
                    }
                    let col_w = (9360usize / cols.max(1)).max(720);
                    body.push_str(
                        "<w:tbl><w:tblPr><w:tblW w:w=\"9360\" w:type=\"dxa\"/>\
                         <w:tblBorders>\
                         <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"CBD5E1\"/>\
                         <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"CBD5E1\"/>\
                         <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"CBD5E1\"/>\
                         <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"CBD5E1\"/>\
                         <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"E2E8F0\"/>\
                         <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"E2E8F0\"/>\
                         </w:tblBorders>\
                         <w:tblCellMar>\
                         <w:top w:w=\"60\" w:type=\"dxa\"/><w:bottom w:w=\"60\" w:type=\"dxa\"/>\
                         <w:left w:w=\"100\" w:type=\"dxa\"/><w:right w:w=\"100\" w:type=\"dxa\"/>\
                         </w:tblCellMar></w:tblPr><w:tblGrid>",
                    );
                    for _ in 0..cols {
                        body.push_str(&format!("<w:gridCol w:w=\"{col_w}\"/>"));
                    }
                    body.push_str("</w:tblGrid>");
                    for (r_idx, row) in rows.iter().enumerate() {
                        let is_hdr = r_idx == 0;
                        body.push_str("<w:tr>");
                        for c_idx in 0..cols {
                            let cell_txt = row.get(c_idx).map(|s| s.as_str()).unwrap_or("");
                            body.push_str(&format!(
                                "<w:tc><w:tcPr><w:tcW w:w=\"{col_w}\" w:type=\"dxa\"/>"
                            ));
                            if is_hdr {
                                body.push_str("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"EFF6FF\"/>");
                            } else if r_idx % 2 == 0 {
                                body.push_str("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F8FAFC\"/>");
                            }
                            body.push_str("</w:tcPr><w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr>");
                            push_run(&mut body, cell_txt, is_hdr, false, false);
                            body.push_str("</w:p></w:tc>");
                        }
                        body.push_str("</w:tr>");
                    }
                    body.push_str("</w:tbl><w:p/>");
                }
            }
        }
    }
    if !any_content {
        body.push_str("<w:p><w:r><w:t>(No extractable text found in PDF)</w:t></w:r></w:p>");
    }

    let document_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <w:body>{body}<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/>\
         <w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\"/></w:sectPr>\
         </w:body></w:document>"
    );

    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );
    for (rid, url) in &links {
        doc_rels.push_str(&format!(
            "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
            xml_escape(url)
        ));
    }
    doc_rels.push_str("</Relationships>");

    let styles_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
        <w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\"/>\
        <w:sz w:val=\"22\"/><w:color w:val=\"1E293B\"/></w:rPr></w:rPrDefault>\
        <w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
        <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/>\
        <w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:before=\"240\" w:after=\"100\"/></w:pPr>\
        <w:rPr><w:b/><w:sz w:val=\"32\"/><w:color w:val=\"0F172A\"/></w:rPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/>\
        <w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:before=\"200\" w:after=\"80\"/></w:pPr>\
        <w:rPr><w:b/><w:sz w:val=\"26\"/><w:color w:val=\"1E3A8A\"/></w:rPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/>\
        <w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:before=\"160\" w:after=\"60\"/></w:pPr>\
        <w:rPr><w:b/><w:sz w:val=\"24\"/><w:color w:val=\"334155\"/></w:rPr></w:style>\
        </w:styles>";

    let core_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
         <dc:title>{}</dc:title><dc:creator>MicroPDF</dc:creator></cp:coreProperties>",
        xml_escape(doc_title)
    );

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
        <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
        <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
        </Types>";

    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
        <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
        </Relationships>";

    let mut zip = MiniZipWriter::new();
    zip.add_file("[Content_Types].xml", content_types.as_bytes());
    zip.add_file("_rels/.rels", root_rels.as_bytes());
    zip.add_file("docProps/core.xml", core_xml.as_bytes());
    zip.add_file("word/document.xml", document_xml.as_bytes());
    zip.add_file("word/styles.xml", styles_xml.as_bytes());
    zip.add_file("word/_rels/document.xml.rels", doc_rels.as_bytes());
    zip.finish()
}

// ---------------------------------------------------------------------------
// Pure-Rust Excel (.xlsx) Generator (powered by rust_xlsxwriter)
// ---------------------------------------------------------------------------

/// Validates strict English thousands grouping (e.g. `1,234` or `12,345,678.90`) or plain
/// dot-decimal numbers (`1234.56`), rejecting European comma-decimals (`12,50`), comma lists
/// (`1,2,3`), leading-zero IDs (`00412`), and >15-digit account/card numbers so Excel never
/// corrupts or truncates them.
fn is_valid_grouped_number(num_part: &str) -> bool {
    let unsigned = num_part
        .strip_prefix('-')
        .or_else(|| num_part.strip_prefix('+'))
        .unwrap_or(num_part);
    if unsigned.is_empty() {
        return false;
    }
    let (int_part, frac_part) = match unsigned.split_once('.') {
        Some((i, f)) => {
            if f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            (i, Some(f))
        }
        None => (unsigned, None),
    };
    if int_part.is_empty() {
        return false;
    }
    // Preserve leading-zero identifiers (ZIP codes, account codes like "00123") as text
    if int_part.len() > 1 && int_part.starts_with('0') && !int_part.contains(',') {
        return false;
    }
    let digit_count = if int_part.contains(',') {
        let groups: Vec<&str> = int_part.split(',').collect();
        if groups.len() < 2
            || groups[0].is_empty()
            || groups[0].len() > 3
            || !groups[0].chars().all(|c| c.is_ascii_digit())
            || groups[0].starts_with('0')
        {
            return false;
        }
        for g in &groups[1..] {
            if g.len() != 3 || !g.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
        }
        groups.iter().map(|g| g.len()).sum::<usize>()
    } else {
        if !int_part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        int_part.len()
    };
    let total_digits = digit_count + frac_part.map_or(0, str::len);
    total_digits >= 1 && total_digits <= 15
}

/// Extraction mode for the Milestone 4 Hybrid Lattice + Stream PDF-to-Excel engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExcelExportMode {
    /// Automatically detects vector/ruled Lattice tables first and falls back to Stream for unruled regions.
    AutoHybrid,
    /// Strictly extracts ruled vector/thin-rect Lattice tables.
    Lattice,
    /// Uses whitespace-gutter Stream alignment across all page words.
    Stream,
}

/// Multi-table worksheet layout for `.xlsx` export.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExcelTableLayout {
    /// Stack all extracted tables vertically on one sheet separated by 2 blank rows.
    StackOnOneSheet,
    /// Create a separate worksheet tab per extracted table.
    SeparateSheets,
}

/// User-configurable options for PDF-to-Excel (`.xlsx`) export.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExcelExportOptions {
    pub mode: ExcelExportMode,
    pub layout: ExcelTableLayout,
    pub normalize_comma_decimals: bool,
}

impl Default for ExcelExportOptions {
    fn default() -> Self {
        Self {
            mode: ExcelExportMode::AutoHybrid,
            layout: ExcelTableLayout::StackOnOneSheet,
            normalize_comma_decimals: true,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HSegment {
    pub y: f32,
    pub x0: f32,
    pub x1: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VSegment {
    pub x: f32,
    pub y0: f32,
    pub y1: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WordBox {
    pub text: String,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
}

impl WordBox {
    #[inline]
    fn cx(&self) -> f32 {
        (self.left + self.right) * 0.5
    }
    #[inline]
    fn cy(&self) -> f32 {
        (self.bottom + self.top) * 0.5
    }
    #[inline]
    fn height(&self) -> f32 {
        (self.top - self.bottom).abs().max(1.0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CellMerge {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExcelTableGrid {
    pub rows: Vec<Vec<String>>,
    pub merges: Vec<CellMerge>,
    pub page_num: usize,
    pub is_lattice: bool,
    pub bbox: (f32, f32, f32, f32),
}

fn decimal_style_for_digits(frac_digits: usize, has_thousands_sep: bool) -> u8 {
    match frac_digits {
        0 => {
            if has_thousands_sep {
                3
            } else {
                5
            }
        }
        1 => 6,
        2 => 3,
        3 => 7,
        4 => 8,
        5 => 9,
        _ => 10,
    }
}

/// Pure-Rust financial number parser supporting standard thousands + decimals (`1,234.56`, `121.4000`)
/// and optional European/financial comma-decimal normalization (`123,9500`, `0,7728`, `-45,50`).
/// Preserves non-numeric strings (`N/A`, `USD`), leading-zero IDs (`00123`), and >15-digit codes.
pub fn parse_excel_numeric_with_opts(cell: &str, normalize_comma_decimals: bool) -> Option<(f64, u8)> {
    let s = cell.trim();
    if s.is_empty() || s.len() > 24 {
        return None;
    }
    let owned_norm: String;
    let s = if s.contains('\u{2212}') {
        owned_norm = s.replace('\u{2212}', "-");
        owned_norm.as_str()
    } else {
        s
    };

    if let Some(pct) = s.strip_suffix('%') {
        let pt = pct.trim();
        if let Some((v, _)) = parse_excel_numeric_with_opts(pt, normalize_comma_decimals) {
            return Some((v / 100.0, 4));
        }
        return None;
    }

    if is_valid_grouped_number(s) {
        let frac_digits = s.split_once('.').map_or(0, |(_, f)| f.len());
        let has_comma = s.contains(',');
        let cleaned = s.replace(',', "");
        if let Ok(v) = cleaned.parse::<f64>() {
            if v.is_finite() {
                let style = decimal_style_for_digits(frac_digits, has_comma);
                return Some((v, style));
            }
        }
        return None;
    }

    if normalize_comma_decimals && !s.contains('.') {
        let unsigned = s
            .strip_prefix('-')
            .or_else(|| s.strip_prefix('+'))
            .unwrap_or(s);
        let comma_count = unsigned.bytes().filter(|&b| b == b',').count();
        if comma_count == 1 {
            if let Some((int_part, frac_part)) = unsigned.split_once(',') {
                if !int_part.is_empty()
                    && !frac_part.is_empty()
                    && frac_part.len() <= 6
                    && int_part.chars().all(|c| c.is_ascii_digit())
                    && frac_part.chars().all(|c| c.is_ascii_digit())
                {
                    if int_part.len() > 1 && int_part.starts_with('0') {
                        return None;
                    }
                    let total_digits = int_part.len() + frac_part.len();
                    if total_digits <= 15 {
                        let replaced = s.replacen(',', ".", 1);
                        if let Ok(v) = replaced.parse::<f64>() {
                            if v.is_finite() {
                                let style = decimal_style_for_digits(frac_part.len(), false);
                                return Some((v, style));
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
fn parse_excel_numeric(cell: &str) -> Option<(f64, u8)> {
    parse_excel_numeric_with_opts(cell, false)
}

/// Converts a thin filled rectangle (`w <= 3.0 pt` or `h <= 3.0 pt`) into its centerline `VSegment` / `HSegment`.
pub fn extract_segments_from_thin_rect(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    h_out: &mut Vec<HSegment>,
    v_out: &mut Vec<VSegment>,
) {
    let x0 = left.min(right);
    let x1 = left.max(right);
    let y0 = bottom.min(top);
    let y1 = bottom.max(top);
    let w = x1 - x0;
    let h = y1 - y0;
    if w <= 3.0 && h >= 6.0 {
        v_out.push(VSegment {
            x: (x0 + x1) * 0.5,
            y0,
            y1,
        });
    } else if h <= 3.0 && w >= 6.0 {
        h_out.push(HSegment {
            y: (y0 + y1) * 0.5,
            x0,
            x1,
        });
    }
}

#[inline]
fn compose_pdf_matrix(parent: [f32; 6], child: [f32; 6]) -> [f32; 6] {
    let [pa, pb, pc, pd, pe, pf] = parent;
    let [ca, cb, cc, cd, ce, cf] = child;
    [
        pa * ca + pc * cb,
        pb * ca + pd * cb,
        pa * cc + pc * cd,
        pb * cc + pd * cd,
        pa * ce + pc * cf + pe,
        pb * ce + pd * cf + pf,
    ]
}

#[inline]
fn apply_pdf_matrix(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn push_axis_aligned_edge(
    p0: (f32, f32),
    p1: (f32, f32),
    h_out: &mut Vec<HSegment>,
    v_out: &mut Vec<VSegment>,
) {
    let dx = (p1.0 - p0.0).abs();
    let dy = (p1.1 - p0.1).abs();
    if dy <= 1.2 && dx >= 4.0 {
        h_out.push(HSegment {
            y: (p0.1 + p1.1) * 0.5,
            x0: p0.0.min(p1.0),
            x1: p0.0.max(p1.0),
        });
    } else if dx <= 1.2 && dy >= 4.0 {
        v_out.push(VSegment {
            x: (p0.0 + p1.0) * 0.5,
            y0: p0.1.min(p1.1),
            y1: p0.1.max(p1.1),
        });
    }
}

/// Maps unrotated PDF page points `(x, y)` (bottom-left origin, Y-up) into **visual top-left-origin**
/// `(vx, vy)` coordinates where `vx` increases left-to-right and `vy` increases top-to-bottom as the
/// user sees the page for `/Rotate` values `0`, `90`, `180`, or `270` degrees clockwise.
pub fn to_visual_coords(x: f32, y: f32, page_w: f32, page_h: f32, rotation_deg: u16) -> (f32, f32) {
    match rotation_deg % 360 {
        90 => (y, x),
        180 => (page_w - x, y),
        270 => (page_h - y, page_w - x),
        _ => (x, page_h - y),
    }
}

/// Converts unrotated PDF page points into **visual Y-up** coordinates `(vx, vy_up)` matching
/// the Lattice & Stream engines (larger `vy_up` = higher on screen, `vx` = left-to-right).
#[inline]
fn to_visual_yup(x: f32, y: f32, page_w: f32, page_h: f32, rotation_deg: u16) -> (f32, f32) {
    let (vx, vy_down) = to_visual_coords(x, y, page_w, page_h, rotation_deg);
    let vis_h = if matches!(rotation_deg % 360, 90 | 270) {
        page_w
    } else {
        page_h
    };
    (vx, vis_h - vy_down)
}

/// Queries the page's `/Rotate` value (`0 | 90 | 180 | 270` CW) and unrotated MediaBox `(page_w, page_h)`.
fn page_unrotated_dims_and_rotation(page: &PdfPage) -> (f32, f32, u16) {
    let rot = match page.rotation().unwrap_or(PdfPageRenderRotation::None) {
        PdfPageRenderRotation::None => 0u16,
        PdfPageRenderRotation::Degrees90 => 90u16,
        PdfPageRenderRotation::Degrees180 => 180u16,
        PdfPageRenderRotation::Degrees270 => 270u16,
    };
    if let Ok(media) = page.boundaries().media() {
        let mw = media.bounds.width().value.abs();
        let mh = media.bounds.height().value.abs();
        if mw > 1.0 && mh > 1.0 {
            return (mw, mh, rot);
        }
    }
    let pw = page.width().value.abs();
    let ph = page.height().value.abs();
    if matches!(rot, 90 | 270) {
        (ph, pw, rot)
    } else {
        (pw, ph, rot)
    }
}

/// Extracts all horizontal and vertical vector lines & thin filled rectangles from a PDFium page
/// (recursing into Form XObjects up to depth 4, composing 2D affine matrices, and normalizing for
/// `/Rotate 0/90/180/270` into visual coordinates).
pub fn extract_page_vector_segments(page: &PdfPage) -> (Vec<HSegment>, Vec<VSegment>) {
    let (page_w, page_h, rot) = page_unrotated_dims_and_rotation(page);

    fn visit_obj(
        obj: &PdfPageObject,
        parent_m: [f32; 6],
        page_w: f32,
        page_h: f32,
        rot: u16,
        h_out: &mut Vec<HSegment>,
        v_out: &mut Vec<VSegment>,
        depth: u8,
    ) {
        if let Some(path) = obj.as_path_object() {
            let local_m = path
                .matrix()
                .map(|pm| [pm.a(), pm.b(), pm.c(), pm.d(), pm.e(), pm.f()])
                .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            let m = compose_pdf_matrix(parent_m, local_m);
            let is_stroked = path.is_stroked().unwrap_or(false);
            let is_filled = path
                .fill_mode()
                .map(|fm| fm != PdfPathFillMode::None)
                .unwrap_or(false);
            if !is_stroked && !is_filled {
                return;
            }

            let mut subpaths: Vec<(Vec<(f32, f32)>, bool, bool)> = Vec::new();
            let mut cur_pts: Vec<(f32, f32)> = Vec::new();
            let mut cur_has_bezier = false;
            let mut cur_closed = false;

            for seg in path.segments().iter() {
                let (rx, ry) = seg.point();
                let (px, py) = apply_pdf_matrix(m, rx.value, ry.value);
                let pt = to_visual_yup(px, py, page_w, page_h, rot);
                match seg.segment_type() {
                    PdfPathSegmentType::MoveTo => {
                        if !cur_pts.is_empty() {
                            subpaths.push((std::mem::take(&mut cur_pts), cur_closed, cur_has_bezier));
                        }
                        cur_pts.push(pt);
                        cur_has_bezier = false;
                        cur_closed = seg.is_close();
                    }
                    PdfPathSegmentType::LineTo => {
                        cur_pts.push(pt);
                        if seg.is_close() {
                            cur_closed = true;
                        }
                    }
                    PdfPathSegmentType::BezierTo => {
                        cur_has_bezier = true;
                        cur_pts.push(pt);
                        if seg.is_close() {
                            cur_closed = true;
                        }
                    }
                    PdfPathSegmentType::Unknown => {}
                }
            }
            if !cur_pts.is_empty() {
                subpaths.push((cur_pts, cur_closed, cur_has_bezier));
            }

            for (pts, closed, has_bezier) in subpaths {
                if pts.len() < 2 {
                    continue;
                }
                if is_stroked && !has_bezier {
                    for w in pts.windows(2) {
                        push_axis_aligned_edge(w[0], w[1], h_out, v_out);
                    }
                    if closed && pts.len() >= 3 {
                        push_axis_aligned_edge(*pts.last().unwrap(), pts[0], h_out, v_out);
                    }
                }
                if is_filled && pts.len() >= 3 {
                    let mut min_x = f32::INFINITY;
                    let mut max_x = f32::NEG_INFINITY;
                    let mut min_y = f32::INFINITY;
                    let mut max_y = f32::NEG_INFINITY;
                    for &(px, py) in &pts {
                        min_x = min_x.min(px);
                        max_x = max_x.max(px);
                        min_y = min_y.min(py);
                        max_y = max_y.max(py);
                    }
                    extract_segments_from_thin_rect(min_x, max_x, min_y, max_y, h_out, v_out);
                }
            }
        } else if depth < 4 {
            if let Some(form) = obj.as_x_object_form_object() {
                let local_m = form
                    .matrix()
                    .map(|pm| [pm.a(), pm.b(), pm.c(), pm.d(), pm.e(), pm.f()])
                    .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                let m = compose_pdf_matrix(parent_m, local_m);
                for child in form.iter() {
                    visit_obj(&child, m, page_w, page_h, rot, h_out, v_out, depth + 1);
                }
            }
        }
    }

    let mut h_out = Vec::new();
    let mut v_out = Vec::new();
    let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    for obj in page.objects().iter() {
        visit_obj(&obj, identity, page_w, page_h, rot, &mut h_out, &mut v_out, 0);
    }
    (h_out, v_out)
}

/// Snaps collinear H/V segments within `snap_tol` (default `1.8 pt`) and merges overlapping or
/// touching segments separated by `<= gap_tol` (default `2.0 pt`).
pub fn cluster_and_merge_segments(
    h_raw: &[HSegment],
    v_raw: &[VSegment],
    snap_tol: f32,
    gap_tol: f32,
) -> (Vec<HSegment>, Vec<VSegment>) {
    let mut hs: Vec<HSegment> = h_raw
        .iter()
        .copied()
        .filter(|s| s.x1 - s.x0 >= 3.0)
        .map(|s| HSegment {
            y: s.y,
            x0: s.x0.min(s.x1),
            x1: s.x0.max(s.x1),
        })
        .collect();
    hs.sort_by(|a, b| a.y.total_cmp(&b.y));

    let mut h_merged: Vec<HSegment> = Vec::new();
    let mut i = 0;
    while i < hs.len() {
        let mut cluster = vec![hs[i]];
        let mut y_sum = hs[i].y * (hs[i].x1 - hs[i].x0);
        let mut w_sum = hs[i].x1 - hs[i].x0;
        let mut y_ref = hs[i].y;
        i += 1;
        while i < hs.len() && (hs[i].y - y_ref).abs() <= snap_tol {
            let len = (hs[i].x1 - hs[i].x0).max(1.0);
            y_sum += hs[i].y * len;
            w_sum += len;
            y_ref = y_sum / w_sum;
            cluster.push(hs[i]);
            i += 1;
        }
        let snapped_y = y_sum / w_sum.max(1.0);
        cluster.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        let mut cur_x0 = cluster[0].x0;
        let mut cur_x1 = cluster[0].x1;
        for seg in &cluster[1..] {
            if seg.x0 <= cur_x1 + gap_tol {
                cur_x1 = cur_x1.max(seg.x1);
            } else {
                h_merged.push(HSegment {
                    y: snapped_y,
                    x0: cur_x0,
                    x1: cur_x1,
                });
                cur_x0 = seg.x0;
                cur_x1 = seg.x1;
            }
        }
        h_merged.push(HSegment {
            y: snapped_y,
            x0: cur_x0,
            x1: cur_x1,
        });
    }

    let mut vs: Vec<VSegment> = v_raw
        .iter()
        .copied()
        .filter(|s| s.y1 - s.y0 >= 3.0)
        .map(|s| VSegment {
            x: s.x,
            y0: s.y0.min(s.y1),
            y1: s.y0.max(s.y1),
        })
        .collect();
    vs.sort_by(|a, b| a.x.total_cmp(&b.x));

    let mut v_merged: Vec<VSegment> = Vec::new();
    let mut j = 0;
    while j < vs.len() {
        let mut cluster = vec![vs[j]];
        let mut x_sum = vs[j].x * (vs[j].y1 - vs[j].y0);
        let mut w_sum = vs[j].y1 - vs[j].y0;
        let mut x_ref = vs[j].x;
        j += 1;
        while j < vs.len() && (vs[j].x - x_ref).abs() <= snap_tol {
            let len = (vs[j].y1 - vs[j].y0).max(1.0);
            x_sum += vs[j].x * len;
            w_sum += len;
            x_ref = x_sum / w_sum;
            cluster.push(vs[j]);
            j += 1;
        }
        let snapped_x = x_sum / w_sum.max(1.0);
        cluster.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        let mut cur_y0 = cluster[0].y0;
        let mut cur_y1 = cluster[0].y1;
        for seg in &cluster[1..] {
            if seg.y0 <= cur_y1 + gap_tol {
                cur_y1 = cur_y1.max(seg.y1);
            } else {
                v_merged.push(VSegment {
                    x: snapped_x,
                    y0: cur_y0,
                    y1: cur_y1,
                });
                cur_y0 = seg.y0;
                cur_y1 = seg.y1;
            }
        }
        v_merged.push(VSegment {
            x: snapped_x,
            y0: cur_y0,
            y1: cur_y1,
        });
    }

    (h_merged, v_merged)
}

fn cluster_coords_1d(vals: &[f32], tol: f32) -> Vec<f32> {
    if vals.is_empty() {
        return Vec::new();
    }
    let mut sorted = vals.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mut out = Vec::new();
    let mut sum = sorted[0];
    let mut count = 1usize;
    let mut first = sorted[0];
    for &v in &sorted[1..] {
        if (v - first).abs() <= tol {
            sum += v;
            count += 1;
        } else {
            out.push(sum / (count as f32));
            sum = v;
            count = 1;
            first = v;
        }
    }
    out.push(sum / (count as f32));
    out
}

/// Builds an intersection graph (`tol = 2.0 pt`) between `HSegment`s and `VSegment`s and isolates
/// independent table regions as connected components (`>= 2` distinct H and `>= 2` distinct V lines).
/// Returns components sorted top-to-bottom (and left-to-right for side-by-side tables).
pub fn isolate_table_components(
    h_segs: &[HSegment],
    v_segs: &[VSegment],
) -> Vec<(Vec<HSegment>, Vec<VSegment>, (f32, f32, f32, f32))> {
    let nh = h_segs.len();
    let nv = v_segs.len();
    if nh < 2 || nv < 2 {
        return Vec::new();
    }
    let tol = 2.0f32;
    let mut h_adj: Vec<Vec<usize>> = vec![Vec::new(); nh];
    let mut v_adj: Vec<Vec<usize>> = vec![Vec::new(); nv];

    for (hi, h) in h_segs.iter().enumerate() {
        for (vi, v) in v_segs.iter().enumerate() {
            if v.x >= h.x0 - tol && v.x <= h.x1 + tol && h.y >= v.y0 - tol && h.y <= v.y1 + tol {
                h_adj[hi].push(vi);
                v_adj[vi].push(hi);
            }
        }
    }

    let mut h_vis = vec![false; nh];
    let mut v_vis = vec![false; nv];
    let mut components: Vec<(Vec<HSegment>, Vec<VSegment>, (f32, f32, f32, f32))> = Vec::new();

    for start_h in 0..nh {
        if h_vis[start_h] || h_adj[start_h].is_empty() {
            continue;
        }
        let mut q_h = std::collections::VecDeque::new();
        let mut q_v = std::collections::VecDeque::new();
        let mut comp_h = Vec::new();
        let mut comp_v = Vec::new();
        h_vis[start_h] = true;
        q_h.push_back(start_h);

        while !q_h.is_empty() || !q_v.is_empty() {
            while let Some(hi) = q_h.pop_front() {
                comp_h.push(h_segs[hi]);
                for &vi in &h_adj[hi] {
                    if !v_vis[vi] {
                        v_vis[vi] = true;
                        q_v.push_back(vi);
                    }
                }
            }
            while let Some(vi) = q_v.pop_front() {
                comp_v.push(v_segs[vi]);
                for &hi in &v_adj[vi] {
                    if !h_vis[hi] {
                        h_vis[hi] = true;
                        q_h.push_back(hi);
                    }
                }
            }
        }

        let ys = cluster_coords_1d(&comp_h.iter().map(|s| s.y).collect::<Vec<_>>(), 1.8);
        let xs = cluster_coords_1d(&comp_v.iter().map(|s| s.x).collect::<Vec<_>>(), 1.8);
        if ys.len() < 2 || xs.len() < 2 {
            continue;
        }
        let min_x = *xs.first().unwrap();
        let max_x = *xs.last().unwrap();
        let min_y = *ys.first().unwrap();
        let max_y = *ys.last().unwrap();
        if (max_x - min_x) < 18.0 || (max_y - min_y) < 10.0 {
            continue;
        }
        components.push((comp_h, comp_v, (min_x, max_x, min_y, max_y)));
    }

    components.sort_by(|a, b| {
        let (ax0, _, ay0, ay1) = a.2;
        let (bx0, _, by0, by1) = b.2;
        let y_overlap = (ay1.min(by1) - ay0.max(by0)).max(0.0);
        let min_h = (ay1 - ay0).min(by1 - by0).max(1.0);
        if y_overlap >= 0.4 * min_h {
            ax0.total_cmp(&bx0)
        } else {
            by1.total_cmp(&ay1)
        }
    });

    components
}

fn format_cell_words(mut cell_words: Vec<&WordBox>) -> String {
    if cell_words.is_empty() {
        return String::new();
    }
    let avg_h = (cell_words.iter().map(|w| w.height()).sum::<f32>() / cell_words.len() as f32).max(6.0);
    cell_words.sort_by(|a, b| b.cy().total_cmp(&a.cy()).then_with(|| a.left.total_cmp(&b.left)));

    let mut lines: Vec<Vec<&WordBox>> = Vec::new();
    for w in cell_words {
        if let Some(last_line) = lines.last_mut() {
            let ref_cy = last_line[0].cy();
            if (ref_cy - w.cy()).abs() <= avg_h * 0.55 {
                last_line.push(w);
                continue;
            }
        }
        lines.push(vec![w]);
    }

    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len());
    for mut line in lines {
        line.sort_by(|a, b| a.left.total_cmp(&b.left));
        let joined = line
            .iter()
            .map(|w| w.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !joined.is_empty() {
            out_lines.push(joined);
        }
    }
    out_lines.join("\n")
}

/// Builds a Lattice `ExcelTableGrid` with true rectangular `<mergeCells>` spans and exact
/// bounding-box word assignment (never shifting empty cells left). Also returns a boolean mask
/// indicating which `words` were consumed by this table.
pub fn build_lattice_grid_from_component(
    comp_h: &[HSegment],
    comp_v: &[VSegment],
    words: &[WordBox],
    page_num: usize,
) -> Option<(ExcelTableGrid, Vec<bool>)> {
    let mut y_lines = cluster_coords_1d(&comp_h.iter().map(|s| s.y).collect::<Vec<_>>(), 1.8);
    let x_lines = cluster_coords_1d(&comp_v.iter().map(|s| s.x).collect::<Vec<_>>(), 1.8);
    if y_lines.len() < 2 || x_lines.len() < 2 {
        return None;
    }
    y_lines.sort_by(|a, b| b.total_cmp(a));
    let nrows = y_lines.len() - 1;
    let ncols = x_lines.len() - 1;
    let snap = 2.2f32;

    let has_h_wall = |row_b: usize, col: usize| -> bool {
        let y = y_lines[row_b];
        let x0 = x_lines[col];
        let x1 = x_lines[col + 1];
        let span = (x1 - x0).max(1.0);
        let mut covered = 0.0f32;
        for h in comp_h {
            if (h.y - y).abs() <= snap {
                let ov = (h.x1.min(x1) - h.x0.max(x0)).max(0.0);
                covered += ov;
            }
        }
        covered >= span * 0.55
    };

    let has_v_wall = |row: usize, col_b: usize| -> bool {
        let x = x_lines[col_b];
        let y_top = y_lines[row];
        let y_bot = y_lines[row + 1];
        let span = (y_top - y_bot).max(1.0);
        let mut covered = 0.0f32;
        for v in comp_v {
            if (v.x - x).abs() <= snap {
                let ov = (v.y1.min(y_top) - v.y0.max(y_bot)).max(0.0);
                covered += ov;
            }
        }
        covered >= span * 0.55
    };

    let mut owner: Vec<Vec<Option<(usize, usize)>>> = vec![vec![None; ncols]; nrows];
    let mut merges: Vec<CellMerge> = Vec::new();
    for r in 0..nrows {
        for c in 0..ncols {
            if owner[r][c].is_some() {
                continue;
            }
            let mut c2 = c;
            while c2 + 1 < ncols && owner[r][c2 + 1].is_none() && !has_v_wall(r, c2 + 1) {
                c2 += 1;
            }
            let mut r2 = r;
            'down: while r2 + 1 < nrows {
                for cc in c..=c2 {
                    if owner[r2 + 1][cc].is_some() || has_h_wall(r2 + 1, cc) {
                        break 'down;
                    }
                }
                for cc in c..c2 {
                    if has_v_wall(r2 + 1, cc + 1) {
                        break 'down;
                    }
                }
                r2 += 1;
            }
            for rr in r..=r2 {
                for cc in c..=c2 {
                    owner[rr][cc] = Some((r, c));
                }
            }
            if r2 > r || c2 > c {
                merges.push(CellMerge {
                    row: r,
                    col: c,
                    row_span: r2 - r + 1,
                    col_span: c2 - c + 1,
                });
            }
        }
    }

    let min_x = x_lines[0];
    let max_x = x_lines[ncols];
    let max_y = y_lines[0];
    let min_y = y_lines[nrows];
    let outer_tol = 2.0f32;

    let mut buckets: Vec<Vec<Vec<&WordBox>>> = vec![vec![Vec::new(); ncols]; nrows];
    let mut consumed = vec![false; words.len()];

    for (wi, w) in words.iter().enumerate() {
        let cx = w.cx();
        let cy = w.cy();
        if cx < min_x - outer_tol
            || cx > max_x + outer_tol
            || cy < min_y - outer_tol
            || cy > max_y + outer_tol
        {
            continue;
        }
        let mut r_hit = None;
        for r in 0..nrows {
            if cy <= y_lines[r] + outer_tol && cy >= y_lines[r + 1] - outer_tol {
                r_hit = Some(r);
                break;
            }
        }
        let mut c_hit = None;
        for c in 0..ncols {
            if cx >= x_lines[c] - outer_tol && cx <= x_lines[c + 1] + outer_tol {
                c_hit = Some(c);
                break;
            }
        }
        if let (Some(r), Some(c)) = (r_hit, c_hit) {
            let (ar, ac) = owner[r][c].unwrap_or((r, c));
            buckets[ar][ac].push(w);
            consumed[wi] = true;
        }
    }

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(nrows);
    let mut non_empty_cells = 0usize;
    for r in 0..nrows {
        let mut row_cells = Vec::with_capacity(ncols);
        for c in 0..ncols {
            let txt = format_cell_words(std::mem::take(&mut buckets[r][c]));
            if !txt.is_empty() {
                non_empty_cells += 1;
            }
            row_cells.push(txt);
        }
        rows.push(row_cells);
    }

    if non_empty_cells == 0 {
        return None;
    }

    Some((
        ExcelTableGrid {
            rows,
            merges,
            page_num,
            is_lattice: true,
            bbox: (min_x, max_x, min_y, max_y),
        },
        consumed,
    ))
}

/// Extracts word-level bounding boxes from a PDFium page via `FPDFText_GetCharBox` (`page.text().chars()`),
/// normalizing for `/Rotate 0/90/180/270` into visual coordinates before word grouping.
pub fn extract_page_word_boxes(page: &PdfPage) -> Vec<WordBox> {
    let Ok(tp) = page.text() else {
        return Vec::new();
    };
    let (page_w, page_h, rot) = page_unrotated_dims_and_rotation(page);
    let mut words: Vec<WordBox> = Vec::new();
    let mut cur_text = String::new();
    let mut cur_l = 0.0f32;
    let mut cur_r = 0.0f32;
    let mut cur_b = 0.0f32;
    let mut cur_t = 0.0f32;

    let flush = |cur_text: &mut String,
                 cur_l: f32,
                 cur_r: f32,
                 cur_b: f32,
                 cur_t: f32,
                 out: &mut Vec<WordBox>| {
        let trimmed = cur_text.trim();
        if !trimmed.is_empty() {
            out.push(WordBox {
                text: trimmed.to_string(),
                left: cur_l,
                right: cur_r,
                bottom: cur_b,
                top: cur_t,
            });
        }
        cur_text.clear();
    };

    for ch in tp.chars().iter() {
        let Some(c) = ch.unicode_char() else {
            continue;
        };
        if c.is_whitespace() || c.is_control() {
            if !cur_text.is_empty() {
                flush(&mut cur_text, cur_l, cur_r, cur_b, cur_t, &mut words);
            }
            continue;
        }
        let Ok(rect) = ch.tight_bounds().or_else(|_| ch.loose_bounds()) else {
            continue;
        };
        let raw_l = rect.left().value.min(rect.right().value);
        let raw_r = rect.left().value.max(rect.right().value);
        let raw_b = rect.bottom().value.min(rect.top().value);
        let raw_t = rect.bottom().value.max(rect.top().value);
        let (x0, y0) = to_visual_yup(raw_l, raw_b, page_w, page_h, rot);
        let (x1, y1) = to_visual_yup(raw_r, raw_t, page_w, page_h, rot);
        let l = x0.min(x1);
        let r = x0.max(x1);
        let b = y0.min(y1);
        let t = y0.max(y1);
        let h = (t - b).max(4.0);

        if cur_text.is_empty() {
            cur_text.push(c);
            cur_l = l;
            cur_r = r;
            cur_b = b;
            cur_t = t;
        } else {
            let cur_h = (cur_t - cur_b).max(h).max(4.0);
            let same_line = ((cur_b + cur_t) * 0.5 - (b + t) * 0.5).abs() <= cur_h * 0.65;
            let gap = l - cur_r;
            let max_char_gap = (cur_h * 0.30).max(1.8);
            if same_line && gap >= -cur_h * 0.5 && gap <= max_char_gap {
                cur_text.push(c);
                cur_l = cur_l.min(l);
                cur_r = cur_r.max(r);
                cur_b = cur_b.min(b);
                cur_t = cur_t.max(t);
            } else {
                flush(&mut cur_text, cur_l, cur_r, cur_b, cur_t, &mut words);
                cur_text.push(c);
                cur_l = l;
                cur_r = r;
                cur_b = b;
                cur_t = t;
            }
        }
    }
    if !cur_text.is_empty() {
        flush(&mut cur_text, cur_l, cur_r, cur_b, cur_t, &mut words);
    }
    words
}

/// Stream Fallback Engine: groups words into visual rows via Y-overlap, detects vertical whitespace
/// gutters via an X-axis coverage histogram, and aligns columns without shifting empty cells left.
pub fn extract_stream_tables_from_words(words: &[WordBox], page_num: usize) -> Vec<ExcelTableGrid> {
    if words.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<&WordBox> = words.iter().collect();
    sorted.sort_by(|a, b| b.cy().total_cmp(&a.cy()).then_with(|| a.left.total_cmp(&b.left)));

    let mut visual_rows: Vec<(f32, f32, Vec<&WordBox>)> = Vec::new();
    for w in sorted {
        let mut placed = false;
        for (r_bot, r_top, r_words) in visual_rows.iter_mut().rev().take(4) {
            let ov = r_top.min(w.top) - r_bot.max(w.bottom);
            let min_h = (*r_top - *r_bot).min(w.height()).max(1.0);
            if ov >= 0.30 * min_h || ((*r_bot + *r_top) * 0.5 - w.cy()).abs() <= min_h * 0.45 {
                *r_bot = r_bot.min(w.bottom);
                *r_top = r_top.max(w.top);
                r_words.push(w);
                placed = true;
                break;
            }
        }
        if !placed {
            visual_rows.push((w.bottom, w.top, vec![w]));
        }
    }
    visual_rows.sort_by(|a, b| ((b.0 + b.1) * 0.5).total_cmp(&((a.0 + a.1) * 0.5)));
    for (_, _, r_words) in &mut visual_rows {
        r_words.sort_by(|a, b| a.left.total_cmp(&b.left));
    }

    let min_x = words.iter().map(|w| w.left).fold(f32::INFINITY, f32::min);
    let max_x = words.iter().map(|w| w.right).fold(f32::NEG_INFINITY, f32::max);
    let min_y = words.iter().map(|w| w.bottom).fold(f32::INFINITY, f32::min);
    let max_y = words.iter().map(|w| w.top).fold(f32::NEG_INFINITY, f32::max);
    let span_x = (max_x - min_x).ceil().max(1.0) as usize;
    let avg_h = (words.iter().map(|w| w.height()).sum::<f32>() / words.len() as f32).max(6.0);
    let min_gutter_pts = (avg_h * 0.65).clamp(5.0, 18.0);

    let mut coverage = vec![0u32; span_x + 2];
    for (_, _, r_words) in &visual_rows {
        let mut p_left = r_words[0].left;
        let mut p_right = r_words[0].right;
        for w in &r_words[1..] {
            if w.left - p_right <= min_gutter_pts * 0.85 {
                p_right = p_right.max(w.right);
            } else {
                let b0 = ((p_left - min_x).floor().max(0.0) as usize).min(span_x);
                let b1 = ((p_right - min_x).ceil().max(0.0) as usize).min(span_x);
                for slot in &mut coverage[b0..=b1] {
                    *slot += 1;
                }
                p_left = w.left;
                p_right = w.right;
            }
        }
        let b0 = ((p_left - min_x).floor().max(0.0) as usize).min(span_x);
        let b1 = ((p_right - min_x).ceil().max(0.0) as usize).min(span_x);
        for slot in &mut coverage[b0..=b1] {
            *slot += 1;
        }
    }

    let mut splits: Vec<f32> = Vec::new();
    let mut bin = 0usize;
    let min_gutter_bins = min_gutter_pts.floor().max(4.0) as usize;
    while bin <= span_x {
        if coverage[bin] == 0 {
            let start = bin;
            while bin <= span_x && coverage[bin] == 0 {
                bin += 1;
            }
            let end = bin;
            if start > 0 && end <= span_x && (end - start) >= min_gutter_bins {
                splits.push(min_x + ((start + end) as f32) * 0.5);
            }
        } else {
            bin += 1;
        }
    }

    let ncols = splits.len() + 1;
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(visual_rows.len());
    for (_, _, r_words) in &visual_rows {
        let mut buckets: Vec<Vec<&WordBox>> = vec![Vec::new(); ncols];
        for &w in r_words {
            let cx = w.cx();
            let mut col = 0usize;
            for &sx in &splits {
                if cx > sx {
                    col += 1;
                } else {
                    break;
                }
            }
            buckets[col.min(ncols - 1)].push(w);
        }
        let row_cells: Vec<String> = buckets
            .into_iter()
            .map(|mut bw| {
                bw.sort_by(|a, b| a.left.total_cmp(&b.left));
                bw.iter()
                    .map(|w| w.text.trim())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        if row_cells.iter().any(|c| !c.is_empty()) {
            rows.push(row_cells);
        }
    }

    if rows.is_empty() {
        return Vec::new();
    }

    vec![ExcelTableGrid {
        rows,
        merges: Vec::new(),
        page_num,
        is_lattice: false,
        bbox: (min_x, max_x, min_y, max_y),
    }]
}

/// Pure-Rust orchestrator for a single page: runs Lattice, Stream, or AutoHybrid extraction.
pub fn extract_page_excel_tables_hybrid(
    h_raw: &[HSegment],
    v_raw: &[VSegment],
    words: &[WordBox],
    page_num: usize,
    mode: ExcelExportMode,
) -> Vec<ExcelTableGrid> {
    let mut tables: Vec<ExcelTableGrid> = Vec::new();
    let mut consumed = vec![false; words.len()];

    if matches!(mode, ExcelExportMode::AutoHybrid | ExcelExportMode::Lattice) {
        let (h_segs, v_segs) = cluster_and_merge_segments(h_raw, v_raw, 1.8, 2.0);
        let components = isolate_table_components(&h_segs, &v_segs);
        for (comp_h, comp_v, _) in components {
            if let Some((grid, mask)) =
                build_lattice_grid_from_component(&comp_h, &comp_v, words, page_num)
            {
                for (i, c) in mask.into_iter().enumerate() {
                    if c {
                        consumed[i] = true;
                    }
                }
                tables.push(grid);
            }
        }
    }

    if mode == ExcelExportMode::Stream || (mode == ExcelExportMode::AutoHybrid && tables.is_empty()) {
        let remaining: Vec<WordBox> = words
            .iter()
            .enumerate()
            .filter(|(i, _)| mode == ExcelExportMode::Stream || !consumed[*i])
            .map(|(_, w)| w.clone())
            .collect();
        tables.extend(extract_stream_tables_from_words(&remaining, page_num));
    } else if mode == ExcelExportMode::AutoHybrid {
        let remaining: Vec<WordBox> = words
            .iter()
            .enumerate()
            .filter(|(i, _)| !consumed[*i])
            .map(|(_, w)| w.clone())
            .collect();
        if remaining.len() >= 6 {
            let stream_extra = extract_stream_tables_from_words(&remaining, page_num);
            for st in stream_extra {
                let cols = st.rows.iter().map(|r| r.len()).max().unwrap_or(0);
                if cols >= 2 && st.rows.len() >= 2 {
                    tables.push(st);
                }
            }
        }
    }

    tables
}

/// Converts an open `PdfDocument` to `.xlsx` using the Milestone 4 Hybrid Lattice + Stream engine.
pub fn export_document_to_xlsx_with_options(
    doc: &PdfDocument,
    opts: ExcelExportOptions,
) -> Vec<u8> {
    let mut all_tables: Vec<ExcelTableGrid> = Vec::new();
    for (p_idx, page) in doc.pages().iter().enumerate() {
        let words = extract_page_word_boxes(&page);
        let (h_raw, v_raw) = if opts.mode == ExcelExportMode::Stream {
            (Vec::new(), Vec::new())
        } else {
            extract_page_vector_segments(&page)
        };
        let page_tables =
            extract_page_excel_tables_hybrid(&h_raw, &v_raw, &words, p_idx + 1, opts.mode);
        all_tables.extend(page_tables);
    }

    if all_tables.is_empty() && opts.mode != ExcelExportMode::Lattice {
        let pages = extract_document_blocks(doc);
        return render_blocks_to_xlsx(&pages);
    }

    render_tables_to_xlsx(&all_tables, opts)
}

fn finish_xlsx_workbook(
    sheets: &[(String, Vec<Vec<String>>, Vec<CellMerge>, Vec<usize>)],
    normalize_comma_decimals: bool,
) -> Vec<u8> {
    use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook};
    use std::collections::HashSet;

    let border_col = Color::RGB(0xCBD5E1);
    let make_base = || {
        Format::new()
            .set_font_name("Calibri")
            .set_font_size(11.0)
            .set_border(FormatBorder::Thin)
            .set_border_color(border_col)
    };

    let fmt_header = make_base()
        .set_bold()
        .set_font_color(Color::RGB(0x0F172A))
        .set_background_color(Color::RGB(0xEFF6FF))
        .set_text_wrap()
        .set_align(FormatAlign::Top);
    let fmt_text = make_base()
        .set_text_wrap()
        .set_align(FormatAlign::Top);
    let fmt_num_2 = make_base().set_num_format("#,##0.00");
    let fmt_pct = make_base().set_num_format("0.00%");
    let fmt_int = make_base();
    let fmt_num_1 = make_base().set_num_format("#,##0.0");
    let fmt_num_3 = make_base().set_num_format("#,##0.000");
    let fmt_num_4 = make_base().set_num_format("#,##0.0000");
    let fmt_num_5 = make_base().set_num_format("#,##0.00000");
    let fmt_num_6 = make_base().set_num_format("#,##0.000000");

    let num_format_for = |style_id: u8| -> &Format {
        match style_id {
            3 => &fmt_num_2,
            4 => &fmt_pct,
            5 => &fmt_int,
            6 => &fmt_num_1,
            7 => &fmt_num_3,
            8 => &fmt_num_4,
            9 => &fmt_num_5,
            10 => &fmt_num_6,
            _ => &fmt_int,
        }
    };

    let mut workbook = Workbook::new();
    let mut used_names: HashSet<String> = HashSet::new();

    for (idx, (raw_name, rows, merges, header_rows)) in sheets.iter().enumerate() {
        let worksheet = workbook.add_worksheet();

        // Sanitize sheet name (max 31 chars, no : \ / ? * [ ], unique case-insensitively)
        let cleaned: String = raw_name
            .chars()
            .filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
            .take(31)
            .collect();
        let base_name = if cleaned.trim().is_empty() {
            format!("Sheet{}", idx + 1)
        } else {
            cleaned.trim().to_string()
        };
        let mut safe_name = base_name.clone();
        let mut suffix = 2usize;
        while !used_names.insert(safe_name.to_ascii_lowercase()) {
            let tag = format!(" ({suffix})");
            let max_base = 31usize.saturating_sub(tag.len());
            safe_name = format!("{}{}", base_name.chars().take(max_base).collect::<String>(), tag);
            suffix += 1;
        }
        let _ = worksheet.set_name(&safe_name);

        let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
        let mut col_widths = vec![10usize; max_cols];
        for row in rows {
            for (c, cell) in row.iter().enumerate() {
                let max_line = cell
                    .lines()
                    .map(|l| l.chars().count())
                    .max()
                    .unwrap_or(0);
                if max_line + 3 > col_widths[c] {
                    col_widths[c] = (max_line + 3).min(52);
                }
            }
        }
        for (c, &w) in col_widths.iter().enumerate() {
            let _ = worksheet.set_column_width(c as u16, w as f64);
        }

        // Collect all cells covered by merges so regular cell writing skips them
        let mut merged_cells: HashSet<(usize, usize)> = HashSet::new();
        for m in merges {
            if m.row_span == 0 || m.col_span == 0 {
                continue;
            }
            for rr in m.row..(m.row + m.row_span) {
                for cc in m.col..(m.col + m.col_span) {
                    merged_cells.insert((rr, cc));
                }
            }
        }

        // Write non-merged cells
        for (r_idx, row) in rows.iter().enumerate() {
            let is_hdr = header_rows.contains(&r_idx);
            let r_u32 = r_idx as u32;
            for (c_idx, val) in row.iter().enumerate() {
                if merged_cells.contains(&(r_idx, c_idx)) {
                    continue;
                }
                let c_u16 = c_idx as u16;
                if !is_hdr {
                    if let Some((num, num_style)) =
                        parse_excel_numeric_with_opts(val, normalize_comma_decimals)
                    {
                        let _ = worksheet.write_number_with_format(
                            r_u32,
                            c_u16,
                            num,
                            num_format_for(num_style),
                        );
                        continue;
                    }
                }
                let fmt = if is_hdr { &fmt_header } else { &fmt_text };
                let _ = worksheet.write_string_with_format(r_u32, c_u16, val, fmt);
            }
        }

        // Apply merged ranges cleanly via rust_xlsxwriter::Worksheet::merge_range
        for m in merges {
            if m.row_span == 0 || m.col_span == 0 {
                continue;
            }
            let r1 = m.row as u32;
            let c1 = m.col as u16;
            let r2 = (m.row + m.row_span - 1) as u32;
            let c2 = (m.col + m.col_span - 1) as u16;
            let is_hdr = header_rows.contains(&m.row);
            let val = rows
                .get(m.row)
                .and_then(|r| r.get(m.col))
                .map(|s| s.as_str())
                .unwrap_or("");
            if !is_hdr {
                if let Some((num, num_style)) =
                    parse_excel_numeric_with_opts(val, normalize_comma_decimals)
                {
                    let num_fmt = num_format_for(num_style);
                    let _ = worksheet.merge_range(r1, c1, r2, c2, "", num_fmt);
                    let _ = worksheet.write_number_with_format(r1, c1, num, num_fmt);
                    continue;
                }
            }
            let fmt = if is_hdr { &fmt_header } else { &fmt_text };
            let _ = worksheet.merge_range(r1, c1, r2, c2, val, fmt);
        }
    }

    workbook.save_to_buffer().unwrap_or_default()
}

/// Renders extracted `ExcelTableGrid`s to a valid OOXML `.xlsx` ZIP byte vector with `<mergeCells>`,
/// financial decimal formats, `wrapText="1"`, auto-fit column widths, and chosen sheet layout.
pub fn render_tables_to_xlsx(tables: &[ExcelTableGrid], opts: ExcelExportOptions) -> Vec<u8> {
    if tables.is_empty() {
        let sheets = vec![(
            "Sheet1".to_string(),
            vec![vec!["(No extractable tables or text found in PDF)".to_string()]],
            Vec::new(),
            Vec::new(),
        )];
        return finish_xlsx_workbook(&sheets, opts.normalize_comma_decimals);
    }

    let mut sheets: Vec<(String, Vec<Vec<String>>, Vec<CellMerge>, Vec<usize>)> = Vec::new();
    match opts.layout {
        ExcelTableLayout::StackOnOneSheet => {
            let mut stacked_rows: Vec<Vec<String>> = Vec::new();
            let mut stacked_merges: Vec<CellMerge> = Vec::new();
            let mut header_rows: Vec<usize> = Vec::new();
            for (t_idx, tbl) in tables.iter().enumerate() {
                if tbl.rows.is_empty() {
                    continue;
                }
                if t_idx > 0 {
                    stacked_rows.push(Vec::new());
                    stacked_rows.push(Vec::new());
                }
                let row_offset = stacked_rows.len();
                header_rows.push(row_offset);
                for m in &tbl.merges {
                    stacked_merges.push(CellMerge {
                        row: m.row + row_offset,
                        col: m.col,
                        row_span: m.row_span,
                        col_span: m.col_span,
                    });
                }
                stacked_rows.extend(tbl.rows.iter().cloned());
            }
            sheets.push((
                "Extracted Tables".to_string(),
                stacked_rows,
                stacked_merges,
                header_rows,
            ));
        }
        ExcelTableLayout::SeparateSheets => {
            for (t_idx, tbl) in tables.iter().enumerate() {
                if tbl.rows.is_empty() {
                    continue;
                }
                let table_num = t_idx + 1;
                if sheets.len() < 30 {
                    let name = format!("Table {table_num} (p.{})", tbl.page_num);
                    sheets.push((name, tbl.rows.clone(), tbl.merges.clone(), vec![0]));
                } else if let Some((_, last_rows, last_merges, last_hdrs)) = sheets.last_mut() {
                    last_rows.push(Vec::new());
                    last_rows.push(Vec::new());
                    let off = last_rows.len();
                    last_hdrs.push(off);
                    for m in &tbl.merges {
                        last_merges.push(CellMerge {
                            row: m.row + off,
                            col: m.col,
                            row_span: m.row_span,
                            col_span: m.col_span,
                        });
                    }
                    last_rows.extend(tbl.rows.iter().cloned());
                }
            }
        }
    }

    finish_xlsx_workbook(&sheets, opts.normalize_comma_decimals)
}

pub fn render_blocks_to_xlsx(pages: &[Vec<ExportBlock>]) -> Vec<u8> {
    let mut sheets: Vec<(String, Vec<Vec<String>>, Vec<CellMerge>, Vec<usize>)> = Vec::new();
    let mut text_rows: Vec<Vec<String>> = Vec::new();
    let mut table_num = 0usize;

    for (p_idx, blocks) in pages.iter().enumerate() {
        let page_lbl = format!("Page {}", p_idx + 1);
        for block in blocks {
            match block {
                ExportBlock::Table(rows) if !rows.is_empty() => {
                    table_num += 1;
                    if sheets.len() < 28 {
                        let name = format!("Table {table_num} (p.{})", p_idx + 1);
                        sheets.push((name, rows.clone(), Vec::new(), vec![0]));
                    } else if let Some((_, last_rows, _, _)) = sheets.last_mut() {
                        last_rows.push(Vec::new());
                        last_rows.push(vec![format!("Table {table_num} ({page_lbl})")]);
                        last_rows.extend(rows.iter().cloned());
                    }
                }
                ExportBlock::Heading { text, .. } => {
                    text_rows.push(vec![page_lbl.clone(), "Heading".to_string(), text.clone()]);
                }
                ExportBlock::Paragraph(spans) => {
                    let s: String = spans.iter().map(|sp| sp.text.as_str()).collect();
                    if !s.trim().is_empty() {
                        text_rows.push(vec![page_lbl.clone(), "Text".to_string(), s.trim().to_string()]);
                    }
                }
                ExportBlock::BulletItem(spans) => {
                    let s: String = spans.iter().map(|sp| sp.text.as_str()).collect();
                    if !s.trim().is_empty() {
                        text_rows.push(vec![page_lbl.clone(), "Bullet".to_string(), s.trim().to_string()]);
                    }
                }
                ExportBlock::NumberedItem { num, spans } => {
                    let s: String = spans.iter().map(|sp| sp.text.as_str()).collect();
                    if !s.trim().is_empty() {
                        text_rows.push(vec![
                            page_lbl.clone(),
                            format!("Item {num}"),
                            s.trim().to_string(),
                        ]);
                    }
                }
                ExportBlock::Table(_) => {}
            }
        }
    }

    if !text_rows.is_empty() {
        let mut with_hdr = Vec::with_capacity(text_rows.len() + 1);
        with_hdr.push(vec!["Page".to_string(), "Type".to_string(), "Content".to_string()]);
        with_hdr.extend(text_rows);
        sheets.push(("Document Text".to_string(), with_hdr, Vec::new(), vec![0]));
    }

    if sheets.is_empty() {
        sheets.push((
            "Sheet1".to_string(),
            vec![vec!["(No extractable text or tables found in PDF)".to_string()]],
            Vec::new(),
            Vec::new(),
        ));
    }

    finish_xlsx_workbook(&sheets, false)
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

    fn mk_run(text: &str, left: f32, right: f32, y: f32, bold: bool) -> TextRun {
        TextRun {
            text: text.to_string(),
            left,
            right,
            bottom: y,
            top: y + 10.0,
            font_size: 10.0,
            bold,
            italic: false,
            link: None,
        }
    }

    #[test]
    fn test_excel_table_not_split_into_text_and_number_flows() {
        // Simulates a 595pt-wide page exported from Excel: text labels on the left half,
        // numeric columns on the right half, including a section row and a blank cell.
        let runs = vec![
            // Header row
            mk_run("Line Item", 50.0, 120.0, 720.0, true),
            mk_run("Q1", 345.0, 365.0, 720.0, true),
            mk_run("Q2", 425.0, 445.0, 720.0, true),
            mk_run("Total", 495.0, 530.0, 720.0, true),
            // Row 1
            mk_run("Product Revenue", 50.0, 155.0, 702.0, false),
            mk_run("$12,450", 322.0, 365.0, 702.0, false),
            mk_run("$14,800", 402.0, 445.0, 702.0, false),
            mk_run("$27,250", 485.0, 530.0, 702.0, false),
            // Row 2: blank Q1 cell
            mk_run("Consulting Fees", 50.0, 150.0, 684.0, false),
            mk_run("$3,200", 408.0, 445.0, 684.0, false),
            mk_run("$3,200", 491.0, 530.0, 684.0, false),
            // Row 3: single-cell subheader
            mk_run("Operating Costs", 50.0, 150.0, 666.0, true),
            // Row 4
            mk_run("Cloud Hosting", 50.0, 140.0, 648.0, false),
            mk_run("$1,100", 328.0, 365.0, 648.0, false),
            mk_run("$1,250", 408.0, 445.0, 648.0, false),
            mk_run("$2,350", 491.0, 530.0, 648.0, false),
            // Row 5
            mk_run("Net Income", 50.0, 125.0, 630.0, true),
            mk_run("$11,350", 322.0, 365.0, 630.0, true),
            mk_run("$16,750", 402.0, 445.0, 630.0, true),
            mk_run("$28,100", 485.0, 530.0, 630.0, true),
            // Row 6
            mk_run("Margin", 50.0, 95.0, 612.0, false),
            mk_run("91.2%", 332.0, 365.0, 612.0, false),
            mk_run("93.1%", 412.0, 445.0, 612.0, false),
            mk_run("92.3%", 495.0, 530.0, 612.0, false),
            // Row 7
            mk_run("Headcount", 50.0, 115.0, 594.0, false),
            mk_run("14", 352.0, 365.0, 594.0, false),
            mk_run("16", 432.0, 445.0, 594.0, false),
            mk_run("16", 517.0, 530.0, 594.0, false),
            // Row 8
            mk_run("Notes", 50.0, 90.0, 576.0, false),
            mk_run("OK", 348.0, 365.0, 576.0, false),
            mk_run("OK", 428.0, 445.0, 576.0, false),
            mk_run("Audited", 492.0, 530.0, 576.0, false),
        ];

        let lines = group_runs_into_lines(595.0, runs);
        // Must stay 9 horizontal rows (NOT split into 9 left rows + 8 right rows!)
        assert_eq!(lines.len(), 9);
        assert!(lines[0].plain_text().contains("Line Item"));
        assert!(lines[0].plain_text().contains("Total"));

        let blocks = classify_lines_into_blocks(&lines, 10.0);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ExportBlock::Table(rows) => {
                assert_eq!(rows.len(), 9);
                assert_eq!(rows[0], vec!["Line Item", "Q1", "Q2", "Total"]);
                assert_eq!(rows[1], vec!["Product Revenue", "$12,450", "$14,800", "$27,250"]);
                // Blank Q1 cell preserved in Consulting Fees row!
                assert_eq!(rows[2], vec!["Consulting Fees", "", "$3,200", "$3,200"]);
                // Single-cell subheader preserved!
                assert_eq!(rows[3], vec!["Operating Costs", "", "", ""]);
                assert_eq!(rows[5], vec!["Net Income", "$11,350", "$16,750", "$28,100"]);
            }
            other => panic!("Expected ExportBlock::Table, got {other:?}"),
        }
    }

    #[test]
    fn test_true_two_column_academic_prose_still_reads_left_then_right() {
        let mut runs = Vec::new();
        for idx in 0..9 {
            let y = 700.0 - idx as f32 * 14.0;
            runs.push(mk_run(
                &format!("Left column prose line {idx} with enough words to fill a column."),
                40.0,
                275.0,
                y,
                false,
            ));
            runs.push(mk_run(
                &format!("Right column prose line {idx} continuing the academic paper text."),
                315.0,
                555.0,
                y,
                false,
            ));
        }
        let lines = group_runs_into_lines(595.0, runs);
        assert_eq!(lines.len(), 18);
        assert!(lines[0].plain_text().starts_with("Left column prose line 0"));
        assert!(lines[8].plain_text().starts_with("Left column prose line 8"));
        assert!(lines[9].plain_text().starts_with("Right column prose line 0"));
    }

    fn read_zip_entries(zip: &[u8]) -> Vec<(String, String)> {
        use flate2::read::DeflateDecoder;
        use std::io::Read;

        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + 30 <= zip.len() {
            let sig = u32::from_le_bytes(zip[pos..pos + 4].try_into().unwrap());
            if sig != 0x0403_4b50 {
                break;
            }
            let method = u16::from_le_bytes(zip[pos + 8..pos + 10].try_into().unwrap());
            let crc = u32::from_le_bytes(zip[pos + 14..pos + 18].try_into().unwrap());
            let comp_len = u32::from_le_bytes(zip[pos + 18..pos + 22].try_into().unwrap()) as usize;
            let uncomp_len = u32::from_le_bytes(zip[pos + 22..pos + 26].try_into().unwrap()) as usize;
            let name_len = u16::from_le_bytes(zip[pos + 26..pos + 28].try_into().unwrap()) as usize;
            let extra_len = u16::from_le_bytes(zip[pos + 28..pos + 30].try_into().unwrap()) as usize;
            let name_start = pos + 30;
            let data_start = name_start + name_len + extra_len;
            let name = String::from_utf8(zip[name_start..name_start + name_len].to_vec()).unwrap();
            let raw_payload = &zip[data_start..data_start + comp_len];
            let data = if method == 8 {
                let mut dec = DeflateDecoder::new(raw_payload);
                let mut v = Vec::new();
                dec.read_to_end(&mut v).unwrap();
                v
            } else {
                raw_payload.to_vec()
            };
            assert_eq!(data.len(), uncomp_len, "uncompressed len mismatch for {name}");
            assert_eq!(crc32_ieee(&data), crc, "CRC32 mismatch for {name}");
            out.push((name, String::from_utf8(data).unwrap()));
            pos = data_start + comp_len;
        }
        out
    }

    #[test]
    fn test_docx_and_xlsx_export_produce_valid_ooxml_zips() {
        let pages = vec![vec![
            ExportBlock::Heading {
                level: 1,
                text: "Q4 Financial Summary".to_string(),
            },
            ExportBlock::Paragraph(vec![
                StyledSpan { text: "Visit ".to_string(), bold: false, italic: false, link: None },
                StyledSpan {
                    text: "MicroPDF".to_string(),
                    bold: true,
                    italic: false,
                    link: Some("https://example.com".to_string()),
                },
            ]),
            ExportBlock::Table(vec![
                vec!["Metric".to_string(), "Value".to_string(), "Growth".to_string()],
                vec!["Revenue".to_string(), "12,450".to_string(), "25.5%".to_string()],
            ]),
        ]];

        let docx = render_blocks_to_docx("Q4 Report", &pages);
        assert!(docx.starts_with(b"PK\x03\x04"));
        let docx_entries = read_zip_entries(&docx);
        let doc_xml = &docx_entries.iter().find(|(n, _)| n == "word/document.xml").unwrap().1;
        let rels_xml = &docx_entries.iter().find(|(n, _)| n == "word/_rels/document.xml.rels").unwrap().1;
        assert!(doc_xml.contains("Heading1"));
        assert!(doc_xml.contains("Q4 Financial Summary"));
        assert!(doc_xml.contains("<w:hyperlink r:id=\"rId2\">"));
        assert!(doc_xml.contains("<w:tbl>"));
        assert!(rels_xml.contains("https://example.com"));

        let xlsx = render_blocks_to_xlsx(&pages);
        assert!(xlsx.starts_with(b"PK\x03\x04"));
        let xlsx_entries = read_zip_entries(&xlsx);
        let sheet1 = &xlsx_entries.iter().find(|(n, _)| n == "xl/worksheets/sheet1.xml").unwrap().1;
        let wb = &xlsx_entries.iter().find(|(n, _)| n == "xl/workbook.xml").unwrap().1;
        assert!(wb.contains("Table 1 (p.1)"));
        assert!(wb.contains("Document Text"));
        assert!(sheet1.contains("<v>12450</v>"), "numeric cell should store raw number: {sheet1}");
        assert!(sheet1.contains("<v>0.255</v>"), "percentage cell should store numeric ratio: {sheet1}");
    }

    #[test]
    fn test_tier0_xlsx_numbers_xml_sanitization_and_md_latex_escaping() {
        // Years / integers must NOT be formatted as 2-decimal floats; European comma decimals,
        // leading-zero codes, and >15-digit IDs must stay strings.
        assert_eq!(parse_excel_numeric("2026"), Some((2026.0, 5)));
        assert_eq!(parse_excel_numeric("12,450"), Some((12450.0, 3)));
        assert_eq!(parse_excel_numeric("12.50"), Some((12.5, 3)));
        assert_eq!(parse_excel_numeric("25.5%"), Some((0.255, 4)));
        assert_eq!(parse_excel_numeric("12,50"), None);
        assert_eq!(parse_excel_numeric("00123"), None);
        assert_eq!(parse_excel_numeric("12345678901234567"), None);

        // XML 1.0 forbids U+FFFE and U+FFFF
        let sanitized = xml_escape("A\u{FFFE}B\u{FFFF}C & <D>");
        assert_eq!(sanitized, "ABC &amp; &lt;D&gt;");

        // Markdown URL spaces/parentheses escaping
        let md = render_blocks_to_markdown(&[vec![ExportBlock::Paragraph(vec![StyledSpan {
            text: "Docs".to_string(),
            bold: false,
            italic: false,
            link: Some("https://example.com/a (b).pdf".to_string()),
        }])]]);
        assert!(md.contains("(https://example.com/a%20%28b%29.pdf)"), "md: {md}");

        // LaTeX tabular row starting with '[' must be prefixed with '{}' so LaTeX doesn't parse as \\[len]
        let tex = render_blocks_to_latex(
            "Test",
            &[vec![ExportBlock::Table(vec![
                vec!["Col1".to_string(), "Col2".to_string()],
                vec!["[Note]".to_string(), "Val".to_string()],
            ])]],
        );
        assert!(tex.contains("{}[Note] & Val \\\\"), "tex: {tex}");
    }

    #[test]
    fn test_milestone4_thin_rect_extraction_and_snapping() {
        let mut h = Vec::new();
        let mut v = Vec::new();
        // Two slightly jittered horizontal thin filled rectangles separated by a 1.5 pt gap
        extract_segments_from_thin_rect(10.0, 100.0, 499.6, 500.6, &mut h, &mut v);
        extract_segments_from_thin_rect(101.5, 220.0, 500.2, 501.4, &mut h, &mut v);
        // Thin vertical bar
        extract_segments_from_thin_rect(9.5, 10.5, 400.0, 501.0, &mut h, &mut v);
        // Thick shaded background box (NOT a border rule — must be ignored)
        extract_segments_from_thin_rect(12.0, 200.0, 460.0, 498.0, &mut h, &mut v);

        assert_eq!(h.len(), 2, "only thin horizontal rules should be captured");
        assert_eq!(v.len(), 1, "only thin vertical rules should be captured");

        let (hm, vm) = cluster_and_merge_segments(&h, &v, 1.8, 2.0);
        assert_eq!(hm.len(), 1, "collinear segments within 1.8pt Y and 2.0pt X gap must merge");
        assert!((hm[0].x0 - 10.0).abs() < 0.01 && (hm[0].x1 - 220.0).abs() < 0.01);
        assert_eq!(vm.len(), 1);
    }

    #[test]
    fn test_milestone4_side_by_side_connected_component_isolation() {
        // Two independent 2x2 tables sitting side-by-side on the same Y band (x=20..180 and x=260..420)
        let h_raw = vec![
            // Left table
            HSegment { y: 500.0, x0: 20.0, x1: 180.0 },
            HSegment { y: 470.0, x0: 20.0, x1: 180.0 },
            HSegment { y: 440.0, x0: 20.0, x1: 180.0 },
            // Right table (same Y band, disconnected X)
            HSegment { y: 500.0, x0: 260.0, x1: 420.0 },
            HSegment { y: 470.0, x0: 260.0, x1: 420.0 },
            HSegment { y: 440.0, x0: 260.0, x1: 420.0 },
        ];
        let v_raw = vec![
            // Left table
            VSegment { x: 20.0, y0: 440.0, y1: 500.0 },
            VSegment { x: 100.0, y0: 440.0, y1: 500.0 },
            VSegment { x: 180.0, y0: 440.0, y1: 500.0 },
            // Right table
            VSegment { x: 260.0, y0: 440.0, y1: 500.0 },
            VSegment { x: 340.0, y0: 440.0, y1: 500.0 },
            VSegment { x: 420.0, y0: 440.0, y1: 500.0 },
        ];
        let words = vec![
            WordBox { text: "LeftA".into(), left: 30.0, right: 70.0, bottom: 480.0, top: 492.0 },
            WordBox { text: "LeftB".into(), left: 110.0, right: 150.0, bottom: 480.0, top: 492.0 },
            WordBox { text: "10,50".into(), left: 115.0, right: 150.0, bottom: 450.0, top: 462.0 },
            WordBox { text: "RightA".into(), left: 270.0, right: 315.0, bottom: 480.0, top: 492.0 },
            WordBox { text: "RightB".into(), left: 350.0, right: 395.0, bottom: 480.0, top: 492.0 },
            WordBox { text: "20,75".into(), left: 355.0, right: 395.0, bottom: 450.0, top: 462.0 },
        ];

        let tables = extract_page_excel_tables_hybrid(&h_raw, &v_raw, &words, 1, ExcelExportMode::Lattice);
        assert_eq!(tables.len(), 2, "side-by-side tables must be isolated into 2 independent tables");
        assert_eq!(tables[0].rows[0], vec!["LeftA", "LeftB"]);
        assert_eq!(tables[0].rows[1], vec!["", "10,50"], "empty left cell must NOT shift right cell left");
        assert_eq!(tables[1].rows[0], vec!["RightA", "RightB"]);
        assert_eq!(tables[1].rows[1], vec!["", "20,75"]);
    }

    #[test]
    fn test_milestone4_merged_cells_and_empty_cell_preservation() {
        // 3 rows x 3 cols grid where Row 0 merges Cols 0..=2 (title banner),
        // Row 1 Col 0 merges Rows 1..=2 (row-span), and Row 2 Col 1 is empty while Row 2 Col 2 has a number.
        let h_raw = vec![
            HSegment { y: 600.0, x0: 0.0, x1: 300.0 },   // top
            HSegment { y: 570.0, x0: 0.0, x1: 300.0 },   // below banner
            HSegment { y: 540.0, x0: 100.0, x1: 300.0 }, // NO wall in col 0 between row 1 & 2!
            HSegment { y: 510.0, x0: 0.0, x1: 300.0 },   // bottom
        ];
        let v_raw = vec![
            VSegment { x: 0.0, y0: 510.0, y1: 600.0 },   // outer left
            VSegment { x: 100.0, y0: 510.0, y1: 570.0 }, // NO wall in row 0!
            VSegment { x: 200.0, y0: 510.0, y1: 570.0 }, // NO wall in row 0!
            VSegment { x: 300.0, y0: 510.0, y1: 600.0 }, // outer right
        ];
        let words = vec![
            WordBox { text: "FX".into(), left: 110.0, right: 130.0, bottom: 580.0, top: 592.0 },
            WordBox { text: "Rates".into(), left: 140.0, right: 185.0, bottom: 580.0, top: 592.0 },
            WordBox { text: "USD".into(), left: 20.0, right: 55.0, bottom: 535.0, top: 547.0 },
            WordBox { text: "Spot".into(), left: 115.0, right: 155.0, bottom: 550.0, top: 562.0 },
            WordBox { text: "123,9500".into(), left: 215.0, right: 275.0, bottom: 550.0, top: 562.0 },
            // Row 2 Col 1 intentionally empty!
            WordBox { text: "0,7728".into(), left: 215.0, right: 270.0, bottom: 520.0, top: 532.0 },
        ];

        let tables = extract_page_excel_tables_hybrid(&h_raw, &v_raw, &words, 1, ExcelExportMode::AutoHybrid);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert!(t.is_lattice);
        assert_eq!(t.rows[0], vec!["FX Rates", "", ""]);
        assert_eq!(t.rows[1], vec!["USD", "Spot", "123,9500"]);
        assert_eq!(t.rows[2], vec!["", "", "0,7728"], "empty middle cell must keep col 2 value in col 2");
        assert!(
            t.merges.contains(&CellMerge { row: 0, col: 0, row_span: 1, col_span: 3 }),
            "expected A1:C1 merge, got {:?}",
            t.merges
        );
        assert!(
            t.merges.contains(&CellMerge { row: 1, col: 0, row_span: 2, col_span: 1 }),
            "expected A2:A3 merge, got {:?}",
            t.merges
        );
    }

    #[test]
    fn test_milestone4_financial_number_parsing() {
        // Comma-decimal normalization ON (default for Milestone 4)
        let (v1, s1) = parse_excel_numeric_with_opts("123,9500", true).unwrap();
        assert!((v1 - 123.95).abs() < 1e-9 && s1 == 8, "4 decimals -> style 8 (#,##0.0000)");

        let (v2, s2) = parse_excel_numeric_with_opts("0,7728", true).unwrap();
        assert!((v2 - 0.7728).abs() < 1e-9 && s2 == 8);

        let (v3, s3) = parse_excel_numeric_with_opts("-45,50", true).unwrap();
        assert!((v3 - (-45.5)).abs() < 1e-9 && s3 == 3, "2 decimals -> style 3 (#,##0.00)");

        // Standard period decimals & thousands
        let (v4, s4) = parse_excel_numeric_with_opts("121.4000", true).unwrap();
        assert!((v4 - 121.4).abs() < 1e-9 && s4 == 8);

        let (v5, s5) = parse_excel_numeric_with_opts("1,234.56", true).unwrap();
        assert!((v5 - 1234.56).abs() < 1e-9 && s5 == 3);

        // Non-numeric strings & codes preserved as text
        assert_eq!(parse_excel_numeric_with_opts("N/A", true), None);
        assert_eq!(parse_excel_numeric_with_opts("USD", true), None);
        assert_eq!(parse_excel_numeric_with_opts("00123", true), None);
        assert_eq!(parse_excel_numeric_with_opts("12345678901234567", true), None);

        // Comma-decimal normalization OFF leaves European comma-decimals as text
        assert_eq!(parse_excel_numeric_with_opts("123,9500", false), None);
        assert_eq!(parse_excel_numeric_with_opts("0,7728", false), None);
    }

    #[test]
    fn test_milestone4_stream_gutter_extraction_and_xlsx_merge_xml() {
        // Unruled 3-column table with an empty middle cell on row 2
        let words = vec![
            WordBox { text: "Currency".into(), left: 20.0, right: 70.0, bottom: 500.0, top: 510.0 },
            WordBox { text: "Bid".into(), left: 140.0, right: 165.0, bottom: 500.0, top: 510.0 },
            WordBox { text: "Ask".into(), left: 230.0, right: 255.0, bottom: 500.0, top: 510.0 },

            WordBox { text: "EUR".into(), left: 20.0, right: 48.0, bottom: 480.0, top: 490.0 },
            WordBox { text: "1,0850".into(), left: 138.0, right: 175.0, bottom: 480.0, top: 490.0 },
            WordBox { text: "1,0862".into(), left: 228.0, right: 265.0, bottom: 480.0, top: 490.0 },

            WordBox { text: "JPY".into(), left: 20.0, right: 45.0, bottom: 460.0, top: 470.0 },
            // Bid intentionally missing!
            WordBox { text: "152,4000".into(), left: 225.0, right: 270.0, bottom: 460.0, top: 470.0 },
        ];
        let tables = extract_page_excel_tables_hybrid(&[], &[], &words, 1, ExcelExportMode::AutoHybrid);
        assert_eq!(tables.len(), 1);
        assert!(!tables[0].is_lattice);
        assert_eq!(tables[0].rows[0], vec!["Currency", "Bid", "Ask"]);
        assert_eq!(tables[0].rows[1], vec!["EUR", "1,0850", "1,0862"]);
        assert_eq!(tables[0].rows[2], vec!["JPY", "", "152,4000"], "Stream mode must preserve empty middle cell");

        // Verify OOXML ZIP with <mergeCells>, <numFmts>, wrapText="1", and stacked 2-blank-row layout
        let mut t1 = tables[0].clone();
        t1.merges.push(CellMerge { row: 0, col: 0, row_span: 1, col_span: 2 });
        let t2 = tables[0].clone();
        let xlsx = render_tables_to_xlsx(
            &[t1, t2],
            ExcelExportOptions {
                mode: ExcelExportMode::AutoHybrid,
                layout: ExcelTableLayout::StackOnOneSheet,
                normalize_comma_decimals: true,
            },
        );
        let entries = read_zip_entries(&xlsx);
        let sheet1 = &entries.iter().find(|(n, _)| n == "xl/worksheets/sheet1.xml").unwrap().1;
        let styles = &entries.iter().find(|(n, _)| n == "xl/styles.xml").unwrap().1;
        assert!(sheet1.contains("<mergeCells count=\"1\"><mergeCell ref=\"A1:B1\"/></mergeCells>"), "sheet1: {sheet1}");
        assert!(sheet1.contains("<v>1.085</v>") && styles.contains("formatCode=\"#,##0.0000\""), "4-decimal comma float must write numeric f64 with #,##0.0000 format: {sheet1}");
        assert!(sheet1.contains("<v>152.4</v>"), "sheet1: {sheet1}");
        assert!(styles.contains("wrapText=\"1\""), "styles: {styles}");
    }

    #[test]
    fn test_milestone4_to_visual_coords_all_rotations() {
        let (pw, ph) = (595.0f32, 842.0f32);

        // 0°: top-left origin (x, page_h - y)
        assert_eq!(to_visual_coords(0.0, ph, pw, ph, 0), (0.0, 0.0));
        assert_eq!(to_visual_coords(pw, 0.0, pw, ph, 0), (pw, ph));

        // 90° CW (/Rotate 90): portrait 595x842 rotated CW -> landscape 842x595
        // Unrotated bottom-left (0, 0) -> visual top-left (0, 0);
        // Unrotated top-left (0, 842) -> visual top-right (842, 0);
        // Unrotated top-right (595, 842) -> visual bottom-right (842, 595).
        assert_eq!(to_visual_coords(0.0, 0.0, pw, ph, 90), (0.0, 0.0));
        assert_eq!(to_visual_coords(0.0, ph, pw, ph, 90), (ph, 0.0));
        assert_eq!(to_visual_coords(pw, ph, pw, ph, 90), (ph, pw));
        assert_eq!(to_visual_coords(pw, 0.0, pw, ph, 90), (0.0, pw));

        // Verify a word "0,7981" written along +Y at constant X on a /Rotate 90 page reads left-to-right:
        let (vx0, vy0) = to_visual_coords(120.0, 100.0, pw, ph, 90);
        let (vx1, vy1) = to_visual_coords(120.0, 140.0, pw, ph, 90);
        assert!(vx0 < vx1 && (vy0 - vy1).abs() < 0.01, "90° CW text must read left-to-right");

        // 180°: (page_w - x, y)
        assert_eq!(to_visual_coords(pw, 0.0, pw, ph, 180), (0.0, 0.0));
        assert_eq!(to_visual_coords(0.0, ph, pw, ph, 180), (pw, ph));

        // 270° CW (/Rotate 270): (page_h - y, page_w - x)
        assert_eq!(to_visual_coords(pw, ph, pw, ph, 270), (0.0, 0.0));
        assert_eq!(to_visual_coords(0.0, ph, pw, ph, 270), (0.0, pw));
        assert_eq!(to_visual_coords(0.0, 0.0, pw, ph, 270), (ph, pw));
    }

    #[test]
    fn test_milestone4_xlsx_ooxml_strict_compliance() {
        let grid = ExcelTableGrid {
            page_num: 1,
            is_lattice: true,
            bbox: (0.0, 0.0, 300.0, 100.0),
            rows: vec![
                vec!["Merged Header".to_string(), "".to_string(), "Rate".to_string()],
                vec!["USD".to_string(), "1,0850".to_string(), "1,0900".to_string()],
                vec!["".to_string(), "1,0860".to_string(), "1,0910".to_string()],
            ],
            merges: vec![
                // Horizontal merge A1:B1 (B1 is covered)
                CellMerge { row: 0, col: 0, row_span: 1, col_span: 2 },
                // Vertical merge A2:A3 (A3 is covered)
                CellMerge { row: 1, col: 0, row_span: 2, col_span: 1 },
            ],
        };
        let xlsx = render_tables_to_xlsx(
            &[grid],
            ExcelExportOptions {
                mode: ExcelExportMode::Lattice,
                layout: ExcelTableLayout::StackOnOneSheet,
                normalize_comma_decimals: true,
            },
        );
        let entries = read_zip_entries(&xlsx);
        let styles = &entries.iter().find(|(n, _)| n == "xl/styles.xml").unwrap().1;
        let sheet1 = &entries.iter().find(|(n, _)| n == "xl/worksheets/sheet1.xml").unwrap().1;
        let ct = &entries.iter().find(|(n, _)| n == "[Content_Types].xml").unwrap().1;

        // 1. xl/styles.xml has <cellStyles> immediately after </cellXfs>
        assert!(
            styles.contains("</cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>"),
            "styles.xml missing <cellStyles> after </cellXfs>: {styles}"
        );

        // 2. Worksheet has <dimension> before <sheetViews> before <cols>
        let dim_pos = sheet1.find("<dimension ref=\"A1:C3\"/>").expect("missing <dimension>");
        let views_pos = sheet1
            .find("<sheetViews><sheetView tabSelected=\"1\" workbookViewId=\"0\"/></sheetViews>")
            .expect("missing <sheetViews>");
        let cols_pos = sheet1.find("<cols>").expect("missing <cols>");
        assert!(dim_pos < views_pos && views_pos < cols_pos, "worksheet element order invalid: {sheet1}");

        // 3. Non-top-left merged cells B1 and A3 are self-closing empty styled cells with no <is>/<v>
        let b1_start = sheet1.find("<c r=\"B1\"").expect("missing B1");
        let b1_tag = &sheet1[b1_start..sheet1[b1_start..].find('>').unwrap() + b1_start + 1];
        assert!(b1_tag.ends_with("/>") && !b1_tag.contains("t="), "B1 must be empty styled cell: {b1_tag}");

        let a3_start = sheet1.find("<c r=\"A3\"").expect("missing A3");
        let a3_tag = &sheet1[a3_start..sheet1[a3_start..].find('>').unwrap() + a3_start + 1];
        assert!(a3_tag.ends_with("/>") && !a3_tag.contains("t="), "A3 must be empty styled cell: {a3_tag}");

        assert!(ct.contains("/xl/worksheets/sheet1.xml"), "Content_Types missing sheet1");
    }
}

