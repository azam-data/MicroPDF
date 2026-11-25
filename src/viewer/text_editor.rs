//! Lightweight pure-Rust multi-line text editor for `.md` / `.tex` tabs.
//!
//! Zero GPU, zero GUI toolkit: renders directly into `softbuffer` with soft word wrap,
//! zoom scaling, line numbers, vertical scrollbar, LaTeX/Markdown syntax highlighting,
//! selection highlight, caret, bounded undo/redo (64 snapshots), and clipboard support.

use std::collections::VecDeque;

use super::render::fill_rect_clipped;
use super::theme::UiTheme;
use super::ui_draw::*;

const MAX_EDITOR_UNDO: usize = 64;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(super) enum MdEditorMode {
    #[default]
    Preview,
    Split,
    Edit,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum EditorSyntax {
    Markdown,
    LaTeX,
}

#[derive(Clone)]
struct EditorSnap {
    lines: Vec<String>,
    cursor: (usize, usize),
    sel_anchor: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VisualRow {
    line_idx: usize,
    start_byte: usize,
    end_byte: usize,
    is_first_of_line: bool,
}

pub(super) struct TextEditorState {
    pub(super) lines: Vec<String>,
    /// `(row, byte_col)` always aligned to a valid UTF-8 char boundary.
    pub(super) cursor: (usize, usize),
    pub(super) sel_anchor: Option<(usize, usize)>,
    pub(super) scroll_y: f32,
    pub(super) dragging: bool,
    pub(super) scrollbar_drag: bool,
    content_gen: u64,
    wrap_key: (u32, u32, u64),
    visual_rows: Vec<VisualRow>,
    undo: VecDeque<EditorSnap>,
    redo: VecDeque<EditorSnap>,
}

#[derive(Copy, Clone)]
struct EditorMetrics {
    s: f32,
    font_sz: f32,
    line_h: f32,
    gutter_w: u32,
    pad_y: f32,
    text_pad_l: i32,
    sb_reserve: u32,
}

fn compute_metrics(sf: f64, zoom: f32) -> EditorMetrics {
    let s = sf as f32;
    let z = zoom.clamp(0.5, 3.0);
    let font_sz = (13.5 * s * z).clamp(9.0, 34.0);
    let line_h = (font_sz * 1.48).max(13.0);
    let gutter_w = ((32.0 * s + font_sz * 1.4).round() as u32).max(36);
    let pad_y = 12.0 * s;
    let text_pad_l = (10.0 * s).round() as i32;
    let sb_reserve = (15.0 * s).round().clamp(12.0, 22.0) as u32;
    EditorMetrics {
        s,
        font_sz,
        line_h,
        gutter_w,
        pad_y,
        text_pad_l,
        sb_reserve,
    }
}

/// Soft-wraps a single logical line into contiguous UTF-8 byte ranges `[start, end)`
/// so that no character overflows `max_w`. Prefers whitespace word boundaries and
/// gracefully splits long unbroken tokens (URLs, long commands) by character boundary.
fn wrap_single_line(line: &str, font_sz: f32, max_w: f32) -> Vec<(usize, usize)> {
    if line.is_empty() || max_w <= 24.0 {
        return vec![(0, line.len())];
    }
    if measure_text(line, font_sz, false) <= max_w {
        return vec![(0, line.len())];
    }

    // Tokenize into alternating non-whitespace + trailing whitespace chunks.
    let mut tokens: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    let bytes = line.as_bytes();
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == start {
            // Non-ASCII safety advance by one UTF-8 char
            if let Some(ch) = line[i..].chars().next() {
                i += ch.len_utf8();
            } else {
                break;
            }
        }
        tokens.push((start, i));
    }

    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut row_start = 0usize;
    let mut row_end = 0usize;

    let mut ti = 0usize;
    while ti < tokens.len() {
        let (ts, te) = tokens[ti];
        if row_start == row_end {
            // First token on this visual row
            if measure_text(&line[ts..te], font_sz, false) <= max_w {
                row_start = ts;
                row_end = te;
                ti += 1;
            } else {
                // Single token exceeds max_w: break by character boundary
                let mut seg_s = ts;
                while seg_s < te {
                    let mut seg_e = seg_s;
                    for (rel_idx, ch) in line[seg_s..te].char_indices() {
                        let cand_e = seg_s + rel_idx + ch.len_utf8();
                        if cand_e > seg_s && measure_text(&line[seg_s..cand_e], font_sz, false) > max_w {
                            if seg_e == seg_s {
                                // Always make progress by at least 1 char
                                seg_e = cand_e;
                            }
                            break;
                        }
                        seg_e = cand_e;
                    }
                    if seg_e == seg_s {
                        if let Some(ch) = line[seg_s..te].chars().next() {
                            seg_e = seg_s + ch.len_utf8();
                        } else {
                            break;
                        }
                    }
                    if seg_e < te {
                        rows.push((seg_s, seg_e));
                        seg_s = seg_e;
                    } else {
                        row_start = seg_s;
                        row_end = te;
                        break;
                    }
                }
                ti += 1;
            }
        } else if measure_text(&line[row_start..te], font_sz, false) <= max_w {
            row_end = te;
            ti += 1;
        } else {
            rows.push((row_start, row_end));
            row_start = ts;
            row_end = ts;
        }
    }

    if row_start < row_end || rows.is_empty() {
        rows.push((row_start, row_end));
    }
    rows
}

#[derive(Copy, Clone)]
struct SyntaxPalette {
    plain: u32,
    comment: u32,
    command: u32,
    env_name: u32,
    brace: u32,
    math: u32,
}

fn syntax_palette_for_bg(page_bg: u32, theme: &UiTheme) -> SyntaxPalette {
    let r = ((page_bg >> 16) & 0xFF) as f32;
    let g = ((page_bg >> 8) & 0xFF) as f32;
    let b = (page_bg & 0xFF) as f32;
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    if luma < 128.0 {
        SyntaxPalette {
            plain: theme.text_primary,
            comment: 0x007A_A874,  // soft green
            command: 0x007A_B8FF,  // bright sky blue
            env_name: 0x00D8_96FF, // orchid purple
            brace: 0x00F0_B266,    // warm amber
            math: 0x0052_D1B8,     // crisp teal
        }
    } else {
        SyntaxPalette {
            plain: theme.text_primary,
            comment: 0x004A_7544,  // forest green
            command: 0x001A_53B5,  // cobalt blue
            env_name: 0x007E_22CE, // deep purple
            brace: 0x00B4_5309,    // burnt amber
            math: 0x000F_766E,     // deep teal
        }
    }
}

/// Tokenizes a single logical LaTeX line into contiguous `(start_byte, end_byte, color)` spans.
fn highlight_latex_line(line: &str, pal: SyntaxPalette) -> Vec<(usize, usize, u32)> {
    let mut spans: Vec<(usize, usize, u32)> = Vec::new();
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    let mut in_math = false;
    let mut expect_env_brace = false;

    while i < n {
        let b = bytes[i];

        // Unescaped `%` starts a comment to end of line
        if b == b'%' {
            spans.push((i, n, pal.comment));
            break;
        }

        // Backslash command (`\section`, `\frac`, `\\`, `\%`, `\[`, `\(`, etc.)
        if b == b'\\' {
            let start = i;
            i += 1;
            if i < n {
                if bytes[i].is_ascii_alphabetic() {
                    while i < n && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'*') {
                        i += 1;
                    }
                    let cmd = &line[start..i];
                    expect_env_brace = cmd == "\\begin" || cmd == "\\end";
                    spans.push((start, i, pal.command));
                } else {
                    // Single-char escaped command (`\\`, `\%`, `\(`, `\[`, `\{`, etc.)
                    if let Some(ch) = line[i..].chars().next() {
                        i += ch.len_utf8();
                    }
                    let tok = &line[start..i];
                    let col = if matches!(tok, "\\(" | "\\)" | "\\[" | "\\]") {
                        if tok == "\\(" || tok == "\\[" {
                            in_math = true;
                        } else {
                            in_math = false;
                        }
                        pal.math
                    } else {
                        pal.command
                    };
                    expect_env_brace = false;
                    spans.push((start, i, col));
                }
            } else {
                spans.push((start, i, pal.command));
            }
            continue;
        }

        // Inline / display math delimiters (`$` or `$$`)
        if b == b'$' {
            let start = i;
            i += 1;
            if i < n && bytes[i] == b'$' {
                i += 1;
            }
            in_math = !in_math;
            expect_env_brace = false;
            spans.push((start, i, pal.math));
            continue;
        }

        // Braces and brackets
        if matches!(b, b'{' | b'}' | b'[' | b']') {
            spans.push((i, i + 1, pal.brace));
            i += 1;
            if b == b'{' && expect_env_brace {
                let env_s = i;
                while i < n && bytes[i] != b'}' && bytes[i] != b'%' {
                    i += 1;
                }
                if i > env_s {
                    spans.push((env_s, i, pal.env_name));
                }
                expect_env_brace = false;
            } else if b != b'[' {
                expect_env_brace = false;
            }
            continue;
        }

        // Math operators / alignment markers (`^`, `_`, `&`)
        if matches!(b, b'^' | b'_' | b'&') {
            spans.push((i, i + 1, pal.math));
            i += 1;
            expect_env_brace = false;
            continue;
        }

        // Plain / math text run
        let start = i;
        while i < n && !matches!(bytes[i], b'%' | b'\\' | b'$' | b'{' | b'}' | b'[' | b']' | b'^' | b'_' | b'&') {
            i += 1;
        }
        let col = if in_math { pal.math } else { pal.plain };
        spans.push((start, i, col));
    }

    spans
}

/// Tokenizes a single logical Markdown line into contiguous `(start_byte, end_byte, color)` spans.
fn highlight_markdown_line(line: &str, pal: SyntaxPalette) -> Vec<(usize, usize, u32)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return vec![(0, line.len(), pal.command)];
    }
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return vec![(0, line.len(), pal.math)];
    }
    if trimmed.starts_with('>') {
        return vec![(0, line.len(), pal.comment)];
    }

    let mut spans = Vec::new();
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    let mut in_code = false;
    while i < n {
        if bytes[i] == b'`' {
            let start = i;
            i += 1;
            in_code = !in_code;
            spans.push((start, i, pal.math));
            continue;
        }
        if !in_code && matches!(bytes[i], b'[' | b']' | b'(' | b')') {
            spans.push((i, i + 1, pal.brace));
            i += 1;
            continue;
        }
        let start = i;
        while i < n && bytes[i] != b'`' && (in_code || !matches!(bytes[i], b'[' | b']' | b'(' | b')')) {
            i += 1;
        }
        spans.push((start, i, if in_code { pal.math } else { pal.plain }));
    }
    spans
}

impl TextEditorState {
    pub(super) fn from_text(src: &str) -> Self {
        let mut lines: Vec<String> = src.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            lines,
            cursor: (0, 0),
            sel_anchor: None,
            scroll_y: 0.0,
            dragging: false,
            scrollbar_drag: false,
            content_gen: 1,
            wrap_key: (0, 0, 0),
            visual_rows: Vec::new(),
            undo: VecDeque::with_capacity(MAX_EDITOR_UNDO),
            redo: VecDeque::with_capacity(MAX_EDITOR_UNDO),
        }
    }

    pub(super) fn to_text(&self) -> String {
        self.lines.join("\n")
    }

    fn bump_gen(&mut self) {
        self.content_gen = self.content_gen.wrapping_add(1).max(1);
    }

    fn snap(&self) -> EditorSnap {
        EditorSnap {
            lines: self.lines.clone(),
            cursor: self.cursor,
            sel_anchor: self.sel_anchor,
        }
    }

    fn push_undo(&mut self) {
        if self.undo.len() >= MAX_EDITOR_UNDO {
            self.undo.pop_front();
        }
        self.undo.push_back(self.snap());
        self.redo.clear();
    }

    pub(super) fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push_back(self.snap());
        self.lines = prev.lines;
        self.cursor = prev.cursor;
        self.sel_anchor = prev.sel_anchor;
        self.clamp_cursor();
        self.bump_gen();
        true
    }

    pub(super) fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop_back() else {
            return false;
        };
        if self.undo.len() >= MAX_EDITOR_UNDO {
            self.undo.pop_front();
        }
        self.undo.push_back(self.snap());
        self.lines = next.lines;
        self.cursor = next.cursor;
        self.sel_anchor = next.sel_anchor;
        self.clamp_cursor();
        self.bump_gen();
        true
    }

    fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor.0 = self.cursor.0.min(self.lines.len() - 1);
        let line = &self.lines[self.cursor.0];
        let mut c = self.cursor.1.min(line.len());
        while c > 0 && !line.is_char_boundary(c) {
            c -= 1;
        }
        self.cursor.1 = c;
    }

    pub(super) fn selected_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.sel_anchor?;
        if anchor == self.cursor {
            return None;
        }
        if anchor < self.cursor {
            Some((anchor, self.cursor))
        } else {
            Some((self.cursor, anchor))
        }
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        let ((r0, c0), (r1, c1)) = self.selected_range()?;
        if r0 == r1 {
            return Some(self.lines[r0][c0..c1].to_string());
        }
        let mut out = String::new();
        out.push_str(&self.lines[r0][c0..]);
        for r in (r0 + 1)..r1 {
            out.push('\n');
            out.push_str(&self.lines[r]);
        }
        out.push('\n');
        out.push_str(&self.lines[r1][..c1]);
        Some(out)
    }

    pub(super) fn delete_selection(&mut self) -> bool {
        let Some(((r0, c0), (r1, c1))) = self.selected_range() else {
            return false;
        };
        if r0 == r1 {
            self.lines[r0].replace_range(c0..c1, "");
        } else {
            let tail = self.lines[r1][c1..].to_string();
            self.lines[r0].truncate(c0);
            self.lines[r0].push_str(&tail);
            self.lines.drain((r0 + 1)..=r1);
        }
        self.cursor = (r0, c0);
        self.sel_anchor = None;
        self.clamp_cursor();
        self.bump_gen();
        true
    }

    pub(super) fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.push_undo();
        self.delete_selection();
        let clean = text.replace('\r', "");
        let mut parts = clean.split('\n');
        let first = parts.next().unwrap_or("");
        let (r, c) = self.cursor;
        let tail = self.lines[r][c..].to_string();
        self.lines[r].truncate(c);
        self.lines[r].push_str(first);
        let mut cur_r = r;
        let mut cur_c = self.lines[r].len();
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            self.lines[r].push_str(&tail);
        } else {
            for (idx, part) in rest.iter().enumerate() {
                cur_r += 1;
                let mut line = (*part).to_string();
                if idx + 1 == rest.len() {
                    cur_c = line.len();
                    line.push_str(&tail);
                }
                self.lines.insert(cur_r, line);
            }
        }
        self.cursor = (cur_r, cur_c);
        self.sel_anchor = None;
        self.bump_gen();
    }

    pub(super) fn backspace(&mut self) {
        self.push_undo();
        if self.delete_selection() {
            return;
        }
        let (r, c) = self.cursor;
        if c > 0 {
            let line = &mut self.lines[r];
            let mut prev = c - 1;
            while prev > 0 && !line.is_char_boundary(prev) {
                prev -= 1;
            }
            line.replace_range(prev..c, "");
            self.cursor = (r, prev);
            self.bump_gen();
        } else if r > 0 {
            let cur_line = self.lines.remove(r);
            let prev_len = self.lines[r - 1].len();
            self.lines[r - 1].push_str(&cur_line);
            self.cursor = (r - 1, prev_len);
            self.bump_gen();
        }
    }

    pub(super) fn delete_forward(&mut self) {
        self.push_undo();
        if self.delete_selection() {
            return;
        }
        let (r, c) = self.cursor;
        if c < self.lines[r].len() {
            let line = &mut self.lines[r];
            let mut next = c + 1;
            while next < line.len() && !line.is_char_boundary(next) {
                next += 1;
            }
            line.replace_range(c..next, "");
            self.bump_gen();
        } else if r + 1 < self.lines.len() {
            let next_line = self.lines.remove(r + 1);
            self.lines[r].push_str(&next_line);
            self.bump_gen();
        }
    }

    pub(super) fn select_all(&mut self) {
        self.sel_anchor = Some((0, 0));
        let last_r = self.lines.len().saturating_sub(1);
        let last_c = self.lines[last_r].len();
        self.cursor = (last_r, last_c);
    }

    pub(super) fn insert_newline_auto_indent(&mut self) {
        self.push_undo();
        self.delete_selection();
        let (r, c) = self.cursor;
        let line = &self.lines[r];
        let before = &line[..c];
        let indent_len = before.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
        let indent = before[..indent_len].to_string();
        let after_indent = &before[indent_len..];
        let list_prefix = if after_indent.starts_with("- [ ] ") || after_indent.starts_with("- [x] ") {
            Some("- [ ] ")
        } else if after_indent.starts_with("- ") {
            Some("- ")
        } else if after_indent.starts_with("* ") {
            Some("* ")
        } else if after_indent.starts_with("+ ") {
            Some("+ ")
        } else {
            None
        };
        if let Some(pfx) = list_prefix {
            if after_indent.trim() == pfx.trim() && c == line.len() {
                // Empty bullet line -> clear bullet on Enter
                self.lines[r].truncate(indent_len);
                self.cursor = (r, indent_len);
                self.bump_gen();
                return;
            }
            let tail = self.lines[r][c..].to_string();
            self.lines[r].truncate(c);
            let new_line = format!("{indent}{pfx}{tail}");
            let new_c = indent.len() + pfx.len();
            self.lines.insert(r + 1, new_line);
            self.cursor = (r + 1, new_c);
        } else {
            let tail = self.lines[r][c..].to_string();
            self.lines[r].truncate(c);
            let new_line = format!("{indent}{tail}");
            let new_c = indent.len();
            self.lines.insert(r + 1, new_line);
            self.cursor = (r + 1, new_c);
        }
        self.bump_gen();
    }

    pub(super) fn indent_or_outdent(&mut self, outdent: bool) {
        self.push_undo();
        let (r0, r1) = if let Some(((sr, _), (er, ec))) = self.selected_range() {
            let end_row = if er > sr && ec == 0 { er - 1 } else { er };
            (sr, end_row)
        } else {
            (self.cursor.0, self.cursor.0)
        };
        if !outdent && self.sel_anchor.is_none() {
            let (r, c) = self.cursor;
            self.lines[r].insert_str(c, "  ");
            self.cursor = (r, c + 2);
            self.bump_gen();
            return;
        }
        for r in r0..=r1 {
            if outdent {
                let remove = if self.lines[r].starts_with("  ") {
                    2
                } else if self.lines[r].starts_with(' ') || self.lines[r].starts_with('\t') {
                    1
                } else {
                    0
                };
                if remove > 0 {
                    self.lines[r].drain(..remove);
                    if self.cursor.0 == r {
                        self.cursor.1 = self.cursor.1.saturating_sub(remove);
                    }
                    if let Some((ar, ac)) = self.sel_anchor {
                        if ar == r {
                            self.sel_anchor = Some((ar, ac.saturating_sub(remove)));
                        }
                    }
                }
            } else {
                self.lines[r].insert_str(0, "  ");
                if self.cursor.0 == r {
                    self.cursor.1 += 2;
                }
                if let Some((ar, ac)) = self.sel_anchor {
                    if ar == r {
                        self.sel_anchor = Some((ar, ac + 2));
                    }
                }
            }
        }
        self.bump_gen();
    }

    pub(super) fn insert_char_smart(&mut self, ch: char) {
        let pair = match ch {
            '(' => Some(('(', ')')),
            '[' => Some(('[', ']')),
            '{' => Some(('{', '}')),
            '"' => Some(('"', '"')),
            '`' => Some(('`', '`')),
            '$' => Some(('$', '$')),
            _ => None,
        };
        if let Some((open, close)) = pair {
            if let Some(sel) = self.selected_text() {
                let wrapped = format!("{open}{sel}{close}");
                self.insert_str(&wrapped);
                return;
            }
            let (r, c) = self.cursor;
            if open == close && self.lines[r][c..].starts_with(close) {
                self.cursor = (r, c + close.len_utf8());
                return;
            }
            self.push_undo();
            let s = format!("{open}{close}");
            self.lines[r].insert_str(c, &s);
            self.cursor = (r, c + open.len_utf8());
            self.bump_gen();
        } else {
            let (r, c) = self.cursor;
            if matches!(ch, ')' | ']' | '}') && self.sel_anchor.is_none() && self.lines[r][c..].starts_with(ch) {
                self.cursor = (r, c + ch.len_utf8());
                return;
            }
            let mut buf = [0u8; 4];
            self.insert_str(ch.encode_utf8(&mut buf));
        }
    }

    pub(super) fn duplicate_line(&mut self) {
        self.push_undo();
        let (r, c) = self.cursor;
        let dup = self.lines[r].clone();
        self.lines.insert(r + 1, dup);
        self.cursor = (r + 1, c);
        self.sel_anchor = None;
        self.bump_gen();
    }

    pub(super) fn move_line(&mut self, dir: isize) {
        let r = self.cursor.0;
        if dir < 0 && r > 0 {
            self.push_undo();
            self.lines.swap(r, r - 1);
            self.cursor.0 = r - 1;
            self.sel_anchor = None;
            self.bump_gen();
        } else if dir > 0 && r + 1 < self.lines.len() {
            self.push_undo();
            self.lines.swap(r, r + 1);
            self.cursor.0 = r + 1;
            self.sel_anchor = None;
            self.bump_gen();
        }
    }

    pub(super) fn wrap_selection(&mut self, before: &str, after: &str) {
        if let Some(sel) = self.selected_text() {
            self.insert_str(&format!("{before}{sel}{after}"));
        } else {
            self.push_undo();
            let (r, c) = self.cursor;
            self.lines[r].insert_str(c, &format!("{before}{after}"));
            self.cursor = (r, c + before.len());
            self.bump_gen();
        }
    }

    pub(super) fn toggle_comment(&mut self, syntax: EditorSyntax) {
        self.push_undo();
        let r = self.cursor.0;
        match syntax {
            EditorSyntax::LaTeX => {
                if self.lines[r].starts_with("% ") {
                    self.lines[r].drain(..2);
                    self.cursor.1 = self.cursor.1.saturating_sub(2);
                } else if self.lines[r].starts_with('%') {
                    self.lines[r].drain(..1);
                    self.cursor.1 = self.cursor.1.saturating_sub(1);
                } else {
                    self.lines[r].insert_str(0, "% ");
                    self.cursor.1 += 2;
                }
            }
            EditorSyntax::Markdown => {
                let trimmed = self.lines[r].trim();
                if trimmed.starts_with("<!-- ") && trimmed.ends_with(" -->") && trimmed.len() >= 9 {
                    let inner = trimmed[5..trimmed.len() - 4].to_string();
                    self.lines[r] = inner;
                    self.cursor.1 = self.cursor.1.min(self.lines[r].len());
                } else {
                    let cur = self.lines[r].clone();
                    self.lines[r] = format!("<!-- {cur} -->");
                    self.cursor.1 += 5;
                }
            }
        }
        self.sel_anchor = None;
        self.bump_gen();
    }

    fn ensure_wrapped(&mut self, ew: u32, sf: f64, zoom: f32) -> EditorMetrics {
        let m = compute_metrics(sf, zoom);
        let key = (ew, m.font_sz.to_bits(), self.content_gen);
        if self.wrap_key == key && !self.visual_rows.is_empty() {
            return m;
        }
        let avail_w = (ew as i32 - m.gutter_w as i32 - m.text_pad_l - m.sb_reserve as i32 - (6.0 * m.s).round() as i32)
            .max(40) as f32;
        self.visual_rows.clear();
        for (line_idx, line) in self.lines.iter().enumerate() {
            let segs = wrap_single_line(line, m.font_sz, avail_w);
            for (si, (sb, eb)) in segs.into_iter().enumerate() {
                self.visual_rows.push(VisualRow {
                    line_idx,
                    start_byte: sb,
                    end_byte: eb,
                    is_first_of_line: si == 0,
                });
            }
        }
        if self.visual_rows.is_empty() {
            self.visual_rows.push(VisualRow {
                line_idx: 0,
                start_byte: 0,
                end_byte: 0,
                is_first_of_line: true,
            });
        }
        self.wrap_key = key;
        m
    }

    fn cursor_visual_row(&self) -> usize {
        let (cr, cc) = self.cursor;
        let mut last_for_line = 0usize;
        let mut found_line = false;
        for (vi, vr) in self.visual_rows.iter().enumerate() {
            if vr.line_idx == cr {
                found_line = true;
                last_for_line = vi;
                if cc >= vr.start_byte && (cc < vr.end_byte || (cc == vr.end_byte && vr.end_byte == self.lines[cr].len())) {
                    return vi;
                }
            } else if found_line {
                break;
            }
        }
        last_for_line
    }

    pub(super) fn move_cursor(&mut self, dr: isize, dc: isize, extend_sel: bool) {
        if extend_sel {
            if self.sel_anchor.is_none() {
                self.sel_anchor = Some(self.cursor);
            }
        } else if let Some((start, end)) = self.selected_range() {
            self.sel_anchor = None;
            if dr == 0 && dc < 0 {
                self.cursor = start;
                return;
            } else if dr == 0 && dc > 0 {
                self.cursor = end;
                return;
            }
        } else {
            self.sel_anchor = None;
        }

        let (mut r, mut c) = self.cursor;
        if dr != 0 && !self.visual_rows.is_empty() {
            let cur_vi = self.cursor_visual_row();
            let cur_vr = &self.visual_rows[cur_vi];
            let rel_bytes = c.saturating_sub(cur_vr.start_byte);
            let target_vi = if dr < 0 {
                cur_vi.saturating_sub((-dr) as usize)
            } else {
                (cur_vi + dr as usize).min(self.visual_rows.len().saturating_sub(1))
            };
            let target_vr = &self.visual_rows[target_vi];
            r = target_vr.line_idx;
            c = (target_vr.start_byte + rel_bytes).min(target_vr.end_byte);
        } else if dr < 0 {
            r = r.saturating_sub((-dr) as usize);
            c = c.min(self.lines[r].len());
        } else if dr > 0 {
            r = (r + dr as usize).min(self.lines.len().saturating_sub(1));
            c = c.min(self.lines[r].len());
        }

        if dc < 0 {
            for _ in 0..(-dc) {
                if c > 0 {
                    c -= 1;
                    while c > 0 && !self.lines[r].is_char_boundary(c) {
                        c -= 1;
                    }
                } else if r > 0 {
                    r -= 1;
                    c = self.lines[r].len();
                }
            }
        } else if dc > 0 {
            for _ in 0..dc {
                if c < self.lines[r].len() {
                    c += 1;
                    while c < self.lines[r].len() && !self.lines[r].is_char_boundary(c) {
                        c += 1;
                    }
                } else if r + 1 < self.lines.len() {
                    r += 1;
                    c = 0;
                }
            }
        }
        self.cursor = (r, c);
        self.clamp_cursor();
    }

    pub(super) fn move_home(&mut self, extend_sel: bool) {
        if extend_sel && self.sel_anchor.is_none() {
            self.sel_anchor = Some(self.cursor);
        } else if !extend_sel {
            self.sel_anchor = None;
        }
        if !self.visual_rows.is_empty() {
            let vi = self.cursor_visual_row();
            let sb = self.visual_rows[vi].start_byte;
            if self.cursor.1 != sb {
                self.cursor.1 = sb;
                return;
            }
        }
        self.cursor.1 = 0;
    }

    pub(super) fn move_end(&mut self, extend_sel: bool) {
        if extend_sel && self.sel_anchor.is_none() {
            self.sel_anchor = Some(self.cursor);
        } else if !extend_sel {
            self.sel_anchor = None;
        }
        if !self.visual_rows.is_empty() {
            let vi = self.cursor_visual_row();
            let eb = self.visual_rows[vi].end_byte;
            if self.cursor.1 != eb {
                self.cursor.1 = eb;
                return;
            }
        }
        self.cursor.1 = self.lines[self.cursor.0].len();
    }

    pub(super) fn ensure_cursor_visible(&mut self, ew: u32, view_h: f32, sf: f64, zoom: f32) {
        let m = self.ensure_wrapped(ew, sf, zoom);
        let vi = self.cursor_visual_row();
        let cy = m.pad_y + vi as f32 * m.line_h;
        if cy < self.scroll_y + m.pad_y {
            self.scroll_y = (cy - m.pad_y).max(0.0);
        } else if cy + m.line_h > self.scroll_y + view_h - m.pad_y {
            self.scroll_y = (cy + m.line_h - view_h + m.pad_y).max(0.0);
        }
        let max_s = self.max_scroll_y(ew, view_h, sf, zoom);
        self.scroll_y = self.scroll_y.clamp(0.0, max_s);
    }

    pub(super) fn max_scroll_y(&mut self, ew: u32, view_h: f32, sf: f64, zoom: f32) -> f32 {
        let m = self.ensure_wrapped(ew, sf, zoom);
        let total_h = m.pad_y * 2.0 + self.visual_rows.len() as f32 * m.line_h;
        (total_h - view_h).max(0.0)
    }

    /// Returns `(track_x, track_y, track_w, track_h, thumb_y, thumb_h)` when the editor
    /// content exceeds the visible height.
    pub(super) fn scrollbar_geometry(
        &mut self,
        ex: i32,
        ey: i32,
        ew: u32,
        eh: u32,
        sf: f64,
        zoom: f32,
    ) -> Option<(i32, i32, u32, u32, i32, u32)> {
        let m = self.ensure_wrapped(ew, sf, zoom);
        let total_h = m.pad_y * 2.0 + self.visual_rows.len() as f32 * m.line_h;
        let max_scroll = (total_h - eh as f32).max(0.0);
        if max_scroll <= 1.0 || eh < 40 {
            return None;
        }
        let track_w = (7.0 * m.s).round().clamp(6.0, 12.0) as u32;
        let pad_r = (4.0 * m.s).round() as i32;
        let pad_v = (6.0 * m.s).round() as i32;
        let track_x = ex + ew as i32 - track_w as i32 - pad_r;
        let track_y = ey + pad_v;
        let track_h = eh.saturating_sub((pad_v * 2) as u32);
        if track_h < 24 {
            return None;
        }
        let visible_ratio = (eh as f32 / total_h).clamp(0.08, 0.92);
        let fraction = (self.scroll_y / max_scroll).clamp(0.0, 1.0);
        let thumb_h = ((track_h as f32 * visible_ratio).round() as u32).clamp(24, track_h);
        let thumb_y = track_y + ((track_h.saturating_sub(thumb_h)) as f32 * fraction).round() as i32;
        Some((track_x, track_y, track_w, track_h, thumb_y, thumb_h))
    }

    pub(super) fn hit_scrollbar(
        &mut self,
        px: i32,
        py: i32,
        ex: i32,
        ey: i32,
        ew: u32,
        eh: u32,
        sf: f64,
        zoom: f32,
    ) -> bool {
        let Some((tx, ty, tw, th, _, _)) = self.scrollbar_geometry(ex, ey, ew, eh, sf, zoom) else {
            return false;
        };
        px >= tx - 6 && px <= tx + tw as i32 + 4 && py >= ty && py <= ty + th as i32
    }

    pub(super) fn scroll_to_scrollbar_y(
        &mut self,
        py: f64,
        ex: i32,
        ey: i32,
        ew: u32,
        eh: u32,
        sf: f64,
        zoom: f32,
    ) {
        let max_scroll = self.max_scroll_y(ew, eh as f32, sf, zoom);
        if max_scroll <= 1.0 {
            self.scroll_y = 0.0;
            return;
        }
        let Some((_, track_y, _, track_h, _, thumb_h)) = self.scrollbar_geometry(ex, ey, ew, eh, sf, zoom) else {
            return;
        };
        let usable = (track_h.saturating_sub(thumb_h)).max(1) as f64;
        let rel = (py - track_y as f64 - thumb_h as f64 * 0.5).clamp(0.0, usable);
        let frac = (rel / usable) as f32;
        self.scroll_y = (frac * max_scroll).clamp(0.0, max_scroll);
    }

    pub(super) fn hit_pos(
        &mut self,
        px: i32,
        py: i32,
        ex: i32,
        ey: i32,
        ew: u32,
        sf: f64,
        zoom: f32,
    ) -> (usize, usize) {
        let m = self.ensure_wrapped(ew, sf, zoom);
        let rel_y = (py - ey) as f32 + self.scroll_y - m.pad_y;
        let vi = ((rel_y / m.line_h).floor().max(0.0) as usize)
            .min(self.visual_rows.len().saturating_sub(1));
        let vr = self.visual_rows[vi].clone();
        let text_x = ex + m.gutter_w as i32 + m.text_pad_l;
        let target_dx = (px - text_x).max(0) as f32;

        let seg = &self.lines[vr.line_idx][vr.start_byte..vr.end_byte];
        let mut best_rel = seg.len();
        let mut prev_w = 0.0f32;
        for (idx, _) in seg.char_indices() {
            let w = measure_text(&seg[..idx], m.font_sz, false);
            if w >= target_dx {
                if (target_dx - prev_w) < (w - target_dx) {
                    best_rel = seg[..idx].char_indices().last().map(|(i, _)| i).unwrap_or(0);
                } else {
                    best_rel = idx;
                }
                return (vr.line_idx, vr.start_byte + best_rel);
            }
            prev_w = w;
        }
        (vr.line_idx, vr.start_byte + best_rel)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        ex: i32,
        ey: i32,
        ew: u32,
        eh: u32,
        page_bg: u32,
        theme: &UiTheme,
        sf: f64,
        zoom: f32,
        syntax: EditorSyntax,
    ) {
        if ew == 0 || eh == 0 {
            return;
        }
        let m = self.ensure_wrapped(ew, sf, zoom);
        let max_s = self.max_scroll_y(ew, eh as f32, sf, zoom);
        self.scroll_y = self.scroll_y.clamp(0.0, max_s);

        fill_rect_clipped(buffer, buf_w, buf_h, ex, ey, ew, eh, page_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, ex, ey, m.gutter_w, eh, theme.surface_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, ex + m.gutter_w as i32 - 1, ey, 1, eh, theme.divider);

        let text_x = ex + m.gutter_w as i32 + m.text_pad_l;
        let text_max_x = (ex + ew as i32 - m.sb_reserve as i32 - (4.0 * m.s).round() as i32).max(text_x + 16);
        let clip_bot = ey + eh as i32;
        let sel = self.selected_range();
        let pal = syntax_palette_for_bg(page_bg, theme);
        let cursor_vi = self.cursor_visual_row();

        let first_vi = ((self.scroll_y - m.pad_y) / m.line_h).floor().max(0.0) as usize;
        let vis_count = ((eh as f32 / m.line_h).ceil() as usize) + 2;
        let last_vi = (first_vi + vis_count).min(self.visual_rows.len());

        // Cache syntax tokens for the current logical line across its wrapped visual rows
        let mut cached_line_idx = usize::MAX;
        let mut cached_tokens: Vec<(usize, usize, u32)> = Vec::new();

        for vi in first_vi..last_vi {
            let vr = &self.visual_rows[vi];
            let ly = ey + (m.pad_y + vi as f32 * m.line_h - self.scroll_y).round() as i32;
            if ly < ey || ly + m.line_h.round() as i32 > clip_bot {
                continue;
            }

            if vr.is_first_of_line {
                let num = format!("{}", vr.line_idx + 1);
                let nw = measure_text(&num, m.font_sz * 0.84, false);
                let nx = ex + m.gutter_w as i32 - nw.round() as i32 - (8.0 * m.s).round() as i32;
                draw_text_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    nx,
                    ly + 1,
                    ex + m.gutter_w as i32 - 4,
                    &num,
                    m.font_sz * 0.84,
                    theme.text_muted,
                    false,
                );
            }

            let full_line = &self.lines[vr.line_idx];
            let seg = &full_line[vr.start_byte..vr.end_byte];

            // Selection highlight clipped to this visual row
            if let Some(((r0, c0), (r1, c1))) = sel {
                if vr.line_idx >= r0 && vr.line_idx <= r1 {
                    let line_sc = if vr.line_idx == r0 { c0 } else { 0 };
                    let line_ec = if vr.line_idx == r1 { c1 } else { full_line.len() };
                    let seg_sc = line_sc.clamp(vr.start_byte, vr.end_byte);
                    let seg_ec = line_ec.clamp(vr.start_byte, vr.end_byte);
                    let intersects = (seg_ec > seg_sc)
                        || (vr.line_idx < r1 && vr.end_byte == full_line.len() && line_sc <= vr.end_byte);
                    if intersects {
                        let rel_sc = seg_sc - vr.start_byte;
                        let rel_ec = seg_ec - vr.start_byte;
                        let sx = text_x + measure_text(&seg[..rel_sc], m.font_sz, false).round() as i32;
                        let sw = if rel_ec > rel_sc {
                            measure_text(&seg[rel_sc..rel_ec], m.font_sz, false).round().max(4.0) as u32
                        } else {
                            (6.0 * m.s).round() as u32
                        };
                        let clamped_w = sw.min((text_max_x - sx).max(0) as u32);
                        if clamped_w > 0 {
                            fill_rect_clipped(buffer, buf_w, buf_h, sx, ly, clamped_w, m.line_h.round() as u32, theme.accent_soft_bg);
                        }
                    }
                }
            }

            // Syntax-highlighted drawing of this visual row
            if vr.line_idx != cached_line_idx {
                cached_line_idx = vr.line_idx;
                cached_tokens = match syntax {
                    EditorSyntax::LaTeX => highlight_latex_line(full_line, pal),
                    EditorSyntax::Markdown => highlight_markdown_line(full_line, pal),
                };
            }

            for &(ts, te, col) in &cached_tokens {
                if te <= vr.start_byte || ts >= vr.end_byte {
                    continue;
                }
                let cs = ts.max(vr.start_byte);
                let ce = te.min(vr.end_byte);
                if cs >= ce {
                    continue;
                }
                let rel_s = cs - vr.start_byte;
                let tx = text_x + measure_text(&seg[..rel_s], m.font_sz, false).round() as i32;
                draw_text_clipped(
                    buffer,
                    buf_w,
                    buf_h,
                    tx,
                    ly,
                    text_max_x,
                    &full_line[cs..ce],
                    m.font_sz,
                    col,
                    false,
                );
            }

            // Caret on active visual row
            if vi == cursor_vi {
                let rel_c = self.cursor.1.clamp(vr.start_byte, vr.end_byte) - vr.start_byte;
                let cx = text_x + measure_text(&seg[..rel_c], m.font_sz, false).round() as i32;
                if cx >= text_x && cx <= text_max_x {
                    fill_rect_clipped(buffer, buf_w, buf_h, cx, ly, 2, m.line_h.round() as u32, theme.accent);
                }
            }
        }

        // Built-in vertical scrollbar on the right edge of the editor pane
        if let Some((tx, ty, tw, th, thumb_y, thumb_h)) = self.scrollbar_geometry(ex, ey, ew, eh, sf, zoom) {
            let rad = (tw as f32) * 0.5;
            let tr = theme.scrollbar_track;
            let tr_col = (tr.0 << 16) | (tr.1 << 8) | tr.2;
            fill_rounded_rect_alpha(buffer, buf_w, buf_h, tx, ty, tw, th, rad, tr_col, tr.3);
            let thm = if self.scrollbar_drag {
                theme.scrollbar_thumb_hover
            } else {
                theme.scrollbar_thumb
            };
            let th_col = (thm.0 << 16) | (thm.1 << 8) | thm.2;
            fill_rounded_rect_alpha(buffer, buf_w, buf_h, tx, thumb_y, tw, thumb_h, rad, th_col, thm.3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_editor_insert_delete_selection_and_undo_redo() {
        let mut ed = TextEditorState::from_text("Hello world");
        assert_eq!(ed.to_text(), "Hello world");

        ed.cursor = (0, 5);
        ed.insert_str(" Rust\nLine 2");
        assert_eq!(ed.to_text(), "Hello Rust\nLine 2 world");
        assert_eq!(ed.cursor, (1, 6));

        ed.backspace();
        assert_eq!(ed.to_text(), "Hello Rust\nLine  world");

        ed.select_all();
        assert_eq!(ed.selected_text().as_deref(), Some("Hello Rust\nLine  world"));
        ed.delete_forward();
        assert_eq!(ed.to_text(), "");

        assert!(ed.undo());
        assert_eq!(ed.to_text(), "Hello Rust\nLine  world");
        assert!(ed.redo());
        assert_eq!(ed.to_text(), "");
    }

    #[test]
    fn test_text_editor_word_wrap_zoom_scrollbar_and_latex_syntax() {
        let long_line = "This is a very long LaTeX paragraph with \\section{Intro} and $E = mc^2$ that must wrap across multiple visual rows cleanly.";
        let mut ed = TextEditorState::from_text(long_line);

        // Wrapping at a narrow width produces multiple visual rows covering every byte
        let m1 = ed.ensure_wrapped(260, 1.0, 1.0);
        assert!(ed.visual_rows.len() >= 3, "expected >=3 wrapped rows, got {}", ed.visual_rows.len());
        assert_eq!(ed.visual_rows.first().unwrap().start_byte, 0);
        assert_eq!(ed.visual_rows.last().unwrap().end_byte, long_line.len());

        // Zooming increases font_sz and line_h
        let m2 = ed.ensure_wrapped(260, 1.0, 1.8);
        assert!(m2.font_sz > m1.font_sz);
        assert!(m2.line_h > m1.line_h);

        // Scrollbar appears when total height exceeds viewport height
        assert!(ed.scrollbar_geometry(0, 0, 260, 50, 1.0, 1.8).is_some());

        // LaTeX syntax highlighting assigns distinct colors to commands, braces, env names, math, and comments
        let pal = SyntaxPalette {
            plain: 1,
            comment: 2,
            command: 3,
            env_name: 4,
            brace: 5,
            math: 6,
        };
        let tex = "\\begin{equation} \\frac{a}{b} = $x^2$ % note";
        let toks = highlight_latex_line(tex, pal);
        assert!(toks.iter().any(|&(s, e, c)| &tex[s..e] == "\\begin" && c == pal.command));
        assert!(toks.iter().any(|&(s, e, c)| &tex[s..e] == "equation" && c == pal.env_name));
        assert!(toks.iter().any(|&(s, e, c)| &tex[s..e] == "\\frac" && c == pal.command));
        assert!(toks.iter().any(|&(s, e, c)| &tex[s..e] == "{" && c == pal.brace));
        assert!(toks.iter().any(|&(_, _, c)| c == pal.math));
        assert!(toks.iter().any(|&(s, e, c)| tex[s..e].starts_with('%') && c == pal.comment));
    }

    #[test]
    fn test_text_editor_power_ups() {
        let mut ed = TextEditorState::from_text("  - item one");
        ed.cursor = (0, "  - item one".len());
        ed.insert_newline_auto_indent();
        assert_eq!(ed.to_text(), "  - item one\n  - ");

        // Empty bullet cleared on Enter
        ed.insert_newline_auto_indent();
        assert_eq!(ed.to_text(), "  - item one\n  ");

        // Auto-pair and wrap selection
        let mut ed2 = TextEditorState::from_text("alpha");
        ed2.select_all();
        ed2.wrap_selection("**", "**");
        assert_eq!(ed2.to_text(), "**alpha**");

        ed2.duplicate_line();
        assert_eq!(ed2.to_text(), "**alpha**\n**alpha**");

        ed2.lines[1] = "beta".to_string();
        ed2.cursor = (1, 0);
        ed2.move_line(-1);
        assert_eq!(ed2.to_text(), "beta\n**alpha**");

        ed2.toggle_comment(EditorSyntax::LaTeX);
        assert_eq!(ed2.lines[0], "% beta");
        ed2.toggle_comment(EditorSyntax::LaTeX);
        assert_eq!(ed2.lines[0], "beta");

        ed2.indent_or_outdent(false);
        assert_eq!(ed2.lines[0], "  beta");
        ed2.indent_or_outdent(true);
        assert_eq!(ed2.lines[0], "beta");
    }
}
