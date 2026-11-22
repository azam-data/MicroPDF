//! Milestone 3: Lightweight Strictly Matrix-Based Page Deskew (`Page -> Deskew Page(s)...`).
//!
//! Compact 720p-friendly control bar with angle input/slider (`-15.0°..+15.0°`, `0.1°` step),
//! Draw Guideline mode (crosshair drag or two-click with auto H/V axis snap), optional
//! alignment grid overlay, scope selector (Current Page or Page Range), and real-time CPU
//! viewport preview. Zero new dependencies, zero rasterization on Apply, 0 bytes RAM when closed.

use crate::engine::{compute_guideline_deskew_angle, DeskewAxis, PdfEngine};
use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum DeskewScope {
    CurrentPage,
    PageRange,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum DeskewTextFocus {
    None,
    Angle,
    Range,
}

#[derive(Clone, Debug)]
pub(super) struct DeskewBarState {
    pub(super) angle_deg: f32,
    pub(super) angle_input: String,
    pub(super) scope: DeskewScope,
    pub(super) range_input: String,
    pub(super) focus: DeskewTextFocus,
    pub(super) draw_guideline: bool,
    pub(super) show_grid: bool,
    pub(super) dragging_slider: bool,
    /// Two-click / drag guideline start & end in window physical pixels.
    pub(super) guide_start: Option<(f64, f64)>,
    pub(super) guide_end: Option<(f64, f64)>,
    pub(super) guide_dragging: bool,
    pub(super) last_axis: Option<DeskewAxis>,
    /// Cached rotated preview bitmap `(page_idx, angle_tenths, w, h, pixels)` — freed when closed.
    pub(super) preview_cache: Option<(u16, i32, u32, u32, Vec<u32>)>,
}

impl DeskewBarState {
    pub(super) fn new(current_page_1based: u16, total_pages: u16) -> Self {
        Self {
            angle_deg: 0.0,
            angle_input: "0.0".to_string(),
            scope: DeskewScope::CurrentPage,
            range_input: format!("{current_page_1based}-{total_pages}"),
            focus: DeskewTextFocus::None,
            draw_guideline: false,
            show_grid: true,
            dragging_slider: false,
            guide_start: None,
            guide_end: None,
            guide_dragging: false,
            last_axis: None,
            preview_cache: None,
        }
    }

    pub(super) fn set_angle(&mut self, deg: f32) {
        let snapped = (deg.clamp(-15.0, 15.0) * 10.0).round() / 10.0;
        // Avoid "-0.0" display
        self.angle_deg = if snapped.abs() < 0.05 { 0.0 } else { snapped };
        self.angle_input = format!("{:.1}", self.angle_deg);
        self.preview_cache = None;
    }

    pub(super) fn angle_tenths(&self) -> i32 {
        (self.angle_deg * 10.0).round() as i32
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum DeskewHit {
    AngleMinus,
    AngleInput,
    AnglePlus,
    Slider,
    ToggleGuideline,
    ToggleGrid,
    ScopeCurrent,
    ScopeRange,
    RangeInput,
    Reset,
    Apply,
    Cancel,
    InsideBar,
}

#[derive(Clone, Copy)]
struct DeskewBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: DeskewHit,
}

impl DeskewBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

/// Rotates a `0x00RRGGBB` bitmap clockwise by `cw_deg` around its center in screen space (Y-down),
/// filling out-of-bounds corners with crisp white (`0x00FF_FFFF`) to match PDF MediaBox white corners.
pub(super) fn rotate_bitmap_preview_cw(src: &[u32], w: u32, h: u32, cw_deg: f32) -> Vec<u32> {
    let n = (w as usize) * (h as usize);
    if n == 0 || src.len() < n {
        return Vec::new();
    }
    if cw_deg.abs() < 0.02 {
        return src[..n].to_vec();
    }
    let rad = (cw_deg as f64).to_radians();
    let cos_t = rad.cos() as f32;
    let sin_t = rad.sin() as f32;
    let cx = (w as f32) * 0.5;
    let cy = (h as f32) * 0.5;
    let wi = w as i32;
    let hi = h as i32;

    let mut out = vec![0x00FF_FFFFu32; n];
    for y in 0..hi {
        let dy = (y as f32) - cy;
        let row_off = (y as usize) * (w as usize);
        let base_sx = cx + sin_t * dy;
        let base_sy = cy + cos_t * dy;
        for x in 0..wi {
            let dx = (x as f32) - cx;
            // Inverse of screen Y-down CW rotation:
            // src_x = cx + cos*dx + sin*dy, src_y = cy - sin*dx + cos*dy
            let sx = (base_sx + cos_t * dx).round() as i32;
            let sy = (base_sy - sin_t * dx).round() as i32;
            if sx >= 0 && sx < wi && sy >= 0 && sy < hi {
                out[row_off + (x as usize)] = src[(sy as usize) * (w as usize) + (sx as usize)];
            }
        }
    }
    out
}

/// Parses a page range specification (e.g. `"1-6"` or `"1, 3, 5-8"`) into sorted unique 0-based page indices.
pub(super) fn parse_deskew_page_range(spec: &str, total_pages: u16) -> Option<Vec<u16>> {
    if total_pages == 0 {
        return None;
    }
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.eq_ignore_ascii_case("all") {
        return Some((0..total_pages).collect());
    }
    let mut pages = Vec::new();
    for part in trimmed.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if let Some((a_str, b_str)) = p.split_once('-') {
            let a = a_str.trim().parse::<u16>().ok()?;
            let b = if b_str.trim().is_empty() {
                total_pages
            } else {
                b_str.trim().parse::<u16>().ok()?
            };
            if a == 0 || b == 0 || a > b || b > total_pages {
                return None;
            }
            for pg in a..=b {
                pages.push(pg - 1);
            }
        } else {
            let single = p.parse::<u16>().ok()?;
            if single == 0 || single > total_pages {
                return None;
            }
            pages.push(single - 1);
        }
    }
    if pages.is_empty() {
        return None;
    }
    pages.sort_unstable();
    pages.dedup();
    Some(pages)
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn deskew_bar_height_px(&self, sf: f64) -> u32 {
        if self.deskew_bar.is_some() && !self.is_fullscreen && !self.is_presentation {
            (40.0 * sf).round() as u32
        } else {
            0
        }
    }

    pub(super) fn open_deskew_bar(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first before using Page Deskew".to_string());
            self.request_redraw();
            return;
        }
        self.close_all_popovers();
        self.compare_mode = None;
        if !self.view_mode.is_page_view() {
            self.view_mode = ViewMode::FitPage;
        }
        self.scroll_mode = ScrollMode::SinglePage;
        self.mouse_mode = MouseMode::Select;
        let cur = self.current_page_index + 1;
        let total = self.total_pages().max(1);
        self.deskew_bar = Some(DeskewBarState::new(cur, total));
        self.status_notice = Some(
            "Page Deskew: Adjust angle (-15°..+15°) or enable Draw Guideline to snap along a margin/line"
                .to_string(),
        );
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn close_deskew_bar(&mut self) {
        if self.deskew_bar.take().is_some() {
            self.status_notice = Some("Closed Page Deskew".to_string());
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn deskew_bar_layout(
        &self,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<DeskewBox>)> {
        let _ = self.deskew_bar.as_ref()?;
        let bar_h = self.deskew_bar_height_px(sf);
        if bar_h == 0 {
            return None;
        }
        let s = sf as f32;
        let x = sidebar_w as i32;
        let y = top_bars_h as i32;
        let w = buf_w.saturating_sub(sidebar_w);
        let pad = (8.0 * s).round() as i32;
        let gap = (5.0 * s).round() as i32;
        let btn_h = (24.0 * s).round() as u32;
        let by = y + ((bar_h.saturating_sub(btn_h)) / 2) as i32;

        // Compact 720p-safe layout: left controls + right action buttons
        let mut cx = x + pad + (58.0 * s).round() as i32; // after "Deskew:" label
        let sm_w = (22.0 * s).round() as u32;
        let ang_w = (54.0 * s).round() as u32;
        let slider_w = ((126.0 * s).round() as u32).min(w / 6).max(64);
        let guide_w = (82.0 * s).round() as u32;
        let grid_w = (50.0 * s).round() as u32;
        let sc_cur_w = (54.0 * s).round() as u32;
        let sc_rng_w = (48.0 * s).round() as u32;
        let rng_in_w = (62.0 * s).round() as u32;

        let mut hits = Vec::with_capacity(12);

        hits.push(DeskewBox { x: cx, y: by, w: sm_w, h: btn_h, hit: DeskewHit::AngleMinus });
        cx += sm_w as i32 + 3;
        hits.push(DeskewBox { x: cx, y: by, w: ang_w, h: btn_h, hit: DeskewHit::AngleInput });
        cx += ang_w as i32 + 3;
        hits.push(DeskewBox { x: cx, y: by, w: sm_w, h: btn_h, hit: DeskewHit::AnglePlus });
        cx += sm_w as i32 + gap + 2;
        hits.push(DeskewBox { x: cx, y: by, w: slider_w, h: btn_h, hit: DeskewHit::Slider });
        cx += slider_w as i32 + gap + 4;
        hits.push(DeskewBox { x: cx, y: by, w: guide_w, h: btn_h, hit: DeskewHit::ToggleGuideline });
        cx += guide_w as i32 + gap;
        hits.push(DeskewBox { x: cx, y: by, w: grid_w, h: btn_h, hit: DeskewHit::ToggleGrid });
        cx += grid_w as i32 + gap + 4;
        hits.push(DeskewBox { x: cx, y: by, w: sc_cur_w, h: btn_h, hit: DeskewHit::ScopeCurrent });
        cx += sc_cur_w as i32 + 2;
        hits.push(DeskewBox { x: cx, y: by, w: sc_rng_w, h: btn_h, hit: DeskewHit::ScopeRange });
        cx += sc_rng_w as i32 + 3;
        hits.push(DeskewBox { x: cx, y: by, w: rng_in_w, h: btn_h, hit: DeskewHit::RangeInput });

        // Right-aligned Reset / Apply / Cancel
        let cancel_w = (56.0 * s).round() as u32;
        let apply_w = (58.0 * s).round() as u32;
        let reset_w = (50.0 * s).round() as u32;
        let cancel_x = x + w as i32 - pad - cancel_w as i32;
        let apply_x = cancel_x - gap - apply_w as i32;
        let reset_x = apply_x - gap - reset_w as i32;
        hits.push(DeskewBox { x: reset_x, y: by, w: reset_w, h: btn_h, hit: DeskewHit::Reset });
        hits.push(DeskewBox { x: apply_x, y: by, w: apply_w, h: btn_h, hit: DeskewHit::Apply });
        hits.push(DeskewBox { x: cancel_x, y: by, w: cancel_w, h: btn_h, hit: DeskewHit::Cancel });

        Some((x, y, w, bar_h, hits))
    }

    pub(super) fn hit_test_deskew_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<DeskewHit> {
        let (x, y, w, h, hits) = self.deskew_bar_layout(buf_w, sidebar_w, top_bars_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < x || ix >= x + w as i32 || iy < y || iy >= y + h as i32 {
            return None;
        }
        hits.into_iter()
            .find(|b| b.contains(ix, iy))
            .map(|b| b.hit)
            .or(Some(DeskewHit::InsideBar))
    }

    pub(super) fn update_deskew_slider_from_x(
        &mut self,
        mx: f64,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) {
        let Some((_, _, _, _, hits)) = self.deskew_bar_layout(buf_w, sidebar_w, top_bars_h, sf) else {
            return;
        };
        let Some(sb) = hits.iter().find(|b| b.hit == DeskewHit::Slider) else {
            return;
        };
        let t = (((mx as f32) - (sb.x as f32)) / (sb.w.max(1) as f32)).clamp(0.0, 1.0);
        let deg = -15.0 + t * 30.0;
        if let Some(ref mut d) = self.deskew_bar {
            d.set_angle(deg);
        }
        self.request_redraw();
    }

    pub(super) fn handle_deskew_bar_click(
        &mut self,
        hit: DeskewHit,
        mx: f64,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) {
        match hit {
            DeskewHit::AngleMinus => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.focus = DeskewTextFocus::None;
                    d.set_angle(d.angle_deg - 0.1);
                }
                self.request_redraw();
            }
            DeskewHit::AnglePlus => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.focus = DeskewTextFocus::None;
                    d.set_angle(d.angle_deg + 0.1);
                }
                self.request_redraw();
            }
            DeskewHit::AngleInput => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.focus = DeskewTextFocus::Angle;
                }
                self.request_redraw();
            }
            DeskewHit::Slider => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.focus = DeskewTextFocus::None;
                    d.dragging_slider = true;
                }
                self.update_deskew_slider_from_x(mx, buf_w, sidebar_w, top_bars_h, sf);
            }
            DeskewHit::ToggleGuideline => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.focus = DeskewTextFocus::None;
                    d.draw_guideline = !d.draw_guideline;
                    if !d.draw_guideline {
                        d.guide_start = None;
                        d.guide_end = None;
                        d.guide_dragging = false;
                    }
                    let msg = if d.draw_guideline {
                        "Draw Guideline ON: Click-and-drag (or two-click) along a skewed text margin or horizontal line"
                    } else {
                        "Draw Guideline OFF"
                    };
                    self.status_notice = Some(msg.to_string());
                }
                self.request_redraw();
            }
            DeskewHit::ToggleGrid => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.show_grid = !d.show_grid;
                }
                self.request_redraw();
            }
            DeskewHit::ScopeCurrent => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.scope = DeskewScope::CurrentPage;
                    d.focus = DeskewTextFocus::None;
                }
                self.request_redraw();
            }
            DeskewHit::ScopeRange => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.scope = DeskewScope::PageRange;
                    d.focus = DeskewTextFocus::Range;
                }
                self.request_redraw();
            }
            DeskewHit::RangeInput => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.scope = DeskewScope::PageRange;
                    d.focus = DeskewTextFocus::Range;
                }
                self.request_redraw();
            }
            DeskewHit::Reset => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.set_angle(0.0);
                    d.guide_start = None;
                    d.guide_end = None;
                    d.guide_dragging = false;
                    d.last_axis = None;
                    d.focus = DeskewTextFocus::None;
                }
                self.status_notice = Some("Reset deskew angle to 0.0°".to_string());
                self.request_redraw();
            }
            DeskewHit::Apply => {
                self.apply_deskew_from_bar();
            }
            DeskewHit::Cancel => {
                self.close_deskew_bar();
            }
            DeskewHit::InsideBar => {
                if let Some(ref mut d) = self.deskew_bar {
                    d.focus = DeskewTextFocus::None;
                }
                self.request_redraw();
            }
        }
    }

    pub(super) fn commit_deskew_guideline(&mut self, p1: (f64, f64), p2: (f64, f64)) {
        let dx = (p2.0 - p1.0) as f32;
        let dy = (p2.1 - p1.1) as f32;
        if let Some((axis, deg)) = compute_guideline_deskew_angle(dx, dy) {
            if let Some(ref mut d) = self.deskew_bar {
                d.guide_start = Some(p1);
                d.guide_end = Some(p2);
                d.guide_dragging = false;
                d.last_axis = Some(axis);
                d.set_angle(deg);
            }
            let axis_lbl = match axis {
                DeskewAxis::Horizontal => "Horizontal (0°)",
                DeskewAxis::Vertical => "Vertical (90°)",
            };
            self.status_notice = Some(format!(
                "Guideline snapped to {axis_lbl} -> Deskew {deg:+.1}° (click Apply to commit)"
            ));
            self.request_redraw();
        }
    }

    pub(super) fn apply_deskew_from_bar(&mut self) {
        let Some(bar) = self.deskew_bar.clone() else {
            return;
        };
        let total = self.total_pages();
        if total == 0 {
            return;
        }
        let angle = bar.angle_deg.clamp(-15.0, 15.0);
        if angle.abs() < 0.05 {
            self.status_notice = Some("Deskew angle is 0.0° — no rotation applied".to_string());
            self.request_redraw();
            return;
        }
        let target_pages = match bar.scope {
            DeskewScope::CurrentPage => vec![self.current_page_index],
            DeskewScope::PageRange => match parse_deskew_page_range(&bar.range_input, total) {
                Some(p) => p,
                None => {
                    self.status_notice = Some(format!(
                        "Invalid page range '{}' (use e.g. '1-6' or '1,3,5')",
                        bar.range_input.trim()
                    ));
                    self.request_redraw();
                    return;
                }
            },
        };

        self.push_undo_snapshot();
        let res = self
            .doc
            .as_mut()
            .map(|doc| PdfEngine::deskew_pages_in_doc(doc, &target_pages, angle));
        match res {
            Some(Ok(count)) => {
                for &p in &target_pages {
                    self.thumbnail_cache.remove(&p);
                }
                self.deskew_bar = None;
                self.invalidate_page_caches();
                self.status_notice = Some(format!(
                    "Deskewed {count} page(s) by {angle:+.1}° (matrix transform, Ctrl+Z to undo, S to save)"
                ));
            }
            Some(Err(e)) => {
                let _ = self.undo_stack.pop_back();
                self.status_notice = Some(format!("Deskew failed: {e}"));
            }
            None => {}
        }
        self.update_title_bar();
        self.request_redraw();
    }

    /// Returns the live rotated preview pixel slice for the active page when Deskew angle != 0.
    pub(super) fn ensure_deskew_preview_pixels(&mut self) {
        let Some(bar) = self.deskew_bar.as_ref() else {
            return;
        };
        if bar.angle_deg.abs() < 0.02 {
            if let Some(ref mut d) = self.deskew_bar {
                d.preview_cache = None;
            }
            return;
        }
        let Some(cache) = self.cached_page.as_ref() else {
            return;
        };
        if cache.pixels.is_empty() {
            return;
        }
        let page_idx = cache.page_index;
        let w = cache.bitmap_width;
        let h = cache.bitmap_height;
        let tenths = bar.angle_tenths();
        let angle = bar.angle_deg;
        let stale = bar
            .preview_cache
            .as_ref()
            .map_or(true, |c| c.0 != page_idx || c.1 != tenths || c.2 != w || c.3 != h);
        if stale {
            let rotated = rotate_bitmap_preview_cw(&cache.pixels, w, h, angle);
            if let Some(ref mut d) = self.deskew_bar {
                d.preview_cache = Some((page_idx, tenths, w, h, rotated));
            }
        }
    }

    /// Draws the optional alignment grid overlay, Draw Guideline vector overlay, and compact Deskew bar.
    pub(super) fn draw_deskew_overlay(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        let Some(bar) = self.deskew_bar.as_ref() else {
            return;
        };
        let Some((bx, by, bw, bh, hits)) =
            self.deskew_bar_layout(buf_w, sidebar_w, top_bars_h, sf)
        else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let bottom_limit = buf_h.saturating_sub(status_bar_h);
        let content_top = (by + bh as i32).max(0);

        // 1. Optional Alignment Grid Overlay over the page draw rect
        if bar.show_grid && self.page_draw_rect.2 > 16 && self.page_draw_rect.3 > 16 {
            let (px, py, pw, ph) = self.page_draw_rect;
            let x0 = px.max(sidebar_w as i32);
            let x1 = (px + pw as i32).min(buf_w as i32);
            let y0 = py.max(content_top);
            let y1 = (py + ph as i32).min(bottom_limit as i32);
            let step = (48.0 * s).round().max(24.0) as i32;
            let grid_col = 0x0002_84C7u32;
            if x1 > x0 && y1 > y0 {
                // Vertical alignment lines
                let mut gx = px + step;
                while gx < x1 {
                    if gx >= x0 {
                        for gy in (y0..y1).step_by(2) {
                            let idx = (gy as u32 * buf_w + gx as u32) as usize;
                            buffer[idx] = blend_50(buffer[idx], grid_col);
                        }
                    }
                    gx += step;
                }
                // Horizontal alignment lines
                let mut gy = py + step;
                while gy < y1 {
                    if gy >= y0 {
                        for gx in (x0..x1).step_by(2) {
                            let idx = (gy as u32 * buf_w + gx as u32) as usize;
                            buffer[idx] = blend_50(buffer[idx], grid_col);
                        }
                    }
                    gy += step;
                }
            }
        }

        // 2. High-contrast Draw Guideline Overlay
        if let Some((sx1, sy1)) = bar.guide_start {
            let (sx2, sy2) = if bar.guide_dragging || bar.guide_end.is_none() {
                self.cursor_pos
            } else {
                bar.guide_end.unwrap_or(self.cursor_pos)
            };
            let x0 = sx1.round() as i32;
            let y0 = sy1.round() as i32;
            let x1 = sx2.round() as i32;
            let y1 = sy2.round() as i32;
            let dx = (x1 - x0) as f32;
            let dy = (y1 - y0) as f32;

            // Target snapped reference line (dashed amber)
            if let Some((axis, deg)) = compute_guideline_deskew_angle(dx, dy) {
                let (rx1, ry1) = match axis {
                    DeskewAxis::Horizontal => (x1, y0),
                    DeskewAxis::Vertical => (x0, y1),
                };
                draw_deskew_line(buffer, buf_w, bottom_limit, x0, y0, rx1, ry1, 0x00F5_9E0B, true, false);
                // Main drawn guideline (high-contrast red-orange + white halo endpoints)
                draw_deskew_line(buffer, buf_w, bottom_limit, x0, y0, x1, y1, 0x00DC_2626, false, true);
                let arm = (6.0 * s).round() as i32;
                for (cx, cy) in [(x0, y0), (x1, y1)] {
                    draw_deskew_line(buffer, buf_w, bottom_limit, cx - arm, cy, cx + arm, cy, 0x0002_84C7, false, true);
                    draw_deskew_line(buffer, buf_w, bottom_limit, cx, cy - arm, cx, cy + arm, 0x0002_84C7, false, true);
                }

                // Live angle snap pill at midpoint
                let small_sz = (10.5 * s).clamp(9.0, 20.0);
                let axis_ch = if axis == DeskewAxis::Vertical { "V 90°" } else { "H 0°" };
                let pill = format!("Snap {axis_ch}: {deg:+.1}°");
                let pw = (measure_text(&pill, small_sz, true).ceil() as u32) + (14.0 * s).round() as u32;
                let ph = (20.0 * s).round() as u32;
                let mx = ((x0 + x1) / 2 - (pw as i32 / 2)).clamp(sidebar_w as i32 + 6, (buf_w as i32 - pw as i32 - 6).max(6));
                let my = ((y0 + y1) / 2 - ph as i32 - 8).clamp(content_top + 4, (bottom_limit as i32 - ph as i32 - 4).max(content_top + 4));
                fill_rounded_rect(buffer, buf_w, bottom_limit, mx, my, pw, ph, 5.0, 0x001E_293B);
                stroke_rounded_rect(buffer, buf_w, bottom_limit, mx, my, pw, ph, 5.0, 0x00F5_9E0B);
                let tw = measure_text(&pill, small_sz, true);
                draw_text_vcenter(buffer, buf_w, bottom_limit, mx + ((pw as f32 - tw) * 0.5).round() as i32, my, ph, mx + pw as i32, &pill, small_sz, 0x00FF_FFFF, true);
            }
        }

        // 3. Compact Top Deskew Control Bar
        fill_rect_clipped(buffer, buf_w, buf_h, bx, by, bw, bh, theme.surface_elevated);
        fill_rect_clipped(buffer, buf_w, buf_h, bx, by + bh as i32 - 1, bw, 1, theme.divider);

        let small_sz = (10.8 * s).clamp(9.0, 20.0);
        let pad = (8.0 * s).round() as i32;
        draw_text_vcenter(
            buffer,
            buf_w,
            buf_h,
            bx + pad,
            by,
            bh,
            bx + pad + (56.0 * s).round() as i32,
            "Deskew:",
            small_sz,
            theme.text_primary,
            true,
        );

        for b in hits {
            let hov = self.hovered_widget == Some(UiHit::Deskew(b.hit));
            match b.hit {
                DeskewHit::AngleMinus | DeskewHit::AnglePlus => {
                    let lbl = if b.hit == DeskewHit::AngleMinus { "-" } else { "+" };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, if hov { theme.hover_bg } else { theme.surface_bg });
                    stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, theme.border_subtle);
                    let tw = measure_text(lbl, small_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, small_sz, theme.text_primary, true);
                }
                DeskewHit::AngleInput => {
                    let focused = bar.focus == DeskewTextFocus::Angle;
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, theme.input_bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, if focused { theme.input_focus_border } else { theme.border_subtle });
                    let txt = if focused {
                        format!("{}|", bar.angle_input)
                    } else {
                        format!("{:+.1}°", bar.angle_deg)
                    };
                    let tw = measure_text(&txt, small_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32 - 2, &txt, small_sz, theme.text_primary, true);
                }
                DeskewHit::Slider => {
                    let track_h = (4.0 * s).round().max(3.0) as u32;
                    let track_y = b.y + ((b.h.saturating_sub(track_h)) / 2) as i32;
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x + 4, track_y, b.w.saturating_sub(8), track_h, 2.0, theme.border_subtle);
                    // Center zero tick
                    let mid_x = b.x + (b.w as i32) / 2;
                    fill_rect_clipped(buffer, buf_w, buf_h, mid_x, track_y - 3, 1, track_h + 6, theme.text_muted);
                    // Filled bar from center to thumb
                    let frac = ((bar.angle_deg + 15.0) / 30.0).clamp(0.0, 1.0);
                    let thumb_x = b.x + 4 + (((b.w.saturating_sub(8)) as f32) * frac).round() as i32;
                    let fx0 = mid_x.min(thumb_x);
                    let fw = (thumb_x - mid_x).unsigned_abs();
                    if fw > 0 {
                        fill_rect_clipped(buffer, buf_w, buf_h, fx0, track_y, fw, track_h, theme.accent);
                    }
                    let knob = (12.0 * s).round().max(10.0) as u32;
                    let kx = thumb_x - (knob as i32) / 2;
                    let ky = b.y + ((b.h.saturating_sub(knob)) / 2) as i32;
                    fill_rounded_rect(buffer, buf_w, buf_h, kx, ky, knob, knob, (knob as f32) * 0.5, theme.accent);
                    stroke_rounded_rect(buffer, buf_w, buf_h, kx, ky, knob, knob, (knob as f32) * 0.5, theme.text_on_accent);
                }
                DeskewHit::ToggleGuideline => {
                    let active = bar.draw_guideline;
                    let bg = if active { theme.accent } else if hov { theme.hover_bg } else { theme.surface_bg };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0, bg);
                    if !active {
                        stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0, theme.border_subtle);
                    }
                    let fg = if active { theme.text_on_accent } else { theme.text_primary };
                    let lbl = "Guideline";
                    let tw = measure_text(lbl, small_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, small_sz, fg, true);
                }
                DeskewHit::ToggleGrid => {
                    let active = bar.show_grid;
                    let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_bg };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0, if active { theme.accent } else { theme.border_subtle });
                    let fg = if active { theme.accent_soft_fg } else { theme.text_secondary };
                    let lbl = "Grid";
                    let tw = measure_text(lbl, small_sz, active);
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, small_sz, fg, active);
                }
                DeskewHit::ScopeCurrent | DeskewHit::ScopeRange => {
                    let active = (b.hit == DeskewHit::ScopeCurrent && bar.scope == DeskewScope::CurrentPage)
                        || (b.hit == DeskewHit::ScopeRange && bar.scope == DeskewScope::PageRange);
                    let lbl = if b.hit == DeskewHit::ScopeCurrent { "Current" } else { "Range" };
                    let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_bg };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, bg);
                    stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, if active { theme.accent } else { theme.border_subtle });
                    let fg = if active { theme.accent_soft_fg } else { theme.text_secondary };
                    let tw = measure_text(lbl, small_sz, active);
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, small_sz, fg, active);
                }
                DeskewHit::RangeInput => {
                    let active = bar.scope == DeskewScope::PageRange;
                    let focused = bar.focus == DeskewTextFocus::Range;
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, if active { theme.input_bg } else { theme.surface_bg });
                    stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 4.0, if focused { theme.input_focus_border } else { theme.border_subtle });
                    let txt = if focused { format!("{}|", bar.range_input) } else { bar.range_input.clone() };
                    let fg = if active { theme.text_primary } else { theme.text_muted };
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + 5, b.y, b.h, b.x + b.w as i32 - 3, &txt, small_sz, fg, false);
                }
                DeskewHit::Reset | DeskewHit::Cancel => {
                    let lbl = if b.hit == DeskewHit::Reset { "Reset" } else { "Cancel" };
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0, if hov { theme.hover_bg } else { theme.surface_bg });
                    stroke_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0, theme.border_subtle);
                    let tw = measure_text(lbl, small_sz, false);
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, small_sz, theme.text_primary, false);
                }
                DeskewHit::Apply => {
                    fill_rounded_rect(buffer, buf_w, buf_h, b.x, b.y, b.w, b.h, 5.0, if hov { theme.accent_hover } else { theme.accent });
                    let lbl = "Apply";
                    let tw = measure_text(lbl, small_sz, true);
                    draw_text_vcenter(buffer, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, small_sz, theme.text_on_accent, true);
                }
                DeskewHit::InsideBar => {}
            }
        }
    }
}

#[inline]
fn blend_50(dst: u32, src: u32) -> u32 {
    let r = (((dst >> 16) & 0xFF) * 3 + ((src >> 16) & 0xFF)) >> 2;
    let g = (((dst >> 8) & 0xFF) * 3 + ((src >> 8) & 0xFF)) >> 2;
    let b = ((dst & 0xFF) * 3 + (src & 0xFF)) >> 2;
    (r << 16) | (g << 8) | b
}

fn draw_deskew_line(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    dashed: bool,
    thick: bool,
) {
    let dx = (x1 - x0) as f32;
    let dy = (y1 - y0) as f32;
    let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
    for i in 0..=steps {
        if dashed && ((i / 6) % 2 == 1) {
            continue;
        }
        let t = (i as f32) / (steps as f32);
        let px = (x0 as f32 + dx * t).round() as i32;
        let py = (y0 as f32 + dy * t).round() as i32;
        let offsets: &[(i32, i32)] = if thick {
            &[(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)]
        } else {
            &[(0, 0)]
        };
        for &(ox, oy) in offsets {
            let xx = px + ox;
            let yy = py + oy;
            if xx >= 0 && yy >= 0 && (xx as u32) < buf_w && (yy as u32) < buf_h {
                buffer[(yy as u32 * buf_w + xx as u32) as usize] = color;
            }
        }
    }
}
