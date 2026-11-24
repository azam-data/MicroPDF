//! Lightweight pure-Rust multi-line text editor for `.md` / `.tex` tabs.
//!
//! Zero GPU, zero GUI toolkit: renders directly into `softbuffer` with line numbers,
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

#[derive(Clone)]
struct EditorSnap {
    lines: Vec<String>,
    cursor: (usize, usize),
    sel_anchor: Option<(usize, usize)>,
}

pub(super) struct TextEditorState {
    pub(super) lines: Vec<String>,
    /// `(row, byte_col)` always aligned to a valid UTF-8 char boundary.
    pub(super) cursor: (usize, usize),
    pub(super) sel_anchor: Option<(usize, usize)>,
    pub(super) scroll_y: f32,
    pub(super) dragging: bool,
    undo: VecDeque<EditorSnap>,
    redo: VecDeque<EditorSnap>,
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
            undo: VecDeque::with_capacity(MAX_EDITOR_UNDO),
            redo: VecDeque::with_capacity(MAX_EDITOR_UNDO),
        }
    }

    pub(super) fn to_text(&self) -> String {
        self.lines.join("\n")
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
        } else if r > 0 {
            let cur_line = self.lines.remove(r);
            let prev_len = self.lines[r - 1].len();
            self.lines[r - 1].push_str(&cur_line);
            self.cursor = (r - 1, prev_len);
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
        } else if r + 1 < self.lines.len() {
            let next_line = self.lines.remove(r + 1);
            self.lines[r].push_str(&next_line);
        }
    }

    pub(super) fn select_all(&mut self) {
        self.sel_anchor = Some((0, 0));
        let last_r = self.lines.len().saturating_sub(1);
        let last_c = self.lines[last_r].len();
        self.cursor = (last_r, last_c);
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
        if dr < 0 {
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
        self.cursor.1 = 0;
    }

    pub(super) fn move_end(&mut self, extend_sel: bool) {
        if extend_sel && self.sel_anchor.is_none() {
            self.sel_anchor = Some(self.cursor);
        } else if !extend_sel {
            self.sel_anchor = None;
        }
        self.cursor.1 = self.lines[self.cursor.0].len();
    }

    pub(super) fn ensure_cursor_visible(&mut self, view_h: f32, sf: f64) {
        let s = sf as f32;
        let line_h = (19.5 * s).max(14.0);
        let pad_y = 12.0 * s;
        let cy = pad_y + self.cursor.0 as f32 * line_h;
        if cy < self.scroll_y + pad_y {
            self.scroll_y = (cy - pad_y).max(0.0);
        } else if cy + line_h > self.scroll_y + view_h - pad_y {
            self.scroll_y = (cy + line_h - view_h + pad_y).max(0.0);
        }
    }

    pub(super) fn max_scroll_y(&self, view_h: f32, sf: f64) -> f32 {
        let s = sf as f32;
        let line_h = (19.5 * s).max(14.0);
        let total_h = 24.0 * s + self.lines.len() as f32 * line_h;
        (total_h - view_h).max(0.0)
    }

    pub(super) fn hit_pos(&self, px: i32, py: i32, ex: i32, ey: i32, sf: f64) -> (usize, usize) {
        let s = sf as f32;
        let font_sz = (13.0 * s).clamp(10.0, 20.0);
        let line_h = (19.5 * s).max(14.0);
        let gutter_w = (44.0 * s).round() as i32;
        let pad_y = 12.0 * s;
        let rel_y = (py - ey) as f32 + self.scroll_y - pad_y;
        let row = ((rel_y / line_h).floor().max(0.0) as usize).min(self.lines.len().saturating_sub(1));
        let text_x = ex + gutter_w + (10.0 * s).round() as i32;
        let target_dx = (px - text_x).max(0) as f32;

        let line = &self.lines[row];
        let mut best_c = line.len();
        let mut prev_w = 0.0f32;
        for (idx, _) in line.char_indices() {
            let w = measure_text(&line[..idx], font_sz, false);
            if w >= target_dx {
                if (target_dx - prev_w) < (w - target_dx) {
                    best_c = line[..idx].char_indices().last().map(|(i, _)| i).unwrap_or(0);
                } else {
                    best_c = idx;
                }
                return (row, best_c);
            }
            prev_w = w;
        }
        (row, best_c)
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
    ) {
        if ew == 0 || eh == 0 {
            return;
        }
        let s = sf as f32;
        let font_sz = (13.0 * s).clamp(10.0, 20.0);
        let line_h = (19.5 * s).max(14.0);
        let gutter_w = (44.0 * s).round() as u32;
        let pad_y = 12.0 * s;
        let max_s = self.max_scroll_y(eh as f32, sf);
        self.scroll_y = self.scroll_y.clamp(0.0, max_s);

        fill_rect_clipped(buffer, buf_w, buf_h, ex, ey, ew, eh, page_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, ex, ey, gutter_w, eh, theme.surface_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, ex + gutter_w as i32 - 1, ey, 1, eh, theme.divider);

        let text_x = ex + gutter_w as i32 + (10.0 * s).round() as i32;
        let text_max_x = ex + ew as i32 - (8.0 * s).round() as i32;
        let clip_bot = ey + eh as i32;
        let sel = self.selected_range();

        let first_row = ((self.scroll_y - pad_y) / line_h).floor().max(0.0) as usize;
        let vis_rows = ((eh as f32 / line_h).ceil() as usize) + 2;
        let last_row = (first_row + vis_rows).min(self.lines.len());

        for r in first_row..last_row {
            let ly = ey + (pad_y + r as f32 * line_h - self.scroll_y).round() as i32;
            if ly < ey || ly + line_h.round() as i32 > clip_bot {
                continue;
            }
            let num = format!("{}", r + 1);
            let nw = measure_text(&num, font_sz * 0.85, false);
            let nx = ex + gutter_w as i32 - nw.round() as i32 - (8.0 * s).round() as i32;
            draw_text_clipped(buffer, buf_w, buf_h, nx, ly + 1, ex + gutter_w as i32 - 4, &num, font_sz * 0.85, theme.text_muted, false);

            let line = &self.lines[r];
            if let Some(((r0, c0), (r1, c1))) = sel {
                if r >= r0 && r <= r1 {
                    let sc = if r == r0 { c0 } else { 0 };
                    let ec = if r == r1 { c1 } else { line.len() };
                    let sx = text_x + measure_text(&line[..sc], font_sz, false).round() as i32;
                    let sw = if ec > sc {
                        measure_text(&line[sc..ec], font_sz, false).round().max(4.0) as u32
                    } else {
                        (6.0 * s).round() as u32
                    };
                    fill_rect_clipped(buffer, buf_w, buf_h, sx, ly, sw.min((text_max_x - sx).max(0) as u32), line_h.round() as u32, theme.accent_soft_bg);
                }
            }

            let is_heading = line.trim_start().starts_with('#');
            let col = if is_heading { theme.accent_soft_fg } else { theme.text_primary };
            draw_text_clipped(buffer, buf_w, buf_h, text_x, ly, text_max_x, line, font_sz, col, is_heading);

            if r == self.cursor.0 {
                let cx = text_x + measure_text(&line[..self.cursor.1], font_sz, is_heading).round() as i32;
                if cx >= text_x && cx < text_max_x {
                    fill_rect_clipped(buffer, buf_w, buf_h, cx, ly, 2, line_h.round() as u32, theme.accent);
                }
            }
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
}
