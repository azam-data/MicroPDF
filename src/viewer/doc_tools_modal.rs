//! Interactive Visual Modals with Live Page Preview for:
//! 1. **Header & Footer** (`Tools -> Header / Footer...` / `H`):
//!    - 6 live text slots (Header L/C/R, Footer L/C/R) with `{page}` / `{total}` tokens
//!    - Page Range (`All` or `Custom`)
//!    - Font size, edge margin, color preset
//!    - Update / Replace Existing Header/Footer toggle
//!    - Built-in Clear Existing Header/Footer actions (`Clear Header`, `Clear Footer`, `Clear Both`)
//!    - Reusable Style Profiles persisted in portable `micropdf.ini`
//! 2. **Page Numbering** (`Tools -> Page Numbering...` / `N`):
//!    - Live page preview with page stepper (preview both Roman front-matter & Arabic body pages)
//!    - 6 positions, 5 formats (`1`, `Page 1`, `Page 1 of N`, `1 / N`, `- 1 -`), Roman front-matter count,
//!      Skip Page 1 (cover), Font Size, Color, and Clear Existing Page Numbers.

use crate::engine::{
    HeaderFooterColor, HeaderFooterProfile, MarginPosition, MarginZone, PageNumberConfig,
    PageNumberFormat, PdfEngine,
};
use pdfium_render::prelude::*;

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

// ---------------------------------------------------------------------------
// Header & Footer Modal
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum HfField {
    ProfileName,
    CustomRange,
    HeaderLeft,
    HeaderCenter,
    HeaderRight,
    FooterLeft,
    FooterCenter,
    FooterRight,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum HeaderFooterHit {
    PrevPage,
    NextPage,
    ProfilePrev,
    ProfileNext,
    ProfileSave,
    ProfileDelete,
    FocusField(HfField),
    ScopeAll,
    ScopeCustom,
    FontMinus,
    FontPlus,
    MarginMinus,
    MarginPlus,
    ColorGray,
    ColorBlack,
    ColorNavy,
    ToggleReplace,
    ClearHeader,
    ClearFooter,
    ClearBoth,
    Apply,
    Cancel,
    InsideModal,
}

pub(super) struct HeaderFooterModalState {
    pub(super) profile: HeaderFooterProfile,
    pub(super) selected_profile_idx: Option<usize>,
    pub(super) all_pages: bool,
    pub(super) custom_range: String,
    pub(super) replace_existing: bool,
    pub(super) focused_field: HfField,
    pub(super) preview_page: u16,
    pub(super) preview_cache: Option<(u16, u32, u32, Vec<u32>)>,
    pub(super) error_msg: Option<String>,
}

impl HeaderFooterModalState {
    pub(super) fn new(
        current_page: u16,
        last_profile: &HeaderFooterProfile,
        saved_profiles: &[HeaderFooterProfile],
    ) -> Self {
        let selected_profile_idx = saved_profiles
            .iter()
            .position(|p| p.name == last_profile.name);
        Self {
            profile: last_profile.clone(),
            selected_profile_idx,
            all_pages: true,
            custom_range: String::new(),
            replace_existing: true,
            focused_field: HfField::HeaderCenter,
            preview_page: current_page,
            preview_cache: None,
            error_msg: None,
        }
    }

    pub(super) fn focused_text_mut(&mut self) -> &mut String {
        match self.focused_field {
            HfField::ProfileName => &mut self.profile.name,
            HfField::CustomRange => &mut self.custom_range,
            HfField::HeaderLeft => &mut self.profile.header_left,
            HfField::HeaderCenter => &mut self.profile.header_center,
            HfField::HeaderRight => &mut self.profile.header_right,
            HfField::FooterLeft => &mut self.profile.footer_left,
            HfField::FooterCenter => &mut self.profile.footer_center,
            HfField::FooterRight => &mut self.profile.footer_right,
        }
    }
}

// ---------------------------------------------------------------------------
// Page Numbering Modal
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PageNumberHit {
    PrevPage,
    NextPage,
    Pos(MarginPosition),
    Format(PageNumberFormat),
    RomanMinus,
    RomanPlus,
    ToggleSkipFirst,
    FontMinus,
    FontPlus,
    ColorGray,
    ColorBlack,
    ColorNavy,
    ClearExisting,
    Apply,
    Cancel,
    InsideModal,
}

pub(super) struct PageNumberModalState {
    pub(super) config: PageNumberConfig,
    pub(super) preview_page: u16,
    pub(super) preview_cache: Option<(u16, u32, u32, Vec<u32>)>,
}

impl PageNumberModalState {
    pub(super) fn new(current_page: u16, roman_end: usize) -> Self {
        let mut cfg = PageNumberConfig::default();
        cfg.roman_end = roman_end as u16;
        Self {
            config: cfg,
            preview_page: current_page,
            preview_cache: None,
        }
    }
}

#[derive(Clone, Copy)]
struct ModalBox<T: Copy> {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: T,
}

impl<T: Copy> ModalBox<T> {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

struct SplitModalLayout<T: Copy> {
    mx: i32,
    my: i32,
    mw: u32,
    mh: u32,
    left_w: u32,
    panel_x: i32,
    panel_w: u32,
    hits: Vec<ModalBox<T>>,
}

impl<'a> PdfViewerApp<'a> {
    /// Renders a clean single-page preview bitmap for the Header/Footer or Page Numbering modals.
    fn render_doc_tool_preview_page(
        &self,
        page_idx: u16,
        max_w: u32,
        max_h: u32,
    ) -> Option<(u32, u32, Vec<u32>)> {
        let doc = self.doc.as_ref()?;
        let page = doc.pages().get(page_idx).ok()?;
        let pw = page.width().value.max(1.0);
        let ph = page.height().value.max(1.0);
        let scale = (max_w as f32 / pw).min(max_h as f32 / ph);
        let rw = (pw * scale).round().max(40.0) as u32;
        let rh = (ph * scale).round().max(40.0) as u32;
        let cfg = PdfRenderConfig::new()
            .set_fixed_width(rw as i32)
            .set_fixed_height(rh as i32)
            .render_form_data(false)
            .render_annotations(true)
            .limit_render_image_cache_size(true);
        let bmp = page.render_with_config(&cfg).ok()?;
        let img = bmp.as_image().to_rgba8();
        let (bw, bh) = (img.width(), img.height());
        let raw = img.into_raw();
        let mut pixels = Vec::with_capacity((bw * bh) as usize);
        for px in raw.chunks_exact(4) {
            pixels.push(((px[0] as u32) << 16) | ((px[1] as u32) << 8) | (px[2] as u32));
        }
        Some((bw, bh, pixels))
    }

    // =========================================================================
    // HEADER & FOOTER MODAL
    // =========================================================================

    pub(super) fn open_header_footer_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to edit Headers & Footers".to_string());
            self.request_redraw();
            return;
        }
        self.open_menu = None;
        self.show_help_overlay = false;
        self.show_about_overlay = false;
        self.show_customize_toolbar = false;
        self.show_notifications = false;
        self.show_reading_mode_popover = false;
        self.print_preview = None;
        self.combine_modal = None;
        self.page_number_modal = None;
        self.input_mode = InputMode::Normal;
        self.header_footer_modal = Some(HeaderFooterModalState::new(
            self.current_page_index,
            &self.last_hf_profile,
            &self.hf_profiles,
        ));
        self.request_redraw();
    }

    fn layout_header_footer_modal(&self, buf_w: u32, buf_h: u32, sf: f64) -> SplitModalLayout<HeaderFooterHit> {
        let s = sf as f32;
        let mw = ((920.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let mh = ((640.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;
        let panel_w = ((370.0 * s).round() as u32).min(mw.saturating_sub(200));
        let left_w = mw.saturating_sub(panel_w);
        let panel_x = mx + left_w as i32;

        let p = (14.0 * s).round() as i32;
        let ix = panel_x + p;
        let iw = panel_w.saturating_sub((p * 2) as u32);
        let gap = (6.0 * s).round() as i32;
        let row_h = (25.0 * s).round() as u32;
        let lbl_h = (15.0 * s).round() as i32;
        let sec_gap = (8.0 * s).round() as i32;

        let mut hits = Vec::with_capacity(28);

        // Left pane bottom page stepper
        let step_w = (220.0 * s).round().min(left_w as f32 - 24.0) as u32;
        let step_h = (28.0 * s).round() as u32;
        let step_x = mx + ((left_w.saturating_sub(step_w)) / 2) as i32;
        let step_y = my + mh as i32 - (44.0 * s).round() as i32;
        let nav_w = (28.0 * s).round() as u32;
        hits.push(ModalBox { x: step_x, y: step_y, w: nav_w, h: step_h, hit: HeaderFooterHit::PrevPage });
        hits.push(ModalBox { x: step_x + step_w as i32 - nav_w as i32, y: step_y, w: nav_w, h: step_h, hit: HeaderFooterHit::NextPage });

        let mut cy = my + (40.0 * s).round() as i32;

        // 1. Style Profiles row (< Profile >) + Profile Name + Save / Del
        cy += lbl_h;
        let arr_w = (24.0 * s).round() as u32;
        let save_w = (46.0 * s).round() as u32;
        let del_w = (40.0 * s).round() as u32;
        let name_w = iw.saturating_sub(arr_w * 2 + save_w + del_w + (gap * 3) as u32);
        hits.push(ModalBox { x: ix, y: cy, w: arr_w, h: row_h, hit: HeaderFooterHit::ProfilePrev });
        hits.push(ModalBox { x: ix + arr_w as i32 + 2, y: cy, w: name_w, h: row_h, hit: HeaderFooterHit::FocusField(HfField::ProfileName) });
        let nx = ix + arr_w as i32 + name_w as i32 + 4;
        hits.push(ModalBox { x: nx, y: cy, w: arr_w, h: row_h, hit: HeaderFooterHit::ProfileNext });
        let sx = nx + arr_w as i32 + gap;
        hits.push(ModalBox { x: sx, y: cy, w: save_w, h: row_h, hit: HeaderFooterHit::ProfileSave });
        hits.push(ModalBox { x: sx + save_w as i32 + gap, y: cy, w: del_w, h: row_h, hit: HeaderFooterHit::ProfileDelete });
        cy += row_h as i32 + sec_gap;

        // 2. Page Range (All / Custom + input)
        cy += lbl_h;
        let pill_w = (62.0 * s).round() as u32;
        hits.push(ModalBox { x: ix, y: cy, w: pill_w, h: row_h, hit: HeaderFooterHit::ScopeAll });
        hits.push(ModalBox { x: ix + pill_w as i32 + gap, y: cy, w: pill_w, h: row_h, hit: HeaderFooterHit::ScopeCustom });
        let rx = ix + (pill_w as i32 + gap) * 2;
        let rw = iw.saturating_sub((pill_w + gap as u32) * 2);
        hits.push(ModalBox { x: rx, y: cy, w: rw, h: row_h, hit: HeaderFooterHit::FocusField(HfField::CustomRange) });
        cy += row_h as i32 + sec_gap;

        // 3. Header Fields (Left / Center / Right)
        cy += lbl_h;
        let col3 = (iw.saturating_sub((gap * 2) as u32)) / 3;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::FocusField(HfField::HeaderLeft) });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::FocusField(HfField::HeaderCenter) });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(col3 * 2 + (gap * 2) as u32), h: row_h, hit: HeaderFooterHit::FocusField(HfField::HeaderRight) });
        cy += row_h as i32 + sec_gap;

        // 4. Footer Fields (Left / Center / Right)
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::FocusField(HfField::FooterLeft) });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::FocusField(HfField::FooterCenter) });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(col3 * 2 + (gap * 2) as u32), h: row_h, hit: HeaderFooterHit::FocusField(HfField::FooterRight) });
        cy += row_h as i32 + sec_gap;

        // 5. Font Size & Edge Margin row
        cy += lbl_h;
        let half_w = (iw.saturating_sub(gap as u32)) / 2;
        let sm_btn = (24.0 * s).round() as u32;
        hits.push(ModalBox { x: ix, y: cy, w: sm_btn, h: row_h, hit: HeaderFooterHit::FontMinus });
        hits.push(ModalBox { x: ix + half_w as i32 - sm_btn as i32, y: cy, w: sm_btn, h: row_h, hit: HeaderFooterHit::FontPlus });
        let mx2 = ix + half_w as i32 + gap;
        hits.push(ModalBox { x: mx2, y: cy, w: sm_btn, h: row_h, hit: HeaderFooterHit::MarginMinus });
        hits.push(ModalBox { x: ix + iw as i32 - sm_btn as i32, y: cy, w: sm_btn, h: row_h, hit: HeaderFooterHit::MarginPlus });
        cy += row_h as i32 + sec_gap;

        // 6. Text Color (Slate Gray / Black / Navy)
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::ColorGray });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::ColorBlack });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(col3 * 2 + (gap * 2) as u32), h: row_h, hit: HeaderFooterHit::ColorNavy });
        cy += row_h as i32 + sec_gap;

        // 7. Update / Replace Existing Header/Footer toggle
        hits.push(ModalBox { x: ix, y: cy, w: iw, h: row_h, hit: HeaderFooterHit::ToggleReplace });
        cy += row_h as i32 + sec_gap + (4.0 * s).round() as i32;

        // 8. Clear Existing Header / Footer section (3 buttons)
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::ClearHeader });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: HeaderFooterHit::ClearFooter });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(col3 * 2 + (gap * 2) as u32), h: row_h, hit: HeaderFooterHit::ClearBoth });

        // Bottom primary Apply & Cancel buttons
        let foot_h = (32.0 * s).round() as u32;
        let foot_y = my + mh as i32 - p - foot_h as i32;
        let cancel_w = (82.0 * s).round() as u32;
        let apply_w = iw.saturating_sub(cancel_w + gap as u32);
        hits.push(ModalBox { x: ix, y: foot_y, w: apply_w, h: foot_h, hit: HeaderFooterHit::Apply });
        hits.push(ModalBox { x: ix + apply_w as i32 + gap, y: foot_y, w: cancel_w, h: foot_h, hit: HeaderFooterHit::Cancel });

        SplitModalLayout { mx, my, mw, mh, left_w, panel_x, panel_w, hits }
    }

    pub(super) fn hit_test_header_footer_modal(&self, px: i32, py: i32, buf_w: u32, buf_h: u32, sf: f64) -> Option<HeaderFooterHit> {
        self.header_footer_modal.as_ref()?;
        let layout = self.layout_header_footer_modal(buf_w, buf_h, sf);
        for h in &layout.hits {
            if h.contains(px, py) {
                return Some(h.hit);
            }
        }
        if px >= layout.mx && px < layout.mx + layout.mw as i32 && py >= layout.my && py < layout.my + layout.mh as i32 {
            Some(HeaderFooterHit::InsideModal)
        } else {
            None
        }
    }

    pub(super) fn draw_header_footer_modal(&mut self, buffer: &mut [u32], buf_w: u32, buf_h: u32, sf: f64) {
        if self.header_footer_modal.is_none() {
            return;
        }
        let layout = self.layout_header_footer_modal(buf_w, buf_h, sf);
        let s = sf as f32;
        let avail_w = layout.left_w.saturating_sub((48.0 * s).round() as u32).max(80);
        let avail_h = layout.mh.saturating_sub((104.0 * s).round() as u32).max(80);

        // Ensure preview page bitmap is cached
        let need_render = self.header_footer_modal.as_ref().is_some_and(|m| {
            m.preview_cache.as_ref().map(|c| c.0) != Some(m.preview_page)
        });
        if need_render {
            let p_idx = self.header_footer_modal.as_ref().unwrap().preview_page;
            if let Some((rw, rh, px)) = self.render_doc_tool_preview_page(p_idx, avail_w, avail_h) {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.preview_cache = Some((p_idx, rw, rh, px));
                }
            }
        }

        let Some(m) = self.header_footer_modal.as_ref() else { return };
        let theme = self.ui_theme();
        let total = self.total_pages().max(1);
        let p_pts = self
            .doc
            .as_ref()
            .and_then(|d| d.pages().get(m.preview_page).ok())
            .map(|p| (p.width().value.max(1.0), p.height().value.max(1.0)))
            .unwrap_or((595.28, 841.89));

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        let rad = (10.0 * s).clamp(6.0, 14.0);
        draw_soft_shadow(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, (16.0 * s).round() as i32, (6.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.surface_bg);
        fill_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.left_w, layout.mh, rad, theme.canvas_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x - (rad as i32), layout.my, rad as u32, layout.mh, theme.canvas_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x, layout.my + 1, 1, layout.mh.saturating_sub(2), theme.divider);
        stroke_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.border_subtle);

        // Left pane: Live Page Preview with real-time Header/Footer overlay!
        if let Some((_, rw, rh, ref pixels)) = m.preview_cache {
            let sx = layout.mx + ((layout.left_w.saturating_sub(rw)) / 2) as i32;
            let sy = layout.my + (20.0 * s).round() as i32 + ((avail_h.saturating_sub(rh)) / 2) as i32;
            draw_soft_shadow(buffer, buf_w, buf_h, sx, sy, rw, rh, 2.0, (8.0 * s).round() as i32, (3.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
            for y in 0..rh {
                let dy = sy + y as i32;
                if dy < 0 || dy >= buf_h as i32 {
                    continue;
                }
                let drow = (dy as u32 * buf_w) as usize;
                let srow = (y * rw) as usize;
                for x in 0..rw {
                    let dx = sx + x as i32;
                    if dx >= 0 && dx < buf_w as i32 {
                        buffer[drow + dx as usize] = pixels[srow + x as usize];
                    }
                }
            }
            stroke_rounded_rect(buffer, buf_w, buf_h, sx, sy, rw, rh, 1.0, theme.border_subtle);

            // Overlay live Header/Footer preview at exact scaled coordinates!
            let px_per_pt = rw as f32 / p_pts.0;
            let band_h = (55.0 * px_per_pt).round().max(10.0) as u32;
            if m.replace_existing && m.profile.has_header() {
                fill_rect_clipped(buffer, buf_w, buf_h, sx + 1, sy + 1, rw.saturating_sub(2), band_h.min(rh), 0x00FF_FFFF);
                // Subtle dashed guide line showing header zone
                fill_rect_clipped(buffer, buf_w, buf_h, sx + 4, sy + band_h as i32, rw.saturating_sub(8), 1, 0x00BF_DBFE);
            }
            if m.replace_existing && m.profile.has_footer() {
                let fy = sy + rh as i32 - band_h as i32;
                fill_rect_clipped(buffer, buf_w, buf_h, sx + 1, fy.max(sy), rw.saturating_sub(2), band_h.min(rh), 0x00FF_FFFF);
                fill_rect_clipped(buffer, buf_w, buf_h, sx + 4, fy.max(sy), rw.saturating_sub(8), 1, 0x00BF_DBFE);
            }

            let preview_fs = (m.profile.font_size * px_per_pt).clamp(7.0, 22.0);
            let side_px = (36.0 * px_per_pt).round() as i32;
            let edge_px = (m.profile.margin_pt * px_per_pt).round() as i32;
            let txt_col = m.profile.color.ui_rgb();
            let pnum = m.preview_page + 1;
            let draw_slot = |buffer: &mut [u32], pos: MarginPosition, raw: &str| {
                let exp = PdfEngine::expand_hf_tokens(raw.trim(), pnum, total);
                if exp.is_empty() {
                    return;
                }
                let tw = measure_text(&exp, preview_fs, false);
                let tx = match pos {
                    MarginPosition::HeaderLeft | MarginPosition::FooterLeft => sx + side_px,
                    MarginPosition::HeaderCenter | MarginPosition::FooterCenter => {
                        sx + ((rw as f32 - tw) * 0.5).round() as i32
                    }
                    MarginPosition::HeaderRight | MarginPosition::FooterRight => {
                        sx + rw as i32 - side_px - tw.ceil() as i32
                    }
                };
                let ty = if pos.is_header() {
                    sy + edge_px
                } else {
                    sy + rh as i32 - edge_px - preview_fs.ceil() as i32
                };
                draw_text(buffer, buf_w, buf_h, tx, ty, &exp, preview_fs, txt_col, false);
            };
            draw_slot(buffer, MarginPosition::HeaderLeft, &m.profile.header_left);
            draw_slot(buffer, MarginPosition::HeaderCenter, &m.profile.header_center);
            draw_slot(buffer, MarginPosition::HeaderRight, &m.profile.header_right);
            draw_slot(buffer, MarginPosition::FooterLeft, &m.profile.footer_left);
            draw_slot(buffer, MarginPosition::FooterCenter, &m.profile.footer_center);
            draw_slot(buffer, MarginPosition::FooterRight, &m.profile.footer_right);
        }

        let title_sz = (14.0 * s).clamp(11.0, 22.0);
        let lbl_sz = (10.5 * s).clamp(9.0, 17.0);
        let body_sz = (11.5 * s).clamp(9.5, 18.0);
        let lbl_h = (15.0 * s).round() as i32;
        let pill_rad = (5.0 * s).clamp(3.0, 8.0);
        let ic_sz = (11.0 * s).round() as u32;
        let p = (14.0 * s).round() as i32;
        let ix = layout.panel_x + p;

        let find = |hit: HeaderFooterHit| layout.hits.iter().copied().find(|b| b.hit == hit);

        // Page stepper pill
        if let (Some(lp), Some(rp)) = (find(HeaderFooterHit::PrevPage), find(HeaderFooterHit::NextPage)) {
            let sw = (rp.x + rp.w as i32 - lp.x) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, lp.x, lp.y, sw, lp.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, lp.x, lp.y, sw, lp.h, pill_rad, theme.border_subtle);
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronLeft, lp.x + ((lp.w.saturating_sub(ic_sz)) / 2) as i32, lp.y + ((lp.h.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronRight, rp.x + ((rp.w.saturating_sub(ic_sz)) / 2) as i32, rp.y + ((rp.h.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
            let lbl = format!("Preview Page {} / {}", m.preview_page + 1, total);
            let tw = measure_text(&lbl, body_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, lp.x + ((sw as f32 - tw) * 0.5).round() as i32, lp.y, lp.h, rp.x, &lbl, body_sz, theme.text_primary, false);
        }

        // Right panel header
        draw_text(buffer, buf_w, buf_h, ix, layout.my + (11.0 * s).round() as i32, "Header & Footer", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x + 1, layout.my + (35.0 * s).round() as i32, layout.panel_w.saturating_sub(2), 1, theme.divider);

        let draw_pill = |buffer: &mut [u32], r: ModalBox<HeaderFooterHit>, text: &str, active: bool| {
            let hov = self.hovered_widget == Some(UiHit::HeaderFooter(r.hit));
            let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
            let bd = if active { theme.accent } else { theme.border_subtle };
            let fg = if active { theme.accent_soft_fg } else { theme.text_primary };
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bd);
            let clipped = truncate_ellipsis(text, (r.w as f32 - 8.0).max(12.0), body_sz, active);
            let tw = measure_text(&clipped, body_sz, active);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + ((r.w as f32 - tw) * 0.5).round() as i32, r.y, r.h, r.x + r.w as i32 - 2, &clipped, body_sz, fg, active);
        };

        let draw_input = |buffer: &mut [u32], r: ModalBox<HeaderFooterHit>, field: HfField, val: &str, placeholder: &str| {
            let focused = m.focused_field == field;
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, if focused { theme.accent } else { theme.input_border });
            let show = if val.is_empty() && !focused {
                placeholder.to_string()
            } else if focused {
                format!("{val}|")
            } else {
                val.to_string()
            };
            let fg = if val.is_empty() && !focused { theme.text_muted } else { theme.text_primary };
            let clipped = truncate_ellipsis(&show, (r.w as f32 - 10.0).max(12.0), body_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + 6, r.y, r.h, r.x + r.w as i32 - 4, &clipped, body_sz, fg, false);
        };

        // 1. Style Profiles
        if let Some(r) = find(HeaderFooterHit::ProfilePrev) {
            let p_lbl = format!("Saved Style Profiles ({})", self.hf_profiles.len());
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, &p_lbl, lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, "<", false);
        }
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::ProfileName)) {
            draw_input(buffer, r, HfField::ProfileName, &m.profile.name, "Profile name");
        }
        if let Some(r) = find(HeaderFooterHit::ProfileNext) {
            draw_pill(buffer, r, ">", false);
        }
        if let Some(r) = find(HeaderFooterHit::ProfileSave) {
            draw_pill(buffer, r, "Save", false);
        }
        if let Some(r) = find(HeaderFooterHit::ProfileDelete) {
            draw_pill(buffer, r, "Del", false);
        }

        // 2. Page Range
        if let Some(r) = find(HeaderFooterHit::ScopeAll) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Page Range", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, "All", m.all_pages);
        }
        if let Some(r) = find(HeaderFooterHit::ScopeCustom) {
            draw_pill(buffer, r, "Custom", !m.all_pages);
        }
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::CustomRange)) {
            draw_input(buffer, r, HfField::CustomRange, &m.custom_range, "e.g. 1-5, 8-");
        }

        // 3. Header Fields
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::HeaderLeft)) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Header Text (tokens: {page}, {total})", lbl_sz, theme.text_muted, true);
            draw_input(buffer, r, HfField::HeaderLeft, &m.profile.header_left, "Left");
        }
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::HeaderCenter)) {
            draw_input(buffer, r, HfField::HeaderCenter, &m.profile.header_center, "Center");
        }
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::HeaderRight)) {
            draw_input(buffer, r, HfField::HeaderRight, &m.profile.header_right, "Right");
        }

        // 4. Footer Fields
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::FooterLeft)) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Footer Text (Left / Center / Right)", lbl_sz, theme.text_muted, true);
            draw_input(buffer, r, HfField::FooterLeft, &m.profile.footer_left, "Left");
        }
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::FooterCenter)) {
            draw_input(buffer, r, HfField::FooterCenter, &m.profile.footer_center, "Center");
        }
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::FooterRight)) {
            draw_input(buffer, r, HfField::FooterRight, &m.profile.footer_right, "Right");
        }

        // 5. Font Size & Edge Margin
        if let (Some(fm), Some(fp)) = (find(HeaderFooterHit::FontMinus), find(HeaderFooterHit::FontPlus)) {
            draw_text(buffer, buf_w, buf_h, fm.x, fm.y - lbl_h, "Font Size", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, fm, "-", false);
            draw_pill(buffer, fp, "+", false);
            let txt = format!("{:.0} pt", m.profile.font_size);
            let tw = measure_text(&txt, body_sz, true);
            let cx = fm.x + fm.w as i32 + (((fp.x - (fm.x + fm.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, fm.y, fm.h, fp.x, &txt, body_sz, theme.text_primary, true);
        }
        if let (Some(mm), Some(mp)) = (find(HeaderFooterHit::MarginMinus), find(HeaderFooterHit::MarginPlus)) {
            draw_text(buffer, buf_w, buf_h, mm.x, mm.y - lbl_h, "Edge Margin", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, mm, "-", false);
            draw_pill(buffer, mp, "+", false);
            let txt = format!("{:.0} pt", m.profile.margin_pt);
            let tw = measure_text(&txt, body_sz, true);
            let cx = mm.x + mm.w as i32 + (((mp.x - (mm.x + mm.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, mm.y, mm.h, mp.x, &txt, body_sz, theme.text_primary, true);
        }

        // 6. Color
        if let Some(r) = find(HeaderFooterHit::ColorGray) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Text Color", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, HeaderFooterColor::Gray.label(), m.profile.color == HeaderFooterColor::Gray);
        }
        if let Some(r) = find(HeaderFooterHit::ColorBlack) {
            draw_pill(buffer, r, HeaderFooterColor::Black.label(), m.profile.color == HeaderFooterColor::Black);
        }
        if let Some(r) = find(HeaderFooterHit::ColorNavy) {
            draw_pill(buffer, r, HeaderFooterColor::Navy.label(), m.profile.color == HeaderFooterColor::Navy);
        }

        // 7. Update/Replace existing toggle
        if let Some(r) = find(HeaderFooterHit::ToggleReplace) {
            let txt = if m.replace_existing {
                "✓ Replace / Update Existing Header & Footer"
            } else {
                "Replace / Update Existing Header & Footer"
            };
            draw_pill(buffer, r, txt, m.replace_existing);
        }

        // 8. Clear Existing Header / Footer
        if let Some(r) = find(HeaderFooterHit::ClearHeader) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Clear Existing Header / Footer", lbl_sz, theme.danger, true);
            draw_pill(buffer, r, "Clear Header", false);
        }
        if let Some(r) = find(HeaderFooterHit::ClearFooter) {
            draw_pill(buffer, r, "Clear Footer", false);
        }
        if let Some(r) = find(HeaderFooterHit::ClearBoth) {
            draw_pill(buffer, r, "Clear Both", false);
        }

        // Error message if any
        if let Some(ref err) = m.error_msg {
            if let Some(ap) = find(HeaderFooterHit::Apply) {
                draw_text(buffer, buf_w, buf_h, ix, ap.y - (18.0 * s).round() as i32, err, lbl_sz, theme.danger, false);
            }
        }

        // Primary Apply & Cancel buttons
        if let Some(r) = find(HeaderFooterHit::Apply) {
            let hov = self.hovered_widget == Some(UiHit::HeaderFooter(HeaderFooterHit::Apply));
            let bg = if hov { theme.accent_hover } else { theme.accent };
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bg);
            let txt = "Apply Header / Footer";
            let tw = measure_text(txt, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + ((r.w as f32 - tw) * 0.5).round() as i32, r.y, r.h, r.x + r.w as i32 - 4, txt, body_sz, 0x00FF_FFFF, true);
        }
        if let Some(r) = find(HeaderFooterHit::Cancel) {
            draw_pill(buffer, r, "Cancel", false);
        }
    }

    pub(super) fn handle_header_footer_click(&mut self, hit: HeaderFooterHit) {
        let total = self.total_pages().max(1);
        match hit {
            HeaderFooterHit::InsideModal => return,
            HeaderFooterHit::Cancel => {
                self.header_footer_modal = None;
                self.request_redraw();
                return;
            }
            HeaderFooterHit::PrevPage => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.preview_page = m.preview_page.saturating_sub(1);
                }
            }
            HeaderFooterHit::NextPage => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.preview_page = (m.preview_page + 1).min(total.saturating_sub(1));
                }
            }
            HeaderFooterHit::ProfilePrev | HeaderFooterHit::ProfileNext => {
                if !self.hf_profiles.is_empty() {
                    if let Some(m) = self.header_footer_modal.as_mut() {
                        let len = self.hf_profiles.len();
                        let next_idx = match m.selected_profile_idx {
                            None => 0,
                            Some(i) if hit == HeaderFooterHit::ProfileNext => (i + 1) % len,
                            Some(i) => (i + len - 1) % len,
                        };
                        m.selected_profile_idx = Some(next_idx);
                        m.profile = self.hf_profiles[next_idx].clone();
                        m.error_msg = None;
                    }
                }
            }
            HeaderFooterHit::ProfileSave => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    let name = m.profile.name.trim().to_string();
                    if name.is_empty() {
                        m.error_msg = Some("Enter a profile name to save.".to_string());
                    } else {
                        m.profile.name = name.clone();
                        if let Some(pos) = self.hf_profiles.iter().position(|p| p.name.eq_ignore_ascii_case(&name)) {
                            self.hf_profiles[pos] = m.profile.clone();
                            m.selected_profile_idx = Some(pos);
                        } else {
                            self.hf_profiles.push(m.profile.clone());
                            m.selected_profile_idx = Some(self.hf_profiles.len() - 1);
                        }
                        self.last_hf_profile = m.profile.clone();
                        self.status_notice = Some(format!("Saved Header/Footer profile '{name}' to micropdf.ini"));
                        m.error_msg = None;
                        self.save_session_to_ini();
                    }
                }
            }
            HeaderFooterHit::ProfileDelete => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    if let Some(idx) = m.selected_profile_idx {
                        if idx < self.hf_profiles.len() {
                            let removed = self.hf_profiles.remove(idx);
                            m.selected_profile_idx = None;
                            self.status_notice = Some(format!("Deleted profile '{}'", removed.name));
                            self.save_session_to_ini();
                        }
                    }
                }
            }
            HeaderFooterHit::FocusField(f) => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.focused_field = f;
                    if f == HfField::CustomRange {
                        m.all_pages = false;
                    }
                }
            }
            HeaderFooterHit::ScopeAll => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.all_pages = true;
                    m.error_msg = None;
                }
            }
            HeaderFooterHit::ScopeCustom => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.all_pages = false;
                    m.focused_field = HfField::CustomRange;
                }
            }
            HeaderFooterHit::FontMinus => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.profile.font_size = (m.profile.font_size - 1.0).max(6.0);
                }
            }
            HeaderFooterHit::FontPlus => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.profile.font_size = (m.profile.font_size + 1.0).min(24.0);
                }
            }
            HeaderFooterHit::MarginMinus => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.profile.margin_pt = (m.profile.margin_pt - 2.0).max(12.0);
                }
            }
            HeaderFooterHit::MarginPlus => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.profile.margin_pt = (m.profile.margin_pt + 2.0).min(64.0);
                }
            }
            HeaderFooterHit::ColorGray => {
                if let Some(m) = self.header_footer_modal.as_mut() { m.profile.color = HeaderFooterColor::Gray; }
            }
            HeaderFooterHit::ColorBlack => {
                if let Some(m) = self.header_footer_modal.as_mut() { m.profile.color = HeaderFooterColor::Black; }
            }
            HeaderFooterHit::ColorNavy => {
                if let Some(m) = self.header_footer_modal.as_mut() { m.profile.color = HeaderFooterColor::Navy; }
            }
            HeaderFooterHit::ToggleReplace => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.replace_existing = !m.replace_existing;
                }
            }
            HeaderFooterHit::ClearHeader | HeaderFooterHit::ClearFooter | HeaderFooterHit::ClearBoth => {
                let pages = match self.resolve_hf_pages() {
                    Ok(p) => p,
                    Err(msg) => {
                        if let Some(m) = self.header_footer_modal.as_mut() { m.error_msg = Some(msg); }
                        self.request_redraw();
                        return;
                    }
                };
                self.push_undo_snapshot();
                let res = if let Some(doc) = self.doc.as_mut() {
                    let mut r = Ok(());
                    for &idx in &pages {
                        if let Ok(mut page) = doc.pages_mut().get(idx) {
                            if matches!(hit, HeaderFooterHit::ClearHeader | HeaderFooterHit::ClearBoth) {
                                if let Err(e) = PdfEngine::clear_margin_zone(&mut page, MarginZone::Header) {
                                    r = Err(e);
                                    break;
                                }
                            }
                            if matches!(hit, HeaderFooterHit::ClearFooter | HeaderFooterHit::ClearBoth) {
                                if let Err(e) = PdfEngine::clear_margin_zone(&mut page, MarginZone::Footer) {
                                    r = Err(e);
                                    break;
                                }
                            }
                        }
                    }
                    r
                } else {
                    Ok(())
                };
                match res {
                    Ok(()) => {
                        self.has_unsaved_edits = true;
                        self.invalidate_page_caches();
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            m.preview_cache = None;
                            m.error_msg = None;
                        }
                        let what = match hit {
                            HeaderFooterHit::ClearHeader => "Headers",
                            HeaderFooterHit::ClearFooter => "Footers",
                            _ => "Headers & Footers",
                        };
                        self.status_notice = Some(format!("Cleared existing {what} on {} page(s)", pages.len()));
                        self.update_title_bar();
                    }
                    Err(e) => {
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            m.error_msg = Some(format!("Clear failed: {e}"));
                        }
                    }
                }
            }
            HeaderFooterHit::Apply => {
                self.apply_header_footer_modal();
                return;
            }
        }
        self.request_redraw();
    }

    fn resolve_hf_pages(&self) -> Result<Vec<u16>, String> {
        let total = self.total_pages();
        let Some(m) = self.header_footer_modal.as_ref() else {
            return Err("No active modal".to_string());
        };
        if m.all_pages || m.custom_range.trim().is_empty() {
            return Ok((0..total).collect());
        }
        crate::print::parse_page_selection(&m.custom_range, total).map_err(|e| format!("{e}"))
    }

    pub(super) fn apply_header_footer_modal(&mut self) {
        let pages = match self.resolve_hf_pages() {
            Ok(p) => p,
            Err(msg) => {
                if let Some(m) = self.header_footer_modal.as_mut() { m.error_msg = Some(msg); }
                self.request_redraw();
                return;
            }
        };
        let Some((profile, replace_existing)) = self
            .header_footer_modal
            .as_ref()
            .map(|m| (m.profile.clone(), m.replace_existing))
        else {
            return;
        };
        if !profile.has_header() && !profile.has_footer() {
            if let Some(m) = self.header_footer_modal.as_mut() {
                m.error_msg = Some("Enter header/footer text or use Clear buttons.".to_string());
            }
            self.request_redraw();
            return;
        }

        self.push_undo_snapshot();
        let res = if let Some(doc) = self.doc.as_mut() {
            PdfEngine::apply_header_footer_full(doc, &profile, &pages, replace_existing)
        } else {
            Ok(())
        };
        match res {
            Ok(()) => {
                self.last_hf_profile = profile;
                self.has_unsaved_edits = true;
                self.header_footer_modal = None;
                self.invalidate_page_caches();
                self.status_notice = Some(format!(
                    "Applied Header/Footer to {} page(s) (press S to save)",
                    pages.len()
                ));
                self.update_title_bar();
            }
            Err(e) => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.error_msg = Some(format!("Failed: {e}"));
                }
            }
        }
        self.request_redraw();
    }

    // =========================================================================
    // PAGE NUMBERING MODAL
    // =========================================================================

    pub(super) fn open_page_number_modal(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to add Page Numbers".to_string());
            self.request_redraw();
            return;
        }
        self.open_menu = None;
        self.show_help_overlay = false;
        self.show_about_overlay = false;
        self.show_customize_toolbar = false;
        self.show_notifications = false;
        self.show_reading_mode_popover = false;
        self.print_preview = None;
        self.combine_modal = None;
        self.header_footer_modal = None;
        self.input_mode = InputMode::Normal;
        self.page_number_modal = Some(PageNumberModalState::new(
            self.current_page_index,
            self.roman_end,
        ));
        self.request_redraw();
    }

    fn layout_page_number_modal(&self, buf_w: u32, buf_h: u32, sf: f64) -> SplitModalLayout<PageNumberHit> {
        let s = sf as f32;
        let mw = ((860.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let mh = ((570.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;
        let panel_w = ((340.0 * s).round() as u32).min(mw.saturating_sub(200));
        let left_w = mw.saturating_sub(panel_w);
        let panel_x = mx + left_w as i32;

        let p = (14.0 * s).round() as i32;
        let ix = panel_x + p;
        let iw = panel_w.saturating_sub((p * 2) as u32);
        let gap = (6.0 * s).round() as i32;
        let row_h = (26.0 * s).round() as u32;
        let lbl_h = (15.0 * s).round() as i32;
        let sec_gap = (9.0 * s).round() as i32;

        let mut hits = Vec::with_capacity(24);

        // Left pane bottom page stepper
        let step_w = (220.0 * s).round().min(left_w as f32 - 24.0) as u32;
        let step_h = (28.0 * s).round() as u32;
        let step_x = mx + ((left_w.saturating_sub(step_w)) / 2) as i32;
        let step_y = my + mh as i32 - (44.0 * s).round() as i32;
        let nav_w = (28.0 * s).round() as u32;
        hits.push(ModalBox { x: step_x, y: step_y, w: nav_w, h: step_h, hit: PageNumberHit::PrevPage });
        hits.push(ModalBox { x: step_x + step_w as i32 - nav_w as i32, y: step_y, w: nav_w, h: step_h, hit: PageNumberHit::NextPage });

        let mut cy = my + (42.0 * s).round() as i32;
        let col3 = (iw.saturating_sub((gap * 2) as u32)) / 3;
        let col3_last = iw.saturating_sub(col3 * 2 + (gap * 2) as u32);

        // 1. Position (2×3 grid)
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: PageNumberHit::Pos(MarginPosition::HeaderLeft) });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: PageNumberHit::Pos(MarginPosition::HeaderCenter) });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: col3_last, h: row_h, hit: PageNumberHit::Pos(MarginPosition::HeaderRight) });
        cy += row_h as i32 + gap;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: PageNumberHit::Pos(MarginPosition::FooterLeft) });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: PageNumberHit::Pos(MarginPosition::FooterCenter) });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: col3_last, h: row_h, hit: PageNumberHit::Pos(MarginPosition::FooterRight) });
        cy += row_h as i32 + sec_gap;

        // 2. Numbering Format (row of 3 + row of 2)
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: PageNumberHit::Format(PageNumberFormat::Plain) });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: PageNumberHit::Format(PageNumberFormat::PageN) });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: col3_last, h: row_h, hit: PageNumberHit::Format(PageNumberFormat::PageNOfTotal) });
        cy += row_h as i32 + gap;
        let half_w = (iw.saturating_sub(gap as u32)) / 2;
        hits.push(ModalBox { x: ix, y: cy, w: half_w, h: row_h, hit: PageNumberHit::Format(PageNumberFormat::NSlashTotal) });
        hits.push(ModalBox { x: ix + half_w as i32 + gap, y: cy, w: iw.saturating_sub(half_w + gap as u32), h: row_h, hit: PageNumberHit::Format(PageNumberFormat::Dashed) });
        cy += row_h as i32 + sec_gap;

        // 3. Roman Front-Matter & Font Size row
        cy += lbl_h;
        let sm_btn = (26.0 * s).round() as u32;
        hits.push(ModalBox { x: ix, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::RomanMinus });
        hits.push(ModalBox { x: ix + half_w as i32 - sm_btn as i32, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::RomanPlus });
        let fx = ix + half_w as i32 + gap;
        hits.push(ModalBox { x: fx, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::FontMinus });
        hits.push(ModalBox { x: ix + iw as i32 - sm_btn as i32, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::FontPlus });
        cy += row_h as i32 + sec_gap;

        // 4. Text Color
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: PageNumberHit::ColorGray });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: PageNumberHit::ColorBlack });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: col3_last, h: row_h, hit: PageNumberHit::ColorNavy });
        cy += row_h as i32 + sec_gap;

        // 5. Skip Page 1 toggle
        hits.push(ModalBox { x: ix, y: cy, w: iw, h: row_h, hit: PageNumberHit::ToggleSkipFirst });
        cy += row_h as i32 + sec_gap;

        // 6. Clear Existing Page Numbers
        hits.push(ModalBox { x: ix, y: cy, w: iw, h: row_h, hit: PageNumberHit::ClearExisting });

        // Bottom Apply & Cancel
        let foot_h = (32.0 * s).round() as u32;
        let foot_y = my + mh as i32 - p - foot_h as i32;
        let cancel_w = (80.0 * s).round() as u32;
        let apply_w = iw.saturating_sub(cancel_w + gap as u32);
        hits.push(ModalBox { x: ix, y: foot_y, w: apply_w, h: foot_h, hit: PageNumberHit::Apply });
        hits.push(ModalBox { x: ix + apply_w as i32 + gap, y: foot_y, w: cancel_w, h: foot_h, hit: PageNumberHit::Cancel });

        SplitModalLayout { mx, my, mw, mh, left_w, panel_x, panel_w, hits }
    }

    pub(super) fn hit_test_page_number_modal(&self, px: i32, py: i32, buf_w: u32, buf_h: u32, sf: f64) -> Option<PageNumberHit> {
        self.page_number_modal.as_ref()?;
        let layout = self.layout_page_number_modal(buf_w, buf_h, sf);
        for h in &layout.hits {
            if h.contains(px, py) {
                return Some(h.hit);
            }
        }
        if px >= layout.mx && px < layout.mx + layout.mw as i32 && py >= layout.my && py < layout.my + layout.mh as i32 {
            Some(PageNumberHit::InsideModal)
        } else {
            None
        }
    }

    pub(super) fn draw_page_number_modal(&mut self, buffer: &mut [u32], buf_w: u32, buf_h: u32, sf: f64) {
        if self.page_number_modal.is_none() {
            return;
        }
        let layout = self.layout_page_number_modal(buf_w, buf_h, sf);
        let s = sf as f32;
        let avail_w = layout.left_w.saturating_sub((48.0 * s).round() as u32).max(80);
        let avail_h = layout.mh.saturating_sub((104.0 * s).round() as u32).max(80);

        let need_render = self.page_number_modal.as_ref().is_some_and(|m| {
            m.preview_cache.as_ref().map(|c| c.0) != Some(m.preview_page)
        });
        if need_render {
            let p_idx = self.page_number_modal.as_ref().unwrap().preview_page;
            if let Some((rw, rh, px)) = self.render_doc_tool_preview_page(p_idx, avail_w, avail_h) {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.preview_cache = Some((p_idx, rw, rh, px));
                }
            }
        }

        let Some(m) = self.page_number_modal.as_ref() else { return };
        let theme = self.ui_theme();
        let total = self.total_pages().max(1);
        let p_pts = self
            .doc
            .as_ref()
            .and_then(|d| d.pages().get(m.preview_page).ok())
            .map(|p| (p.width().value.max(1.0), p.height().value.max(1.0)))
            .unwrap_or((595.28, 841.89));

        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);
        let rad = (10.0 * s).clamp(6.0, 14.0);
        draw_soft_shadow(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, (16.0 * s).round() as i32, (6.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
        fill_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.surface_bg);
        fill_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.left_w, layout.mh, rad, theme.canvas_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x - (rad as i32), layout.my, rad as u32, layout.mh, theme.canvas_bg);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x, layout.my + 1, 1, layout.mh.saturating_sub(2), theme.divider);
        stroke_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.border_subtle);

        // Left pane live preview + live formatted page number badge
        if let Some((_, rw, rh, ref pixels)) = m.preview_cache {
            let sx = layout.mx + ((layout.left_w.saturating_sub(rw)) / 2) as i32;
            let sy = layout.my + (20.0 * s).round() as i32 + ((avail_h.saturating_sub(rh)) / 2) as i32;
            draw_soft_shadow(buffer, buf_w, buf_h, sx, sy, rw, rh, 2.0, (8.0 * s).round() as i32, (3.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha);
            for y in 0..rh {
                let dy = sy + y as i32;
                if dy < 0 || dy >= buf_h as i32 {
                    continue;
                }
                let drow = (dy as u32 * buf_w) as usize;
                let srow = (y * rw) as usize;
                for x in 0..rw {
                    let dx = sx + x as i32;
                    if dx >= 0 && dx < buf_w as i32 {
                        buffer[drow + dx as usize] = pixels[srow + x as usize];
                    }
                }
            }
            stroke_rounded_rect(buffer, buf_w, buf_h, sx, sy, rw, rh, 1.0, theme.border_subtle);

            let pnum = m.preview_page + 1;
            if let Some(label) = PdfEngine::format_page_number_label(&m.config, pnum, total) {
                let px_per_pt = rw as f32 / p_pts.0;
                let preview_fs = (m.config.font_size * px_per_pt).clamp(8.0, 22.0);
                let side_px = (36.0 * px_per_pt).round() as i32;
                let edge_px = (m.config.margin_pt * px_per_pt).round() as i32;
                let tw = measure_text(&label, preview_fs, false);
                let tx = match m.config.position {
                    MarginPosition::HeaderLeft | MarginPosition::FooterLeft => sx + side_px,
                    MarginPosition::HeaderCenter | MarginPosition::FooterCenter => {
                        sx + ((rw as f32 - tw) * 0.5).round() as i32
                    }
                    MarginPosition::HeaderRight | MarginPosition::FooterRight => {
                        sx + rw as i32 - side_px - tw.ceil() as i32
                    }
                };
                let ty = if m.config.position.is_header() {
                    sy + edge_px
                } else {
                    sy + rh as i32 - edge_px - preview_fs.ceil() as i32
                };
                // Subtle highlight box so user immediately spots the new page number placement
                fill_rounded_rect(buffer, buf_w, buf_h, tx - 5, ty - 3, tw.ceil() as u32 + 10, preview_fs.ceil() as u32 + 6, 3.0, 0x00EF_F6FF);
                stroke_rounded_rect(buffer, buf_w, buf_h, tx - 5, ty - 3, tw.ceil() as u32 + 10, preview_fs.ceil() as u32 + 6, 3.0, theme.accent);
                draw_text(buffer, buf_w, buf_h, tx, ty, &label, preview_fs, m.config.color.ui_rgb(), false);
            }
        }

        let title_sz = (14.0 * s).clamp(11.0, 22.0);
        let lbl_sz = (10.5 * s).clamp(9.0, 17.0);
        let body_sz = (11.5 * s).clamp(9.5, 18.0);
        let lbl_h = (15.0 * s).round() as i32;
        let pill_rad = (5.0 * s).clamp(3.0, 8.0);
        let ic_sz = (11.0 * s).round() as u32;
        let p = (14.0 * s).round() as i32;
        let ix = layout.panel_x + p;
        let find = |hit: PageNumberHit| layout.hits.iter().copied().find(|b| b.hit == hit);

        // Page stepper
        if let (Some(lp), Some(rp)) = (find(PageNumberHit::PrevPage), find(PageNumberHit::NextPage)) {
            let sw = (rp.x + rp.w as i32 - lp.x) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, lp.x, lp.y, sw, lp.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, lp.x, lp.y, sw, lp.h, pill_rad, theme.border_subtle);
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronLeft, lp.x + ((lp.w.saturating_sub(ic_sz)) / 2) as i32, lp.y + ((lp.h.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronRight, rp.x + ((rp.w.saturating_sub(ic_sz)) / 2) as i32, rp.y + ((rp.h.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
            let preview_tag = PdfEngine::format_page_number_label(&m.config, m.preview_page + 1, total)
                .unwrap_or_else(|| "Skipped".to_string());
            let lbl = format!("Page {} / {} ({preview_tag})", m.preview_page + 1, total);
            let tw = measure_text(&lbl, body_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, lp.x + ((sw as f32 - tw) * 0.5).round() as i32, lp.y, lp.h, rp.x, &lbl, body_sz, theme.text_primary, false);
        }

        draw_text(buffer, buf_w, buf_h, ix, layout.my + (11.0 * s).round() as i32, "Page Numbering", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x + 1, layout.my + (35.0 * s).round() as i32, layout.panel_w.saturating_sub(2), 1, theme.divider);

        let draw_pill = |buffer: &mut [u32], r: ModalBox<PageNumberHit>, text: &str, active: bool| {
            let hov = self.hovered_widget == Some(UiHit::PageNumber(r.hit));
            let bg = if active { theme.accent_soft_bg } else if hov { theme.hover_bg } else { theme.surface_elevated };
            let bd = if active { theme.accent } else { theme.border_subtle };
            let fg = if active { theme.accent_soft_fg } else { theme.text_primary };
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bd);
            let clipped = truncate_ellipsis(text, (r.w as f32 - 8.0).max(12.0), body_sz, active);
            let tw = measure_text(&clipped, body_sz, active);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + ((r.w as f32 - tw) * 0.5).round() as i32, r.y, r.h, r.x + r.w as i32 - 2, &clipped, body_sz, fg, active);
        };

        // 1. Position
        for pos in [
            MarginPosition::HeaderLeft, MarginPosition::HeaderCenter, MarginPosition::HeaderRight,
            MarginPosition::FooterLeft, MarginPosition::FooterCenter, MarginPosition::FooterRight,
        ] {
            if let Some(r) = find(PageNumberHit::Pos(pos)) {
                if pos == MarginPosition::HeaderLeft {
                    draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Position", lbl_sz, theme.text_muted, true);
                }
                draw_pill(buffer, r, pos.label(), m.config.position == pos);
            }
        }

        // 2. Format
        for fmt in PageNumberFormat::ALL {
            if let Some(r) = find(PageNumberHit::Format(fmt)) {
                if fmt == PageNumberFormat::Plain {
                    draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Numbering Style", lbl_sz, theme.text_muted, true);
                }
                draw_pill(buffer, r, fmt.label(), m.config.format == fmt);
            }
        }

        // 3. Roman Front-Matter & Font Size
        if let (Some(rm), Some(rp)) = (find(PageNumberHit::RomanMinus), find(PageNumberHit::RomanPlus)) {
            draw_text(buffer, buf_w, buf_h, rm.x, rm.y - lbl_h, "Roman Pages (i, ii...)", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, rm, "-", false);
            draw_pill(buffer, rp, "+", false);
            let txt = if m.config.roman_end == 0 { "None".to_string() } else { format!("1-{}", m.config.roman_end) };
            let tw = measure_text(&txt, body_sz, true);
            let cx = rm.x + rm.w as i32 + (((rp.x - (rm.x + rm.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, rm.y, rm.h, rp.x, &txt, body_sz, theme.text_primary, true);
        }
        if let (Some(fm), Some(fp)) = (find(PageNumberHit::FontMinus), find(PageNumberHit::FontPlus)) {
            draw_text(buffer, buf_w, buf_h, fm.x, fm.y - lbl_h, "Font Size", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, fm, "-", false);
            draw_pill(buffer, fp, "+", false);
            let txt = format!("{:.0} pt", m.config.font_size);
            let tw = measure_text(&txt, body_sz, true);
            let cx = fm.x + fm.w as i32 + (((fp.x - (fm.x + fm.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, fm.y, fm.h, fp.x, &txt, body_sz, theme.text_primary, true);
        }

        // 4. Color
        if let Some(r) = find(PageNumberHit::ColorGray) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Text Color", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, HeaderFooterColor::Gray.label(), m.config.color == HeaderFooterColor::Gray);
        }
        if let Some(r) = find(PageNumberHit::ColorBlack) {
            draw_pill(buffer, r, HeaderFooterColor::Black.label(), m.config.color == HeaderFooterColor::Black);
        }
        if let Some(r) = find(PageNumberHit::ColorNavy) {
            draw_pill(buffer, r, HeaderFooterColor::Navy.label(), m.config.color == HeaderFooterColor::Navy);
        }

        // 5. Skip Page 1
        if let Some(r) = find(PageNumberHit::ToggleSkipFirst) {
            let txt = if m.config.skip_first_page {
                "✓ Skip First Page (Cover Page)"
            } else {
                "Skip First Page (Cover Page)"
            };
            draw_pill(buffer, r, txt, m.config.skip_first_page);
        }

        // 6. Clear existing page numbers
        if let Some(r) = find(PageNumberHit::ClearExisting) {
            draw_pill(buffer, r, "Clear Existing Page Numbers", false);
        }

        // Apply & Cancel
        if let Some(r) = find(PageNumberHit::Apply) {
            let hov = self.hovered_widget == Some(UiHit::PageNumber(PageNumberHit::Apply));
            let bg = if hov { theme.accent_hover } else { theme.accent };
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bg);
            let txt = "Apply Page Numbers";
            let tw = measure_text(txt, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + ((r.w as f32 - tw) * 0.5).round() as i32, r.y, r.h, r.x + r.w as i32 - 4, txt, body_sz, 0x00FF_FFFF, true);
        }
        if let Some(r) = find(PageNumberHit::Cancel) {
            draw_pill(buffer, r, "Cancel", false);
        }
    }

    pub(super) fn handle_page_number_click(&mut self, hit: PageNumberHit) {
        let total = self.total_pages().max(1);
        match hit {
            PageNumberHit::InsideModal => return,
            PageNumberHit::Cancel => {
                self.page_number_modal = None;
                self.request_redraw();
                return;
            }
            PageNumberHit::PrevPage => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.preview_page = m.preview_page.saturating_sub(1);
                }
            }
            PageNumberHit::NextPage => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.preview_page = (m.preview_page + 1).min(total.saturating_sub(1));
                }
            }
            PageNumberHit::Pos(pos) => {
                if let Some(m) = self.page_number_modal.as_mut() { m.config.position = pos; }
            }
            PageNumberHit::Format(fmt) => {
                if let Some(m) = self.page_number_modal.as_mut() { m.config.format = fmt; }
            }
            PageNumberHit::RomanMinus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.roman_end = m.config.roman_end.saturating_sub(1);
                }
            }
            PageNumberHit::RomanPlus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.roman_end = (m.config.roman_end + 1).min(total);
                }
            }
            PageNumberHit::ToggleSkipFirst => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.skip_first_page = !m.config.skip_first_page;
                }
            }
            PageNumberHit::FontMinus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.font_size = (m.config.font_size - 1.0).max(6.0);
                }
            }
            PageNumberHit::FontPlus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.font_size = (m.config.font_size + 1.0).min(24.0);
                }
            }
            PageNumberHit::ColorGray => {
                if let Some(m) = self.page_number_modal.as_mut() { m.config.color = HeaderFooterColor::Gray; }
            }
            PageNumberHit::ColorBlack => {
                if let Some(m) = self.page_number_modal.as_mut() { m.config.color = HeaderFooterColor::Black; }
            }
            PageNumberHit::ColorNavy => {
                if let Some(m) = self.page_number_modal.as_mut() { m.config.color = HeaderFooterColor::Navy; }
            }
            PageNumberHit::ClearExisting => {
                let zone = self
                    .page_number_modal
                    .as_ref()
                    .map(|m| if m.config.position == MarginPosition::FooterCenter { MarginZone::PageNumber } else { m.config.position.zone() })
                    .unwrap_or(MarginZone::PageNumber);
                self.push_undo_snapshot();
                let res = if let Some(doc) = self.doc.as_mut() {
                    PdfEngine::clear_margin_zone_in_doc(doc, zone, 1, total)
                } else {
                    Ok(())
                };
                if res.is_ok() {
                    self.has_unsaved_edits = true;
                    self.invalidate_page_caches();
                    if let Some(m) = self.page_number_modal.as_mut() { m.preview_cache = None; }
                    self.status_notice = Some("Cleared existing page numbers".to_string());
                    self.update_title_bar();
                }
            }
            PageNumberHit::Apply => {
                self.apply_page_number_modal();
                return;
            }
        }
        self.request_redraw();
    }

    pub(super) fn apply_page_number_modal(&mut self) {
        let Some(cfg) = self.page_number_modal.as_ref().map(|m| m.config.clone()) else {
            return;
        };
        self.push_undo_snapshot();
        let res = if let Some(doc) = self.doc.as_mut() {
            PdfEngine::apply_page_numbers_full(doc, &cfg)
        } else {
            Ok(())
        };
        match res {
            Ok(()) => {
                self.roman_end = cfg.roman_end as usize;
                self.has_unsaved_edits = true;
                self.page_number_modal = None;
                self.invalidate_page_caches();
                self.status_notice = Some(format!(
                    "Applied Page Numbers ({}, {}) — press S to save",
                    cfg.position.label(),
                    cfg.format.label()
                ));
                self.update_title_bar();
            }
            Err(e) => {
                self.status_notice = Some(format!("Page Numbering failed: {e}"));
            }
        }
        self.request_redraw();
    }
}
