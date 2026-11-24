//! Shared compact, scrollable System Font Dropdown overlay for modal dialogs
//! (Header & Footer, Page Numbering, Watermark).
//!
//! Zero heap allocation when closed: reads directly from `crate::fonts::global_system_fonts()`.

use super::theme::UiTheme;
use super::ui_draw::*;

pub(super) const FONT_DROPDOWN_MAX_ROWS: usize = 8;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FontDropdownState {
    pub(super) open: bool,
    pub(super) scroll_offset: usize,
    pub(super) hover_idx: Option<usize>,
}

impl FontDropdownState {
    pub(super) fn toggle(&mut self, current_font: &str) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.hover_idx = None;
            let catalog = crate::fonts::global_system_fonts();
            let total = catalog.len();
            let vis = FONT_DROPDOWN_MAX_ROWS.min(total).max(1);
            let idx = catalog
                .iter()
                .position(|e| e.display_name.eq_ignore_ascii_case(current_font))
                .unwrap_or(0);
            let max_off = total.saturating_sub(vis);
            self.scroll_offset = idx.saturating_sub(vis / 2).min(max_off);
        }
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.hover_idx = None;
    }

    pub(super) fn scroll_by(&mut self, delta_rows: isize) {
        let total = crate::fonts::global_system_fonts().len();
        let vis = FONT_DROPDOWN_MAX_ROWS.min(total).max(1);
        let max_off = total.saturating_sub(vis) as isize;
        let next = (self.scroll_offset as isize + delta_rows).clamp(0, max_off);
        self.scroll_offset = next as usize;
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum FontDropdownHit {
    Row(usize),
    TrackPageUp,
    TrackPageDown,
    InsidePopup,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct FontPopupGeom {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) pad: i32,
    pub(super) row_h: u32,
    pub(super) visible_rows: usize,
    pub(super) total_items: usize,
    pub(super) has_scrollbar: bool,
    pub(super) track_x: i32,
    pub(super) track_w: u32,
    pub(super) thumb_y: i32,
    pub(super) thumb_h: u32,
}

pub(super) fn compute_font_popup_geom(
    anchor_x: i32,
    anchor_y: i32,
    anchor_w: u32,
    anchor_h: u32,
    buf_w: u32,
    buf_h: u32,
    sf: f64,
    scroll_offset: usize,
) -> FontPopupGeom {
    let s = sf as f32;
    let total_items = crate::fonts::global_system_fonts().len();
    let visible_rows = FONT_DROPDOWN_MAX_ROWS.min(total_items).max(1);
    let row_h = (23.0 * s).round() as u32;
    let pad = (4.0 * s).round() as i32;
    let popup_h = (visible_rows as u32 * row_h) + (pad * 2) as u32;
    let min_w = (230.0 * s).round() as u32;
    let popup_w = anchor_w.max(min_w).min(buf_w.saturating_sub(16));

    let x = anchor_x
        .min((buf_w as i32 - popup_w as i32 - 8).max(8))
        .max(8);

    let below_y = anchor_y + anchor_h as i32 + (3.0 * s).round() as i32;
    let y = if below_y + popup_h as i32 <= buf_h as i32 - 8 {
        below_y
    } else {
        (anchor_y - (3.0 * s).round() as i32 - popup_h as i32).max(8)
    };

    let has_scrollbar = total_items > visible_rows;
    let track_w = if has_scrollbar { (8.0 * s).round() as u32 } else { 0 };
    let track_x = x + popup_w as i32 - pad - track_w as i32;
    let list_h = (visible_rows as u32 * row_h).max(1);
    let max_off = total_items.saturating_sub(visible_rows).max(1);
    let thumb_h = if has_scrollbar {
        ((list_h as f32 * visible_rows as f32 / total_items as f32).round() as u32)
            .max((18.0 * s).round() as u32)
            .min(list_h)
    } else {
        list_h
    };
    let thumb_travel = list_h.saturating_sub(thumb_h);
    let thumb_y = y
        + pad
        + ((scroll_offset.min(max_off) as f32 / max_off as f32) * thumb_travel as f32).round()
            as i32;

    FontPopupGeom {
        x,
        y,
        w: popup_w,
        h: popup_h,
        pad,
        row_h,
        visible_rows,
        total_items,
        has_scrollbar,
        track_x,
        track_w,
        thumb_y,
        thumb_h,
    }
}

pub(super) fn draw_font_dropdown_button(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    font_name: &str,
    prefix: Option<&str>,
    open: bool,
    hovered: bool,
    theme: &UiTheme,
    s: f32,
) {
    let rad = 5.0 * s;
    let bg = if hovered && !open {
        theme.hover_bg
    } else {
        theme.input_bg
    };
    let border = if open {
        theme.accent
    } else if hovered {
        theme.input_border
    } else {
        theme.border_subtle
    };
    fill_rounded_rect(buffer, buf_w, buf_h, x, y, w, h, rad, bg);
    stroke_rounded_rect(buffer, buf_w, buf_h, x, y, w, h, rad, border);

    let body_sz = 11.5 * s;
    let chev_w = (18.0 * s).round() as i32;
    let text_max_w = (w as i32 - chev_w - 10).max(20) as f32;
    let label = match prefix {
        Some(p) => format!("{p}{font_name}"),
        None => font_name.to_string(),
    };
    let clipped = truncate_ellipsis(&label, text_max_w, body_sz, false);
    draw_text_vcenter(
        buffer,
        buf_w,
        buf_h,
        x + 7,
        y,
        h,
        x + w as i32 - chev_w,
        &clipped,
        body_sz,
        theme.text_primary,
        false,
    );

    // Draw crisp chevron (down when closed, up when open)
    let cx = (x + w as i32 - chev_w / 2 - 2) as f32;
    let cy = (y + h as i32 / 2) as f32;
    let arm = 3.5 * s;
    let dy = 2.0 * s;
    let chev_col = if open { theme.accent } else { theme.text_muted };
    if open {
        draw_line_aa(buffer, buf_w, buf_h, cx - arm, cy + dy, cx, cy - dy, 1.4 * s, chev_col);
        draw_line_aa(buffer, buf_w, buf_h, cx, cy - dy, cx + arm, cy + dy, 1.4 * s, chev_col);
    } else {
        draw_line_aa(buffer, buf_w, buf_h, cx - arm, cy - dy, cx, cy + dy, 1.4 * s, chev_col);
        draw_line_aa(buffer, buf_w, buf_h, cx, cy + dy, cx + arm, cy - dy, 1.4 * s, chev_col);
    }
}

pub(super) fn draw_font_dropdown_popup(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    anchor_x: i32,
    anchor_y: i32,
    anchor_w: u32,
    anchor_h: u32,
    sf: f64,
    state: &FontDropdownState,
    current_font: &str,
    theme: &UiTheme,
) {
    if !state.open {
        return;
    }
    let s = sf as f32;
    let g = compute_font_popup_geom(
        anchor_x,
        anchor_y,
        anchor_w,
        anchor_h,
        buf_w,
        buf_h,
        sf,
        state.scroll_offset,
    );
    let rad = 6.0 * s;
    draw_soft_shadow(
        buffer,
        buf_w,
        buf_h,
        g.x,
        g.y,
        g.w,
        g.h,
        rad,
        (10.0 * s).round() as i32,
        (4.0 * s).round() as i32,
        theme.shadow_rgb,
        theme.shadow_max_alpha,
    );
    fill_rounded_rect(buffer, buf_w, buf_h, g.x, g.y, g.w, g.h, rad, theme.surface_elevated);
    stroke_rounded_rect(buffer, buf_w, buf_h, g.x, g.y, g.w, g.h, rad, theme.input_border);

    let catalog = crate::fonts::global_system_fonts();
    let row_x = g.x + g.pad;
    let row_w = (g.w as i32 - g.pad * 2 - if g.has_scrollbar { g.track_w as i32 + 4 } else { 0 }).max(40) as u32;
    let font_sz = 11.5 * s;

    for vis_i in 0..g.visible_rows {
        let idx = state.scroll_offset + vis_i;
        if idx >= catalog.len() {
            break;
        }
        let ry = g.y + g.pad + (vis_i as u32 * g.row_h) as i32;
        let entry = &catalog[idx];
        let is_selected = entry.display_name.eq_ignore_ascii_case(current_font);
        let is_hovered = state.hover_idx == Some(idx);

        if is_selected {
            fill_rounded_rect(buffer, buf_w, buf_h, row_x, ry + 1, row_w, g.row_h.saturating_sub(2), 4.0 * s, theme.accent);
        } else if is_hovered {
            fill_rounded_rect(buffer, buf_w, buf_h, row_x, ry + 1, row_w, g.row_h.saturating_sub(2), 4.0 * s, theme.hover_bg);
        }

        let text_col = if is_selected { theme.text_on_accent } else { theme.text_primary };
        let clipped = truncate_ellipsis(&entry.display_name, (row_w.saturating_sub(14)) as f32, font_sz, is_selected);
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            row_x + 7,
            ry,
            g.row_h,
            row_x + row_w as i32 - 5,
            &clipped,
            font_sz,
            text_col,
            is_selected,
        );
    }

    if g.has_scrollbar {
        let list_h = (g.visible_rows as u32 * g.row_h).max(1);
        fill_rounded_rect(
            buffer,
            buf_w,
            buf_h,
            g.track_x + 1,
            g.y + g.pad,
            g.track_w.saturating_sub(2).max(3),
            list_h,
            3.0 * s,
            theme.input_bg,
        );
        fill_rounded_rect(
            buffer,
            buf_w,
            buf_h,
            g.track_x + 1,
            g.thumb_y,
            g.track_w.saturating_sub(2).max(3),
            g.thumb_h,
            3.0 * s,
            theme.text_muted,
        );
    }
}

pub(super) fn hit_test_font_dropdown_popup(
    px: i32,
    py: i32,
    anchor_x: i32,
    anchor_y: i32,
    anchor_w: u32,
    anchor_h: u32,
    buf_w: u32,
    buf_h: u32,
    sf: f64,
    state: &FontDropdownState,
) -> Option<FontDropdownHit> {
    if !state.open {
        return None;
    }
    let g = compute_font_popup_geom(
        anchor_x,
        anchor_y,
        anchor_w,
        anchor_h,
        buf_w,
        buf_h,
        sf,
        state.scroll_offset,
    );
    if px < g.x || px >= g.x + g.w as i32 || py < g.y || py >= g.y + g.h as i32 {
        return None;
    }

    if g.has_scrollbar && px >= g.track_x - 2 {
        if py < g.thumb_y {
            return Some(FontDropdownHit::TrackPageUp);
        } else if py >= g.thumb_y + g.thumb_h as i32 {
            return Some(FontDropdownHit::TrackPageDown);
        } else {
            return Some(FontDropdownHit::InsidePopup);
        }
    }

    let rel_y = py - (g.y + g.pad);
    if rel_y >= 0 {
        let vis_i = (rel_y as u32 / g.row_h) as usize;
        if vis_i < g.visible_rows {
            let idx = state.scroll_offset + vis_i;
            if idx < g.total_items {
                return Some(FontDropdownHit::Row(idx));
            }
        }
    }
    Some(FontDropdownHit::InsidePopup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_dropdown_state_toggle_and_scroll_clamping() {
        let mut dd = FontDropdownState::default();
        assert!(!dd.open);
        assert_eq!(dd.scroll_offset, 0);

        dd.toggle("Helvetica (Built-in)");
        assert!(dd.open);

        let total = crate::fonts::global_system_fonts().len();
        let vis = FONT_DROPDOWN_MAX_ROWS.min(total).max(1);
        let max_off = total.saturating_sub(vis);

        dd.scroll_by(10_000);
        assert_eq!(dd.scroll_offset, max_off);

        dd.scroll_by(-10_000);
        assert_eq!(dd.scroll_offset, 0);

        dd.close();
        assert!(!dd.open);
        assert!(dd.hover_idx.is_none());
    }
}
