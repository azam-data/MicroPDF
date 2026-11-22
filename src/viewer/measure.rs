//! Tier 2.4: Quick Measure / Ruler Tool (`Edit -> Measure Distance & Area`).
//! Computes exact Length, ΔX × ΔY, Area, and Angle directly from PDF points (`72 pt = 1 in`)
//! in toggleable units (`mm`, `cm`, `in`, `pt`). Zero extra dependencies.

use pdfium_render::prelude::*;

use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum MeasureUnit {
    Mm,
    Cm,
    In,
    Pt,
}

impl MeasureUnit {
    pub(super) const ALL: [Self; 4] = [Self::Mm, Self::Cm, Self::In, Self::Pt];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Cm => "cm",
            Self::In => "in",
            Self::Pt => "pt",
        }
    }

    pub(super) fn area_label(self) -> &'static str {
        match self {
            Self::Mm => "mm²",
            Self::Cm => "cm²",
            Self::In => "in²",
            Self::Pt => "pt²",
        }
    }

    pub(super) fn from_points(self, pts: f32) -> f32 {
        match self {
            Self::Pt => pts,
            Self::In => pts / 72.0,
            Self::Cm => pts * 2.54 / 72.0,
            Self::Mm => pts * 25.4 / 72.0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct MeasurementResult {
    pub(super) page_idx: u16,
    pub(super) x1_pt: f32,
    pub(super) y1_pt: f32,
    pub(super) x2_pt: f32,
    pub(super) y2_pt: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct MeasurementStats {
    pub(super) length: f32,
    pub(super) dx: f32,
    pub(super) dy: f32,
    pub(super) area: f32,
    pub(super) angle_deg: f32,
}

/// Pure-Rust geometric measurement calculation from two PDF-space points (`72 pt = 1 inch`).
pub(super) fn compute_measurement_stats(
    x1_pt: f32,
    y1_pt: f32,
    x2_pt: f32,
    y2_pt: f32,
    unit: MeasureUnit,
) -> MeasurementStats {
    let raw_dx = x2_pt - x1_pt;
    let raw_dy = y2_pt - y1_pt;
    let raw_len = raw_dx.hypot(raw_dy);
    let dx = unit.from_points(raw_dx.abs());
    let dy = unit.from_points(raw_dy.abs());
    let length = unit.from_points(raw_len);
    let area = dx * dy;
    let angle_deg = if raw_len > 1e-4 {
        raw_dy.atan2(raw_dx).to_degrees()
    } else {
        0.0
    };
    MeasurementStats {
        length,
        dx,
        dy,
        area,
        angle_deg,
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum MeasureBarHit {
    Unit(MeasureUnit),
    Clear,
    Exit,
    InsideBar,
}

fn draw_line_clipped(
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
        if dashed && ((i / 5) % 2 == 1) {
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

impl<'a> PdfViewerApp<'a> {
    fn measure_bar_layout(
        &self,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<(i32, i32, u32, u32, MeasureBarHit)>)> {
        if self.mouse_mode != MouseMode::Measure || !self.view_mode.is_page_view() {
            return None;
        }
        let s = sf as f32;
        let pad = (10.0 * s).round() as i32;
        let gap = (5.0 * s).round() as i32;
        let btn_h = (22.0 * s).round() as u32;
        let bar_h = (36.0 * s).round() as u32;
        let unit_w = (32.0 * s).round() as u32;
        let clear_w = (48.0 * s).round() as u32;
        let exit_w = (44.0 * s).round() as u32;
        let readout_w = (340.0 * s).round() as u32;
        let bar_w = ((pad as u32) * 2
            + readout_w
            + unit_w * 4
            + clear_w
            + exit_w
            + (gap as u32) * 6)
            .min(buf_w.saturating_sub(sidebar_w + 16));
        let content_w = buf_w.saturating_sub(sidebar_w);
        let bx = sidebar_w as i32 + ((content_w.saturating_sub(bar_w)) / 2) as i32;
        let by = top_bars_h as i32 + (10.0 * s).round() as i32;
        let btn_y = by + ((bar_h.saturating_sub(btn_h)) / 2) as i32;

        let exit_x = bx + bar_w as i32 - pad - exit_w as i32;
        let clear_x = exit_x - gap - clear_w as i32;
        let mut hits = Vec::with_capacity(6);
        let mut ux = clear_x - gap - (unit_w as i32 * 4 + gap * 3);
        for unit in MeasureUnit::ALL {
            hits.push((ux, btn_y, unit_w, btn_h, MeasureBarHit::Unit(unit)));
            ux += unit_w as i32 + gap;
        }
        hits.push((clear_x, btn_y, clear_w, btn_h, MeasureBarHit::Clear));
        hits.push((exit_x, btn_y, exit_w, btn_h, MeasureBarHit::Exit));
        Some((bx, by, bar_w, bar_h, hits))
    }

    pub(super) fn hit_test_measure_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<MeasureBarHit> {
        let (bx, by, bw, bh, hits) = self.measure_bar_layout(buf_w, sidebar_w, top_bars_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < bx || ix >= bx + bw as i32 || iy < by || iy >= by + bh as i32 {
            return None;
        }
        for (rx, ry, rw, rh, hit) in hits {
            if ix >= rx && ix < rx + rw as i32 && iy >= ry && iy < ry + rh as i32 {
                return Some(hit);
            }
        }
        Some(MeasureBarHit::InsideBar)
    }

    pub(super) fn handle_measure_bar_click(&mut self, hit: MeasureBarHit) {
        match hit {
            MeasureBarHit::Unit(u) => {
                self.measure_unit = u;
                self.request_redraw();
            }
            MeasureBarHit::Clear => {
                self.last_measurement = None;
                self.request_redraw();
            }
            MeasureBarHit::Exit => {
                self.last_measurement = None;
                self.mouse_mode = MouseMode::Select;
                self.status_notice = Some("Exited Measure Tool — Mouse Mode: Select Text".to_string());
                self.update_title_bar();
                self.request_redraw();
            }
            MeasureBarHit::InsideBar => {}
        }
    }

    fn pdf_point_to_screen_point(&self, page_idx: u16, x_pt: f32, y_pt: f32) -> Option<(i32, i32)> {
        let rect = PdfRect::new_from_values(y_pt, x_pt, y_pt + 0.1, x_pt + 0.1);
        let (sx, sy, _, _) = self.pdf_rect_to_screen_rect(page_idx, rect)?;
        Some((sx, sy))
    }

    pub(super) fn draw_measure_overlay(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        if self.mouse_mode != MouseMode::Measure || !self.view_mode.is_page_view() {
            return;
        }
        let bottom_limit = buf_h.saturating_sub(status_bar_h);
        let theme = self.ui_theme();
        let s = sf as f32;
        let sub_sz = (10.5 * s).clamp(9.0, 20.0);
        let accent_cyan = 0x0002_84C7u32;

        // Determine active measurement (live drag overrides last completed measurement)
        let live_measure = self.drag_start.and_then(|(sx1, sy1)| {
            let (p1, pt1x, pt1y) = self.screen_point_to_pdf_point(sx1, sy1)?;
            let (p2, pt2x, pt2y) = self.screen_point_to_pdf_point(self.cursor_pos.0, self.cursor_pos.1)?;
            if p1 == p2 {
                Some(MeasurementResult {
                    page_idx: p1,
                    x1_pt: pt1x.value,
                    y1_pt: pt1y.value,
                    x2_pt: pt2x.value,
                    y2_pt: pt2y.value,
                })
            } else {
                None
            }
        });
        let active_m = live_measure.or(self.last_measurement);

        if let Some(m) = active_m {
            if let (Some((x0, y0)), Some((x1, y1))) = (
                self.pdf_point_to_screen_point(m.page_idx, m.x1_pt, m.y1_pt),
                self.pdf_point_to_screen_point(m.page_idx, m.x2_pt, m.y2_pt),
            ) {
                // Dashed ΔX × ΔY bounding guides
                draw_line_clipped(buffer, buf_w, bottom_limit, x0, y0, x1, y0, accent_cyan, true, false);
                draw_line_clipped(buffer, buf_w, bottom_limit, x1, y0, x1, y1, accent_cyan, true, false);
                // Thick solid diagonal ruler line
                draw_line_clipped(buffer, buf_w, bottom_limit, x0, y0, x1, y1, accent_cyan, false, true);

                // Endpoint crosshairs
                let arm = (6.0 * s).round() as i32;
                for (cx, cy) in [(x0, y0), (x1, y1)] {
                    draw_line_clipped(buffer, buf_w, bottom_limit, cx - arm, cy, cx + arm, cy, 0x00DC_2626, false, true);
                    draw_line_clipped(buffer, buf_w, bottom_limit, cx, cy - arm, cx, cy + arm, 0x00DC_2626, false, true);
                }

                // Midpoint length pill
                let stats = compute_measurement_stats(m.x1_pt, m.y1_pt, m.x2_pt, m.y2_pt, self.measure_unit);
                let pill_lbl = format!("{:.2} {}", stats.length, self.measure_unit.label());
                let pw = (measure_text(&pill_lbl, sub_sz, true).ceil() as u32) + (14.0 * s).round() as u32;
                let ph = (20.0 * s).round() as u32;
                let mx = ((x0 + x1) / 2 - (pw as i32 / 2)).clamp(sidebar_w as i32 + 4, (buf_w as i32 - pw as i32 - 4).max(4));
                let my = ((y0 + y1) / 2 - ph as i32 - 6).clamp(top_bars_h as i32 + 48, (bottom_limit as i32 - ph as i32 - 4).max(48));
                fill_rounded_rect(buffer, buf_w, bottom_limit, mx, my, pw, ph, 5.0, accent_cyan);
                let tw = measure_text(&pill_lbl, sub_sz, true);
                draw_text_vcenter(buffer, buf_w, bottom_limit, mx + ((pw as f32 - tw) * 0.5).round() as i32, my, ph, mx + pw as i32, &pill_lbl, sub_sz, 0x00FF_FFFF, true);
            }
        }

        // Draw floating Measure HUD Bar at top-center
        if let Some((bx, by, bw, bh, hits)) = self.measure_bar_layout(buf_w, sidebar_w, top_bars_h, sf) {
            let rad = (8.0 * s).max(5.0);
            let btn_rad = (5.0 * s).max(3.0);
            draw_soft_shadow(buffer, buf_w, buf_h, bx, by, bw, bh, rad, 12, 4, theme.shadow_rgb, theme.shadow_max_alpha);
            fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, rad, accent_cyan);

            let first_btn_x = hits.first().map(|h| h.0).unwrap_or(bx + bw as i32);
            let readout = if let Some(m) = active_m {
                let st = compute_measurement_stats(m.x1_pt, m.y1_pt, m.x2_pt, m.y2_pt, self.measure_unit);
                let u = self.measure_unit.label();
                let ua = self.measure_unit.area_label();
                format!(
                    "L: {:.2}{u}  ·  Δ: {:.2}×{:.2}{u}  ·  Area: {:.2}{ua}  ·  {:.1}°",
                    st.length, st.dx, st.dy, st.area, st.angle_deg
                )
            } else {
                "Ruler Tool — drag across any two points on the page to measure".to_string()
            };
            let clipped = truncate_ellipsis(&readout, (first_btn_x - bx - 18).max(40) as f32, sub_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, bx + (10.0 * s).round() as i32, by, bh, first_btn_x - 6, &clipped, sub_sz, theme.text_primary, true);

            for &(rx, ry, rw, rh, hit) in &hits {
                let hov = self.hovered_widget == Some(UiHit::MeasureBar(hit));
                match hit {
                    MeasureBarHit::Unit(u) => {
                        let active = self.measure_unit == u;
                        let bg = if active { accent_cyan } else if hov { theme.hover_bg } else { theme.surface_elevated };
                        let fg = if active { 0x00FF_FFFF } else { theme.text_secondary };
                        let bdr = if active { accent_cyan } else { theme.border_subtle };
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, btn_rad, bg);
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, btn_rad, bdr);
                        let lbl = u.label();
                        let tw = measure_text(lbl, sub_sz, active);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, lbl, sub_sz, fg, active);
                    }
                    MeasureBarHit::Clear => {
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, btn_rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, btn_rad, theme.border_subtle);
                        let tw = measure_text("Clear", sub_sz, false);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "Clear", sub_sz, theme.text_secondary, false);
                    }
                    MeasureBarHit::Exit => {
                        let bg = if hov { theme.danger } else { theme.danger_soft_bg };
                        let fg = if hov { theme.text_on_accent } else { theme.danger };
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, btn_rad, bg);
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, btn_rad, theme.danger);
                        let tw = measure_text("Done", sub_sz, true);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "Done", sub_sz, fg, true);
                    }
                    MeasureBarHit::InsideBar => {}
                }
            }
        }
    }
}
