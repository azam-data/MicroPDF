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

/// Converts an open `PdfDocument` to a Microsoft Excel OOXML (`.xlsx`) workbook.
/// Detected tables become clean formatted worksheets with typed numbers & headers; non-table
/// documents export cleanly as structured text sheets.
pub fn export_document_to_xlsx(doc: &PdfDocument) -> Vec<u8> {
    let pages = extract_document_blocks(doc);
    render_blocks_to_xlsx(&pages)
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
// Pure-Rust Excel (.xlsx) OOXML Generator
// ---------------------------------------------------------------------------

fn xlsx_col_letter(mut col_zero_based: usize) -> String {
    let mut chars = Vec::new();
    loop {
        chars.push((b'A' + (col_zero_based % 26) as u8) as char);
        if col_zero_based < 26 {
            break;
        }
        col_zero_based = col_zero_based / 26 - 1;
    }
    chars.into_iter().rev().collect()
}

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

fn parse_excel_numeric(cell: &str) -> Option<(f64, u8)> {
    let s = cell.trim();
    if s.is_empty() || s.len() > 24 {
        return None;
    }
    // Style 3 = decimal/comma float (#,##0.00), Style 4 = percentage (0.00%), Style 5 = plain integer (General)
    if let Some(pct) = s.strip_suffix('%') {
        let pt = pct.trim();
        if !is_valid_grouped_number(pt) {
            return None;
        }
        let cleaned = pt.replace(',', "");
        if let Ok(v) = cleaned.parse::<f64>() {
            if v.is_finite() {
                return Some((v / 100.0, 4));
            }
        }
        return None;
    }
    if !is_valid_grouped_number(s) {
        return None;
    }
    let has_decimal_or_comma = s.contains('.') || s.contains(',');
    let cleaned = s.replace(',', "");
    if let Ok(v) = cleaned.parse::<f64>() {
        if v.is_finite() {
            let style = if has_decimal_or_comma { 3 } else { 5 };
            return Some((v, style));
        }
    }
    None
}

fn build_xlsx_sheet_xml(rows: &[Vec<String>], bold_first_row: bool) -> String {
    let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let mut col_widths = vec![12usize; max_cols];
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            let len = cell.chars().count();
            if len + 3 > col_widths[c] {
                col_widths[c] = (len + 3).min(56);
            }
        }
    }

    let mut xml = String::with_capacity(2048);
    xml.push_str(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <cols>",
    );
    for (i, w) in col_widths.iter().enumerate() {
        let col_idx = i + 1;
        xml.push_str(&format!(
            "<col min=\"{col_idx}\" max=\"{col_idx}\" width=\"{w}\" customWidth=\"1\"/>"
        ));
    }
    xml.push_str("</cols><sheetData>");

    for (r_idx, row) in rows.iter().enumerate() {
        let row_num = r_idx + 1;
        let is_hdr = bold_first_row && r_idx == 0;
        xml.push_str(&format!("<row r=\"{row_num}\">"));
        for (c_idx, val) in row.iter().enumerate() {
            let col_ref = format!("{}{}", xlsx_col_letter(c_idx), row_num);
            if !is_hdr {
                if let Some((num, num_style)) = parse_excel_numeric(val) {
                    xml.push_str(&format!(
                        "<c r=\"{col_ref}\" s=\"{num_style}\"><v>{num}</v></c>"
                    ));
                    continue;
                }
            }
            let style = if is_hdr { 1 } else { 2 };
            xml.push_str(&format!(
                "<c r=\"{col_ref}\" t=\"inlineStr\" s=\"{style}\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                xml_escape(val)
            ));
        }
        xml.push_str("</row>");
    }

    xml.push_str("</sheetData></worksheet>");
    xml
}

pub fn render_blocks_to_xlsx(pages: &[Vec<ExportBlock>]) -> Vec<u8> {
    let mut sheets: Vec<(String, Vec<Vec<String>>, bool)> = Vec::new();
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
                        sheets.push((name, rows.clone(), true));
                    } else if let Some((_, last_rows, _)) = sheets.last_mut() {
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
        sheets.push(("Document Text".to_string(), with_hdr, true));
    }

    if sheets.is_empty() {
        sheets.push((
            "Sheet1".to_string(),
            vec![vec!["(No extractable text or tables found in PDF)".to_string()]],
            false,
        ));
    }

    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    let mut wb_sheets = String::new();
    let mut wb_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );

    let mut zip = MiniZipWriter::new();
    for (idx, (raw_name, rows, bold_hdr)) in sheets.iter().enumerate() {
        let s_num = idx + 1;
        let rid = format!("rId{}", s_num + 1);
        // Excel sheet names max 31 chars, no : \ / ? * [ ]
        let safe_name: String = raw_name
            .chars()
            .filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
            .take(31)
            .collect();
        content_types.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{s_num}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
        wb_sheets.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{s_num}\" r:id=\"{rid}\"/>",
            xml_escape(&safe_name)
        ));
        wb_rels.push_str(&format!(
            "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{s_num}.xml\"/>"
        ));
        let sheet_xml = build_xlsx_sheet_xml(rows, *bold_hdr);
        zip.add_file(&format!("xl/worksheets/sheet{s_num}.xml"), sheet_xml.as_bytes());
    }
    content_types.push_str("</Types>");
    wb_rels.push_str("</Relationships>");

    let workbook_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets>{wb_sheets}</sheets></workbook>"
    );

    // Cell styles: 0=default, 1=bold blue-tinted header with border, 2=text with border, 3=number with border, 4=percent with border
    let styles_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
        <fonts count=\"2\">\
          <font><sz val=\"11\"/><name val=\"Calibri\"/></font>\
          <font><b/><sz val=\"11\"/><color rgb=\"FF0F172A\"/><name val=\"Calibri\"/></font>\
        </fonts>\
        <fills count=\"3\">\
          <fill><patternFill patternType=\"none\"/></fill>\
          <fill><patternFill patternType=\"gray125\"/></fill>\
          <fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFEFF6FF\"/></patternFill></fill>\
        </fills>\
        <borders count=\"2\">\
          <border><left/><right/><top/><bottom/><diagonal/></border>\
          <border>\
            <left style=\"thin\"><color rgb=\"FFCBD5E1\"/></left>\
            <right style=\"thin\"><color rgb=\"FFCBD5E1\"/></right>\
            <top style=\"thin\"><color rgb=\"FFCBD5E1\"/></top>\
            <bottom style=\"thin\"><color rgb=\"FFCBD5E1\"/></bottom>\
            <diagonal/>\
          </border>\
        </borders>\
        <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
        <cellXfs count=\"6\">\
          <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
          <xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\"/>\
          <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"/>\
          <xf numFmtId=\"4\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyNumberFormat=\"1\" applyBorder=\"1\"/>\
          <xf numFmtId=\"10\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyNumberFormat=\"1\" applyBorder=\"1\"/>\
          <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"/>\
        </cellXfs>\
        </styleSheet>";

    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
        </Relationships>";

    zip.add_file("[Content_Types].xml", content_types.as_bytes());
    zip.add_file("_rels/.rels", root_rels.as_bytes());
    zip.add_file("xl/workbook.xml", workbook_xml.as_bytes());
    zip.add_file("xl/styles.xml", styles_xml.as_bytes());
    zip.add_file("xl/_rels/workbook.xml.rels", wb_rels.as_bytes());
    zip.finish()
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
}

