//! Milestone 2.2: Detailed Watermark Dialog with Live Preview (`Tools -> Add Watermark... (W)`).
//!
//! Compact 720p modal featuring a real-time rotated/alpha-blended page preview on the left and
//! full controls on the right: Text, System Font Picker, Font Size, Color Swatches, Rotation
//! (`-180°..+180°` with `-45°/0°/+45°` presets), Opacity (`5%..100%`), Placement (Centered,
//! Custom Position, or Tiled/Repeating with spacing), and Page Range. Zero idle RAM when closed.

use crate::engine::{PdfEngine, WatermarkConfig, WatermarkPlacement};
use pdfium_render::prelude::*;

use super::deskew::parse_deskew_page_range;
use super::font_dropdown::*;
use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

const WM_COLORS: [(&str, (u8, u8, u8)); 6] = [
    ("Gray", (120, 125, 135)),
    ("Red", (210, 38, 48)),
    ("Blue", (37, 99, 235)),
    ("Black", (28, 32, 38)),
    ("Green", (22, 145, 74)),
    ("Purple", (124, 58, 237)),
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum WatermarkFocus {
    None,
    Text,
    CustomRange,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum WatermarkHit {
    Close,
    PrevPage,
    NextPage,
    FocusText,
    FontDropdownToggle,
    FontDropdownPopup(FontDropdownHit),
    FontDropdownClose,
    SizeMinus,
    SizePlus,
    OpacityMinus,
    OpacityPlus,
    ColorSwatch(usize),
    RotMinus,
    RotPreset(i16),
    RotPlus,
    PlaceCentered,
    PlaceCustom,
    PlaceTiled,
    Param1Minus,
    Param1Plus,
    Param2Minus,
    Param2Plus,
    ScopeAll,
    ScopeCustom,
    FocusRange,
    Apply,
    Cancel,
    InsideModal,
}

#[derive(Clone, Copy)]
struct WmBox {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: WatermarkHit,
}

impl WmBox {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

pub(super) struct WatermarkModalState {
    pub(super) cfg: WatermarkConfig,
    pub(super) all_pages: bool,
    pub(super) custom_range: String,
    pub(super) focus: WatermarkFocus,
    pub(super) preview_page: u16,
    /// `(page_idx, avail_w, avail_h, rw, rh, pixels)`
    pub(super) preview_cache: Option<(u16, u32, u32, u32, u32, Vec<u32>)>,
    pub(super) status_msg: Option<(String, bool)>,
    pub(super) font_dropdown: FontDropdownState,
}

impl WatermarkModalState {
    pub(super) fn new(current_page: u16) -> Self {
        Self {
            cfg: WatermarkConfig::default(),
            all_pages: true,
            custom_range: String::new(),
            focus: WatermarkFocus::Text,
            preview_page: current_page,
            preview_cache: None,
            status_msg: None,
            font_dropdown: FontDropdownState::default(),
        }
    }
}

/// Renders a rotated, alpha-blended text stamp centered at `(cx, cy)` inside a clipped rect
/// `(clip_x0..clip_x1, clip_y0..clip_y1)` for real-time CPU watermark preview.
#[allow(clippy::too_many_arguments)]
fn draw_rotated_text_preview(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    clip_x0: i32,
    clip_y0: i32,
    clip_x1: i32,
    clip_y1: i32,
    cx: f32,
    cy: f32,
    text: &str,
    size_px: f32,
    rgb: (u8, u8, u8),
    opacity_pct: u8,
    rot_ccw_deg: f32,
) {
    if text.trim().is_empty() {
        return;
    }
    let sz = size_px.clamp(8.0, 72.0);
    let tw = (measure_text(text, sz, true).ceil() as u32 + 8).min(512);
    let th = ((sz * 1.35).ceil() as u32 + 6).min(128);
    if tw == 0 || th == 0 {
        return;
    }
    // Render white text onto black scratch mask so brightness = glyph coverage alpha
    let mut mask = vec![0u32; (tw * th) as usize];
    draw_text_vcenter(
        &mut mask,
        tw,
        th,
        4,
        0,
        th,
        tw as i32 - 2,
        text,
        sz,
        0x00FF_FFFF,
        true,
    );

    // PDF positive CCW angle rotates screen Y-down coordinates by -rot_ccw_deg
    let rad = (-rot_ccw_deg as f64).to_radians();
    let cos_t = rad.cos() as f32;
    let sin_t = rad.sin() as f32;
    let half_w = tw as f32 * 0.5;
    let half_h = th as f32 * 0.5;
    let opacity = (opacity_pct.clamp(5, 100) as f32) / 100.0;

    // Rotated bounding box on screen
    let ext_x = (half_w * cos_t.abs() + half_h * sin_t.abs()).ceil() as i32 + 2;
    let ext_y = (half_w * sin_t.abs() + half_h * cos_t.abs()).ceil() as i32 + 2;
    let x0 = (cx.round() as i32 - ext_x).max(clip_x0).max(0);
    let x1 = (cx.round() as i32 + ext_x).min(clip_x1).min(buf_w as i32);
    let y0 = (cy.round() as i32 - ext_y).max(clip_y0).max(0);
    let y1 = (cy.round() as i32 + ext_y).min(clip_y1).min(buf_h as i32);

    let (sr, sg, sb) = (rgb.0 as u32, rgb.1 as u32, rgb.2 as u32);
    for py in y0..y1 {
        let dy = py as f32 - cy;
        let row_off = (py as u32 * buf_w) as usize;
        for px in x0..x1 {
            let dx = px as f32 - cx;
            // Inverse rotate back into mask space
            let mx = (dx * cos_t + dy * sin_t + half_w).round() as i32;
            let my = (-dx * sin_t + dy * cos_t + half_h).round() as i32;
            if mx >= 0 && mx < tw as i32 && my >= 0 && my < th as i32 {
                let cov = (mask[(my as u32 * tw + mx as u32) as usize] & 0xFF) as f32;
                if cov > 1.0 {
                    let a = ((cov / 255.0) * opacity * 255.0).round().clamp(0.0, 255.0) as u32;
                    let inv = 255 - a;
                    let dst = buffer[row_off + px as usize];
                    let dr = (dst >> 16) & 0xFF;
                    let dg = (dst >> 8) & 0xFF;
                    let db = dst & 0xFF;
                    let r = (sr * a + dr * inv) / 255;
                    let g = (sg * a + dg * inv) / 255;
                    let b = (sb * a + db * inv) / 255;
                    buffer[row_off + px as usize] = (r << 16) | (g << 8) | b;
                }
            }
        }
    }
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_watermark_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to add a watermark".to_string());
            self.request_redraw();
            return;
        }
        self.close_all_popovers();
        self.watermark_modal = Some(WatermarkModalState::new(self.current_page_index));
        self.request_redraw();
    }

    fn watermark_modal_layout(
        &self,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, u32, Vec<WmBox>)> {
        let st = self.watermark_modal.as_ref()?;
        let s = sf as f32;
        let mw = ((770.0 * s).round() as u32).min(buf_w.saturating_sub(24)).max(560);
        let mh = ((518.0 * s).round() as u32).min(buf_h.saturating_sub(24)).max(440);
        let mx = ((buf_w.saturating_sub(mw)) / 2) as i32;
        let my = ((buf_h.saturating_sub(mh)) / 2) as i32;
        let left_w = ((mw as f32) * 0.39).round() as u32;
        let panel_x = mx + left_w as i32;
        let panel_w = mw.saturating_sub(left_w);
        let p = (14.0 * s).round() as i32;
        let hdr_h = (42.0 * s).round() as u32;
        let row_h = (26.0 * s).round() as u32;
        let gap = (6.0 * s).round() as i32;
        let lbl_h = (15.0 * s).round() as i32;
        let ix = panel_x + p;
        let iw = panel_w.saturating_sub((p * 2) as u32);
        let sm = (24.0 * s).round() as u32;

        let mut hits = Vec::with_capacity(32);

        // Header Close X
        hits.push(WmBox {
            x: mx + mw as i32 - p - sm as i32,
            y: my + ((hdr_h.saturating_sub(sm)) / 2) as i32,
            w: sm,
            h: sm,
            hit: WatermarkHit::Close,
        });

        // Left preview stepper
        let step_y = my + mh as i32 - p - row_h as i32;
        let step_w = (54.0 * s).round() as u32;
        hits.push(WmBox { x: mx + p, y: step_y, w: step_w, h: row_h, hit: WatermarkHit::PrevPage });
        hits.push(WmBox { x: mx + left_w as i32 - p - step_w as i32, y: step_y, w: step_w, h: row_h, hit: WatermarkHit::NextPage });

        let mut cy = my + hdr_h as i32 + (10.0 * s).round() as i32;

        // 1. Watermark Text
        cy += lbl_h;
        hits.push(WmBox { x: ix, y: cy, w: iw, h: row_h, hit: WatermarkHit::FocusText });
        cy += row_h as i32 + (8.0 * s).round() as i32;

        // 2. System Font Picker
        cy += lbl_h;
        hits.push(WmBox { x: ix, y: cy, w: iw, h: row_h, hit: WatermarkHit::FontDropdownToggle });
        cy += row_h as i32 + (8.0 * s).round() as i32;

        // 3. Font Size & Opacity
        cy += lbl_h;
        let half_w = (iw.saturating_sub(gap as u32)) / 2;
        hits.push(WmBox { x: ix, y: cy, w: sm, h: row_h, hit: WatermarkHit::SizeMinus });
        hits.push(WmBox { x: ix + half_w as i32 - sm as i32, y: cy, w: sm, h: row_h, hit: WatermarkHit::SizePlus });
        let rx2 = ix + half_w as i32 + gap;
        hits.push(WmBox { x: rx2, y: cy, w: sm, h: row_h, hit: WatermarkHit::OpacityMinus });
        hits.push(WmBox { x: ix + iw as i32 - sm as i32, y: cy, w: sm, h: row_h, hit: WatermarkHit::OpacityPlus });
        cy += row_h as i32 + (8.0 * s).round() as i32;

        // 4. Color Swatches (6)
        cy += lbl_h;
        let sw_w = (iw.saturating_sub((gap * 5) as u32)) / 6;
        for i in 0..WM_COLORS.len() {
            hits.push(WmBox {
                x: ix + (i as i32) * (sw_w as i32 + gap),
                y: cy,
                w: sw_w,
                h: row_h,
                hit: WatermarkHit::ColorSwatch(i),
            });
        }
        cy += row_h as i32 + (8.0 * s).round() as i32;

        // 5. Rotation (-180..+180 with -45 / 0 / +45 presets)
        cy += lbl_h;
        let preset_w = (46.0 * s).round() as u32;
        hits.push(WmBox { x: ix, y: cy, w: sm, h: row_h, hit: WatermarkHit::RotMinus });
        let mut px = ix + sm as i32 + 4;
        for deg in [-45i16, 0, 45] {
            hits.push(WmBox { x: px, y: cy, w: preset_w, h: row_h, hit: WatermarkHit::RotPreset(deg) });
            px += preset_w as i32 + 4;
        }
        hits.push(WmBox { x: ix + iw as i32 - sm as i32, y: cy, w: sm, h: row_h, hit: WatermarkHit::RotPlus });
        cy += row_h as i32 + (8.0 * s).round() as i32;

        // 6. Coverage / Placement (Centered | Custom | Tiled)
        cy += lbl_h;
        let col3 = (iw.saturating_sub((gap * 2) as u32)) / 3;
        hits.push(WmBox { x: ix, y: cy, w: col3, h: row_h, hit: WatermarkHit::PlaceCentered });
        hits.push(WmBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: WatermarkHit::PlaceCustom });
        hits.push(WmBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(col3 * 2 + (gap * 2) as u32), h: row_h, hit: WatermarkHit::PlaceTiled });
        cy += row_h as i32 + (6.0 * s).round() as i32;

        // Optional placement sub-parameters (Custom X%/Y% or Tiled Spacing)
        if !matches!(st.cfg.placement, WatermarkPlacement::Centered) {
            hits.push(WmBox { x: ix, y: cy, w: sm, h: row_h, hit: WatermarkHit::Param1Minus });
            hits.push(WmBox { x: ix + half_w as i32 - sm as i32, y: cy, w: sm, h: row_h, hit: WatermarkHit::Param1Plus });
            hits.push(WmBox { x: rx2, y: cy, w: sm, h: row_h, hit: WatermarkHit::Param2Minus });
            hits.push(WmBox { x: ix + iw as i32 - sm as i32, y: cy, w: sm, h: row_h, hit: WatermarkHit::Param2Plus });
            cy += row_h as i32 + (8.0 * s).round() as i32;
        }

        // 7. Page Range
        cy += lbl_h;
        let all_w = (72.0 * s).round() as u32;
        let cust_w = (68.0 * s).round() as u32;
        hits.push(WmBox { x: ix, y: cy, w: all_w, h: row_h, hit: WatermarkHit::ScopeAll });
        hits.push(WmBox { x: ix + all_w as i32 + 4, y: cy, w: cust_w, h: row_h, hit: WatermarkHit::ScopeCustom });
        let rng_x = ix + all_w as i32 + cust_w as i32 + 8;
        let rng_w = (ix + iw as i32 - rng_x).max(60) as u32;
        hits.push(WmBox { x: rng_x, y: cy, w: rng_w, h: row_h, hit: WatermarkHit::FocusRange });

        // Footer Apply / Cancel
        let foot_h = (32.0 * s).round() as u32;
        let foot_y = my + mh as i32 - p - foot_h as i32;
        let cancel_w = (80.0 * s).round() as u32;
        let apply_w = iw.saturating_sub(cancel_w + gap as u32);
        hits.push(WmBox { x: ix, y: foot_y, w: apply_w, h: foot_h, hit: WatermarkHit::Apply });
        hits.push(WmBox { x: ix + apply_w as i32 + gap, y: foot_y, w: cancel_w, h: foot_h, hit: WatermarkHit::Cancel });

        Some((mx, my, mw, mh, left_w, hits))
    }

    pub(super) fn ensure_watermark_preview_rendered(&mut self, buf_w: u32, buf_h: u32, sf: f64) {
        let Some((_, _, _, mh, left_w, _)) = self.watermark_modal_layout(buf_w, buf_h, sf) else {
            return;
        };
        let s = sf as f32;
        let avail_w = left_w.saturating_sub((32.0 * s).round() as u32).max(80);
        let avail_h = mh.saturating_sub((120.0 * s).round() as u32).max(100);
        let page_idx = self.watermark_modal.as_ref().map(|m| m.preview_page).unwrap_or(0);
        let already = self
            .watermark_modal
            .as_ref()
            .and_then(|m| m.preview_cache.as_ref())
            .is_some_and(|(p, aw, ah, _, _, _)| *p == page_idx && *aw == avail_w && *ah == avail_h);
        if already {
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let total = doc.pages().len();
        if total == 0 {
            return;
        }
        let clamped_idx = page_idx.min(total - 1);
        let Ok(page) = doc.pages().get(clamped_idx) else {
            return;
        };
        let pw = page.width().value.max(1.0);
        let ph = page.height().value.max(1.0);
        let sc = (avail_w as f32 / pw).min(avail_h as f32 / ph);
        let rw = (pw * sc).round().max(32.0) as u32;
        let rh = (ph * sc).round().max(32.0) as u32;
        let cfg = PdfRenderConfig::new()
            .set_fixed_width(rw as i32)
            .set_fixed_height(rh as i32)
            .render_form_data(true)
            .render_annotations(true);
        if let Ok(bmp) = page.render_with_config(&cfg) {
            let raw = bmp.as_raw_bytes();
            let bw = bmp.width() as u32;
            let bh = bmp.height() as u32;
            let mut pixels = Vec::with_capacity((bw * bh) as usize);
            for ch in raw.chunks_exact(4) {
                pixels.push(((ch[2] as u32) << 16) | ((ch[1] as u32) << 8) | (ch[0] as u32));
            }
            if let Some(ref mut m) = self.watermark_modal {
                m.preview_page = clamped_idx;
                m.preview_cache = Some((clamped_idx, avail_w, avail_h, bw, bh, pixels));
            }
        }
    }

    pub(super) fn draw_watermark_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(st) = self.watermark_modal.as_ref() else {
            return;
        };
        let Some((mx, my, mw, mh, left_w, hits)) = self.watermark_modal_layout(buf_w, buf_h, sf) else {
            return;
        };
        let theme = self.ui_theme();
        let s = sf as f32;
        let p = (14.0 * s).round() as i32;
        let hdr_h = (42.0 * s).round() as u32;
        let rad = (10.0 * s).max(6.0);
        let btn_rad = (5.0 * s).max(3.0);
        let title_sz = (14.5 * s).clamp(12.0, 26.0);
        let body_sz = (11.2 * s).clamp(9.5, 20.0);
        let lbl_sz = (10.2 * s).clamp(9.0, 18.0);
        let lbl_h = (15.0 * s).round() as i32;

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        draw_soft_shadow(buffer, buf_w, buf_h, mx, my, mw, mh, rad, 18, 6, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        draw_text_vcenter(buffer, buf_w, buf_h, mx + p, my, hdr_h, mx + mw as i32 - 44, "Add Watermark — Live Preview", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, mx, my + hdr_h as i32, mw, 1, theme.divider);
        fill_rect_clipped(buffer, buf_w, buf_h, mx + left_w as i32, my + hdr_h as i32, 1, mh.saturating_sub(hdr_h), theme.divider);

        // Left preview canvas
        let canvas_x = mx + p;
        let canvas_y = my + hdr_h as i32 + (12.0 * s).round() as i32;
        let canvas_w = left_w.saturating_sub((p * 2) as u32);
        let canvas_h = mh.saturating_sub(hdr_h + (64.0 * s).round() as u32);
        fill_rounded_rect(buffer, buf_w, buf_h, canvas_x, canvas_y, canvas_w, canvas_h, 6.0 * s, theme.canvas_bg);

        let (px, py, pw, ph) = if let Some((_, _, _, bw, bh, ref pixels)) = st.preview_cache {
            let ox = canvas_x + ((canvas_w.saturating_sub(bw)) / 2) as i32;
            let oy = canvas_y + ((canvas_h.saturating_sub(bh)) / 2) as i32;
            draw_soft_shadow(buffer, buf_w, buf_h, ox, oy, bw, bh, 2.0, 8, 3, theme.shadow_rgb, theme.shadow_max_alpha);
            for r in 0..bh {
                let dy = oy + r as i32;
                if dy < canvas_y || dy >= canvas_y + canvas_h as i32 {
                    continue;
                }
                let dst_off = (dy as u32 * buf_w) as usize;
                let src_off = (r * bw) as usize;
                for c in 0..bw {
                    let dx = ox + c as i32;
                    if dx >= canvas_x && dx < canvas_x + canvas_w as i32 {
                        buffer[dst_off + dx as usize] = pixels[src_off + c as usize];
                    }
                }
            }
            stroke_rounded_rect(buffer, buf_w, buf_h, ox, oy, bw, bh, 1.0, theme.page_border);
            (ox, oy, bw, bh)
        } else {
            // Synthetic blank white preview sheet when no PDF bitmap is cached (e.g. headless preview)
            let bh = canvas_h.saturating_sub(24).max(120);
            let bw = ((bh as f32) * 0.772).round().min(canvas_w.saturating_sub(20) as f32) as u32;
            let ox = canvas_x + ((canvas_w.saturating_sub(bw)) / 2) as i32;
            let oy = canvas_y + ((canvas_h.saturating_sub(bh)) / 2) as i32;
            draw_soft_shadow(buffer, buf_w, buf_h, ox, oy, bw, bh, 2.0, 8, 3, theme.shadow_rgb, theme.shadow_max_alpha);
            fill_rect_clipped(buffer, buf_w, buf_h, ox, oy, bw, bh, 0x00FF_FFFF);
            // Subtle mock text lines on blank sheet
            for line_i in 0..10 {
                let ly = oy + 26 + line_i * ((bh as i32 - 52) / 10).max(12);
                let lw = if line_i % 3 == 2 { bw * 58 / 100 } else { bw * 76 / 100 };
                fill_rect_clipped(buffer, buf_w, buf_h, ox + (bw / 10) as i32, ly, lw, 4, 0x00E2_E8F0);
            }
            stroke_rounded_rect(buffer, buf_w, buf_h, ox, oy, bw, bh, 1.0, theme.page_border);
            (ox, oy, bw, bh)
        };

        // Live Watermark Overlay on preview page
        let scale_pt_to_px = (pw as f32) / 612.0;
        let preview_font_px = (st.cfg.font_size * scale_pt_to_px).clamp(8.0, 68.0);
        let centers: Vec<(f32, f32)> = match st.cfg.placement {
            WatermarkPlacement::Centered => vec![(px as f32 + pw as f32 * 0.5, py as f32 + ph as f32 * 0.5)],
            WatermarkPlacement::Custom { x_pct, y_pct } => vec![(
                px as f32 + pw as f32 * x_pct.clamp(0.05, 0.95),
                py as f32 + ph as f32 * y_pct.clamp(0.05, 0.95),
            )],
            WatermarkPlacement::Tiled { gap_x_pt, gap_y_pt } => {
                let tw_px = measure_text(&st.cfg.text, preview_font_px, true).max(24.0);
                let sx = (tw_px * 0.85 + gap_x_pt * scale_pt_to_px).max(40.0);
                let sy = (preview_font_px * 1.4 + gap_y_pt * scale_pt_to_px).max(32.0);
                let mut pts = Vec::new();
                let mut row_i = 0usize;
                let mut y = py as f32 + sy * 0.5;
                while y < (py + ph as i32) as f32 + sy * 0.5 {
                    let stag = if row_i % 2 == 1 { sx * 0.5 } else { 0.0 };
                    let mut x = px as f32 + sx * 0.5 - stag;
                    while x < (px + pw as i32) as f32 + sx * 0.5 {
                        pts.push((x, y));
                        x += sx;
                    }
                    y += sy;
                    row_i += 1;
                }
                pts
            }
        };
        for (cx, cy) in centers {
            draw_rotated_text_preview(
                buffer,
                buf_w,
                buf_h,
                px,
                py,
                px + pw as i32,
                py + ph as i32,
                cx,
                cy,
                &st.cfg.text,
                preview_font_px,
                st.cfg.color_rgb,
                st.cfg.opacity_pct,
                st.cfg.rotation_deg,
            );
        }

        let find = |target: WatermarkHit| hits.iter().copied().find(|b| b.hit == target);
        let draw_pill = |buf: &mut [u32], b: WmBox, lbl: &str, active: bool| {
            let hov = self.hovered_widget == Some(UiHit::Watermark(b.hit));
            let bg = if active { theme.accent } else if hov { theme.hover_bg } else { theme.surface_elevated };
            let fg = if active { theme.text_on_accent } else { theme.text_primary };
            let bdr = if active { theme.accent } else { theme.border_subtle };
            fill_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bg);
            stroke_rounded_rect(buf, buf_w, buf_h, b.x, b.y, b.w, b.h, btn_rad, bdr);
            let tw = measure_text(lbl, body_sz, active);
            draw_text_vcenter(buf, buf_w, buf_h, b.x + ((b.w as f32 - tw) * 0.5).round() as i32, b.y, b.h, b.x + b.w as i32, lbl, body_sz, fg, active);
        };

        // Close X
        if let Some(cb) = find(WatermarkHit::Close) {
            draw_pill(buffer, cb, "x", false);
        }

        // Left preview page stepper label
        if let (Some(prev), Some(next)) = (find(WatermarkHit::PrevPage), find(WatermarkHit::NextPage)) {
            draw_pill(buffer, prev, "< Prev", false);
            draw_pill(buffer, next, "Next >", false);
            let total = self.total_pages().max(1);
            let lbl = format!("Page {} / {}", st.preview_page + 1, total);
            let tw = measure_text(&lbl, lbl_sz, true);
            let cx = prev.x + prev.w as i32 + (((next.x - (prev.x + prev.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, prev.y, prev.h, next.x, &lbl, lbl_sz, theme.text_secondary, true);
        }

        // 1. Watermark Text
        if let Some(tb) = find(WatermarkHit::FocusText) {
            draw_text(buffer, buf_w, buf_h, tb.x, tb.y - lbl_h, "Watermark Text", lbl_sz, theme.text_muted, true);
            let focused = st.focus == WatermarkFocus::Text;
            fill_rounded_rect(buffer, buf_w, buf_h, tb.x, tb.y, tb.w, tb.h, btn_rad, theme.input_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, tb.x, tb.y, tb.w, tb.h, btn_rad, if focused { theme.accent } else { theme.border_subtle });
            let display = if focused { format!("{}|", st.cfg.text) } else { st.cfg.text.clone() };
            draw_text_vcenter(buffer, buf_w, buf_h, tb.x + 8, tb.y, tb.h, tb.x + tb.w as i32 - 6, &display, body_sz, theme.text_primary, false);
        }

        // 2. System Font Picker
        if let Some(r) = find(WatermarkHit::FontDropdownToggle) {
            draw_text(buffer, buf_w, buf_h, r.x, r.y - lbl_h, "Font Family (System & Built-in)", lbl_sz, theme.text_muted, true);
            let hov = self.hovered_widget == Some(UiHit::Watermark(WatermarkHit::FontDropdownToggle));
            draw_font_dropdown_button(
                buffer,
                buf_w,
                buf_h,
                r.x,
                r.y,
                r.w,
                r.h,
                &st.cfg.font_name,
                None,
                st.font_dropdown.open,
                hov,
                &theme,
                s,
            );
        }

        // 3. Font Size & Opacity
        if let (Some(sm), Some(sp)) = (find(WatermarkHit::SizeMinus), find(WatermarkHit::SizePlus)) {
            draw_text(buffer, buf_w, buf_h, sm.x, sm.y - lbl_h, "Font Size", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, sm, "-", false);
            draw_pill(buffer, sp, "+", false);
            let txt = format!("{:.0} pt", st.cfg.font_size);
            let tw = measure_text(&txt, body_sz, true);
            let cx = sm.x + sm.w as i32 + (((sp.x - (sm.x + sm.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, sm.y, sm.h, sp.x, &txt, body_sz, theme.text_primary, true);
        }
        if let (Some(om), Some(op)) = (find(WatermarkHit::OpacityMinus), find(WatermarkHit::OpacityPlus)) {
            draw_text(buffer, buf_w, buf_h, om.x, om.y - lbl_h, "Opacity", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, om, "-", false);
            draw_pill(buffer, op, "+", false);
            let txt = format!("{}%", st.cfg.opacity_pct);
            let tw = measure_text(&txt, body_sz, true);
            let cx = om.x + om.w as i32 + (((op.x - (om.x + om.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, om.y, om.h, op.x, &txt, body_sz, theme.text_primary, true);
        }

        // 4. Color Swatches
        for (i, &(_name, rgb)) in WM_COLORS.iter().enumerate() {
            if let Some(sb) = find(WatermarkHit::ColorSwatch(i)) {
                if i == 0 {
                    draw_text(buffer, buf_w, buf_h, sb.x, sb.y - lbl_h, "Color", lbl_sz, theme.text_muted, true);
                }
                let active = st.cfg.color_rgb == rgb;
                let col = ((rgb.0 as u32) << 16) | ((rgb.1 as u32) << 8) | (rgb.2 as u32);
                fill_rounded_rect(buffer, buf_w, buf_h, sb.x, sb.y, sb.w, sb.h, btn_rad, col);
                stroke_rounded_rect(buffer, buf_w, buf_h, sb.x, sb.y, sb.w, sb.h, btn_rad, if active { theme.accent } else { theme.border_subtle });
                if active {
                    stroke_rounded_rect(buffer, buf_w, buf_h, sb.x + 2, sb.y + 2, sb.w.saturating_sub(4), sb.h.saturating_sub(4), 3.0, 0x00FF_FFFF);
                }
            }
        }

        // 5. Rotation
        if let (Some(rm), Some(rp)) = (find(WatermarkHit::RotMinus), find(WatermarkHit::RotPlus)) {
            draw_text(buffer, buf_w, buf_h, rm.x, rm.y - lbl_h, "Rotation (-180° to +180°)", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, rm, "-", false);
            draw_pill(buffer, rp, "+", false);
            let mut last_px = rm.x + rm.w as i32;
            for deg in [-45i16, 0, 45] {
                if let Some(pb) = find(WatermarkHit::RotPreset(deg)) {
                    let lbl = if deg > 0 { format!("+{deg}°") } else { format!("{deg}°") };
                    draw_pill(buffer, pb, &lbl, (st.cfg.rotation_deg - deg as f32).abs() < 0.5);
                    last_px = pb.x + pb.w as i32;
                }
            }
            let cur_lbl = format!("{:+.0}°", st.cfg.rotation_deg);
            let tw = measure_text(&cur_lbl, body_sz, true);
            let cx = last_px + (((rp.x - last_px) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, rm.y, rm.h, rp.x, &cur_lbl, body_sz, theme.text_primary, true);
        }

        // 6. Placement
        if let Some(pc) = find(WatermarkHit::PlaceCentered) {
            draw_text(buffer, buf_w, buf_h, pc.x, pc.y - lbl_h, "Coverage & Placement", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, pc, "Centered", matches!(st.cfg.placement, WatermarkPlacement::Centered));
        }
        if let Some(pu) = find(WatermarkHit::PlaceCustom) {
            draw_pill(buffer, pu, "Custom Pos", matches!(st.cfg.placement, WatermarkPlacement::Custom { .. }));
        }
        if let Some(pt) = find(WatermarkHit::PlaceTiled) {
            draw_pill(buffer, pt, "Tiled / Grid", matches!(st.cfg.placement, WatermarkPlacement::Tiled { .. }));
        }
        if let (Some(p1m), Some(p1p), Some(p2m), Some(p2p)) = (
            find(WatermarkHit::Param1Minus),
            find(WatermarkHit::Param1Plus),
            find(WatermarkHit::Param2Minus),
            find(WatermarkHit::Param2Plus),
        ) {
            draw_pill(buffer, p1m, "-", false);
            draw_pill(buffer, p1p, "+", false);
            draw_pill(buffer, p2m, "-", false);
            draw_pill(buffer, p2p, "+", false);
            let (l1, l2) = match st.cfg.placement {
                WatermarkPlacement::Custom { x_pct, y_pct } => (
                    format!("X {:.0}%", x_pct * 100.0),
                    format!("Y {:.0}%", y_pct * 100.0),
                ),
                WatermarkPlacement::Tiled { gap_x_pt, gap_y_pt } => (
                    format!("GapX {:.0}", gap_x_pt),
                    format!("GapY {:.0}", gap_y_pt),
                ),
                WatermarkPlacement::Centered => (String::new(), String::new()),
            };
            let t1 = measure_text(&l1, lbl_sz, true);
            let c1 = p1m.x + p1m.w as i32 + (((p1p.x - (p1m.x + p1m.w as i32)) as f32 - t1) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, c1, p1m.y, p1m.h, p1p.x, &l1, lbl_sz, theme.text_primary, true);
            let t2 = measure_text(&l2, lbl_sz, true);
            let c2 = p2m.x + p2m.w as i32 + (((p2p.x - (p2m.x + p2m.w as i32)) as f32 - t2) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, c2, p2m.y, p2m.h, p2p.x, &l2, lbl_sz, theme.text_primary, true);
        }

        // 7. Page Range
        if let Some(sa) = find(WatermarkHit::ScopeAll) {
            draw_text(buffer, buf_w, buf_h, sa.x, sa.y - lbl_h, "Apply To Pages", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, sa, "All Pages", st.all_pages);
        }
        if let Some(sc) = find(WatermarkHit::ScopeCustom) {
            draw_pill(buffer, sc, "Range", !st.all_pages);
        }
        if let Some(rb) = find(WatermarkHit::FocusRange) {
            let focused = st.focus == WatermarkFocus::CustomRange;
            fill_rounded_rect(buffer, buf_w, buf_h, rb.x, rb.y, rb.w, rb.h, btn_rad, if st.all_pages { theme.surface_elevated } else { theme.input_bg });
            stroke_rounded_rect(buffer, buf_w, buf_h, rb.x, rb.y, rb.w, rb.h, btn_rad, if focused { theme.accent } else { theme.border_subtle });
            let disp = if focused {
                format!("{}|", st.custom_range)
            } else if st.custom_range.is_empty() {
                "e.g. 1-5, 8".to_string()
            } else {
                st.custom_range.clone()
            };
            let fg = if st.custom_range.is_empty() && !focused { theme.text_muted } else { theme.text_primary };
            draw_text_vcenter(buffer, buf_w, buf_h, rb.x + 8, rb.y, rb.h, rb.x + rb.w as i32 - 4, &disp, body_sz, fg, false);
        }

        if let Some((ref msg, is_err)) = st.status_msg {
            if let Some(ab) = find(WatermarkHit::Apply) {
                draw_text(buffer, buf_w, buf_h, ab.x, ab.y - (18.0 * s).round() as i32, msg, lbl_sz, if is_err { theme.danger } else { theme.accent }, true);
            }
        }

        // Footer buttons
        if let Some(ab) = find(WatermarkHit::Apply) {
            draw_pill(buffer, ab, "Apply Watermark", true);
        }
        if let Some(cb) = find(WatermarkHit::Cancel) {
            draw_pill(buffer, cb, "Cancel", false);
        }
        if st.font_dropdown.open {
            if let Some(r) = find(WatermarkHit::FontDropdownToggle) {
                draw_font_dropdown_popup(
                    buffer,
                    buf_w,
                    buf_h,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    sf,
                    &st.font_dropdown,
                    &st.cfg.font_name,
                    &theme,
                );
            }
        }
    }

    pub(super) fn hit_test_watermark_modal(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<WatermarkHit> {
        let st = self.watermark_modal.as_ref()?;
        let (x, y, w, h, _, hits) = self.watermark_modal_layout(buf_w, buf_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if st.font_dropdown.open {
            if let Some(anchor) = hits.iter().find(|b| b.hit == WatermarkHit::FontDropdownToggle) {
                if let Some(pop_hit) = hit_test_font_dropdown_popup(
                    ix,
                    iy,
                    anchor.x,
                    anchor.y,
                    anchor.w,
                    anchor.h,
                    buf_w,
                    buf_h,
                    sf,
                    &st.font_dropdown,
                ) {
                    return Some(WatermarkHit::FontDropdownPopup(pop_hit));
                }
                if anchor.contains(ix, iy) {
                    return Some(WatermarkHit::FontDropdownToggle);
                }
            }
            return Some(WatermarkHit::FontDropdownClose);
        }
        if ix < x || ix >= x + w as i32 || iy < y || iy >= y + h as i32 {
            return None;
        }
        hits.into_iter()
            .find(|b| b.contains(ix, iy))
            .map(|b| b.hit)
            .or(Some(WatermarkHit::InsideModal))
    }

    pub(super) fn handle_watermark_click(&mut self, hit: WatermarkHit) {
        let total = self.total_pages().max(1);
        match hit {
            WatermarkHit::Close | WatermarkHit::Cancel => {
                self.watermark_modal = None;
            }
            WatermarkHit::PrevPage => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    if m.preview_page > 0 {
                        m.preview_page -= 1;
                        m.preview_cache = None;
                    }
                }
            }
            WatermarkHit::NextPage => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    if m.preview_page + 1 < total {
                        m.preview_page += 1;
                        m.preview_cache = None;
                    }
                }
            }
            WatermarkHit::FocusText => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.focus = WatermarkFocus::Text;
                }
            }
            WatermarkHit::FontDropdownToggle => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    let cur = m.cfg.font_name.clone();
                    m.font_dropdown.toggle(&cur);
                }
            }
            WatermarkHit::FontDropdownClose => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.font_dropdown.close();
                }
            }
            WatermarkHit::FontDropdownPopup(pop) => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    match pop {
                        FontDropdownHit::Row(idx) => {
                            if let Some(entry) = crate::fonts::global_system_fonts().get(idx) {
                                m.cfg.font_name = entry.display_name.clone();
                            }
                            m.font_dropdown.close();
                        }
                        FontDropdownHit::TrackPageUp => {
                            m.font_dropdown.scroll_by(-(FONT_DROPDOWN_MAX_ROWS as isize));
                        }
                        FontDropdownHit::TrackPageDown => {
                            m.font_dropdown.scroll_by(FONT_DROPDOWN_MAX_ROWS as isize);
                        }
                        FontDropdownHit::InsidePopup => {}
                    }
                }
            }
            WatermarkHit::SizeMinus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.font_size = (m.cfg.font_size - 4.0).max(12.0);
                }
            }
            WatermarkHit::SizePlus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.font_size = (m.cfg.font_size + 4.0).min(144.0);
                }
            }
            WatermarkHit::OpacityMinus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.opacity_pct = m.cfg.opacity_pct.saturating_sub(5).max(5);
                }
            }
            WatermarkHit::OpacityPlus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.opacity_pct = (m.cfg.opacity_pct + 5).min(100);
                }
            }
            WatermarkHit::ColorSwatch(i) => {
                if let (Some(m), Some(&(_, rgb))) = (self.watermark_modal.as_mut(), WM_COLORS.get(i)) {
                    m.cfg.color_rgb = rgb;
                }
            }
            WatermarkHit::RotMinus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.rotation_deg = (m.cfg.rotation_deg - 15.0).max(-180.0);
                }
            }
            WatermarkHit::RotPlus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.rotation_deg = (m.cfg.rotation_deg + 15.0).min(180.0);
                }
            }
            WatermarkHit::RotPreset(deg) => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.rotation_deg = deg as f32;
                }
            }
            WatermarkHit::PlaceCentered => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.cfg.placement = WatermarkPlacement::Centered;
                }
            }
            WatermarkHit::PlaceCustom => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    if !matches!(m.cfg.placement, WatermarkPlacement::Custom { .. }) {
                        m.cfg.placement = WatermarkPlacement::Custom { x_pct: 0.5, y_pct: 0.5 };
                    }
                }
            }
            WatermarkHit::PlaceTiled => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    if !matches!(m.cfg.placement, WatermarkPlacement::Tiled { .. }) {
                        m.cfg.placement = WatermarkPlacement::Tiled { gap_x_pt: 90.0, gap_y_pt: 90.0 };
                    }
                }
            }
            WatermarkHit::Param1Minus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    match &mut m.cfg.placement {
                        WatermarkPlacement::Custom { x_pct, .. } => *x_pct = (*x_pct - 0.05).max(0.1),
                        WatermarkPlacement::Tiled { gap_x_pt, .. } => *gap_x_pt = (*gap_x_pt - 15.0).max(30.0),
                        WatermarkPlacement::Centered => {}
                    }
                }
            }
            WatermarkHit::Param1Plus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    match &mut m.cfg.placement {
                        WatermarkPlacement::Custom { x_pct, .. } => *x_pct = (*x_pct + 0.05).min(0.9),
                        WatermarkPlacement::Tiled { gap_x_pt, .. } => *gap_x_pt = (*gap_x_pt + 15.0).min(280.0),
                        WatermarkPlacement::Centered => {}
                    }
                }
            }
            WatermarkHit::Param2Minus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    match &mut m.cfg.placement {
                        WatermarkPlacement::Custom { y_pct, .. } => *y_pct = (*y_pct - 0.05).max(0.1),
                        WatermarkPlacement::Tiled { gap_y_pt, .. } => *gap_y_pt = (*gap_y_pt - 15.0).max(30.0),
                        WatermarkPlacement::Centered => {}
                    }
                }
            }
            WatermarkHit::Param2Plus => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    match &mut m.cfg.placement {
                        WatermarkPlacement::Custom { y_pct, .. } => *y_pct = (*y_pct + 0.05).min(0.9),
                        WatermarkPlacement::Tiled { gap_y_pt, .. } => *gap_y_pt = (*gap_y_pt + 15.0).min(280.0),
                        WatermarkPlacement::Centered => {}
                    }
                }
            }
            WatermarkHit::ScopeAll => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.all_pages = true;
                    m.focus = WatermarkFocus::None;
                }
            }
            WatermarkHit::ScopeCustom | WatermarkHit::FocusRange => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.all_pages = false;
                    m.focus = WatermarkFocus::CustomRange;
                }
            }
            WatermarkHit::Apply => {
                self.apply_watermark_from_modal();
                return;
            }
            WatermarkHit::InsideModal => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    m.focus = WatermarkFocus::None;
                }
            }
        }
        self.request_redraw();
    }

    pub(super) fn handle_watermark_key(&mut self, key: &winit::keyboard::Key, text: Option<&str>) {
        use winit::keyboard::{Key, NamedKey};
        match key {
            Key::Named(NamedKey::Escape) => {
                if self.watermark_modal.as_ref().is_some_and(|m| m.font_dropdown.open) {
                    if let Some(m) = self.watermark_modal.as_mut() {
                        m.font_dropdown.close();
                    }
                } else {
                    self.watermark_modal = None;
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                self.apply_watermark_from_modal();
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(m) = self.watermark_modal.as_mut() {
                    match m.focus {
                        WatermarkFocus::Text => {
                            m.cfg.text.pop();
                        }
                        WatermarkFocus::CustomRange => {
                            m.custom_range.pop();
                        }
                        WatermarkFocus::None => {}
                    }
                }
                self.request_redraw();
            }
            _ => {
                if let (Some(t), Some(m)) = (text, self.watermark_modal.as_mut()) {
                    match m.focus {
                        WatermarkFocus::Text => {
                            for ch in t.chars() {
                                if !ch.is_control() && m.cfg.text.len() < 48 {
                                    m.cfg.text.push(ch);
                                }
                            }
                        }
                        WatermarkFocus::CustomRange => {
                            for ch in t.chars() {
                                if (ch.is_ascii_digit() || matches!(ch, '-' | ',' | ' '))
                                    && m.custom_range.len() < 36
                                {
                                    m.custom_range.push(ch);
                                }
                            }
                        }
                        WatermarkFocus::None => {}
                    }
                    self.request_redraw();
                }
            }
        }
    }

    fn apply_watermark_from_modal(&mut self) {
        let Some(m) = self.watermark_modal.as_ref() else {
            return;
        };
        if m.cfg.text.trim().is_empty() {
            if let Some(mm) = self.watermark_modal.as_mut() {
                mm.status_msg = Some(("Enter watermark text first".to_string(), true));
            }
            self.request_redraw();
            return;
        }
        let total = self.total_pages();
        let pages: Vec<u16> = if m.all_pages {
            (0..total).collect()
        } else {
            match parse_deskew_page_range(&m.custom_range, total) {
                Some(p) => p,
                None => {
                    if let Some(mm) = self.watermark_modal.as_mut() {
                        mm.status_msg = Some((format!("Invalid range (1-{total}, e.g. 1-5, 8)"), true));
                    }
                    self.request_redraw();
                    return;
                }
            }
        };
        let cfg = m.cfg.clone();
        self.push_undo_snapshot();
        let res = if let Some(doc) = self.doc.as_mut() {
            PdfEngine::apply_watermark_full(doc, &cfg, &pages)
        } else {
            return;
        };
        match res {
            Ok(()) => {
                self.watermark_modal = None;
                self.has_unsaved_edits = true;
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice = Some(format!(
                    "Applied watermark '{}' ({}) to {} page(s) — Ctrl+Z to undo, S to save",
                    cfg.text.trim(),
                    cfg.font_name,
                    pages.len()
                ));
            }
            Err(e) => {
                if let Some(mm) = self.watermark_modal.as_mut() {
                    mm.status_msg = Some((format!("Watermark failed: {e}"), true));
                }
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }
}
