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
    ResetFields,
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
    /// When true (e.g. after Ctrl+A), next typed char replaces whole field and Backspace/Delete clears it.
    pub(super) select_all: bool,
    pub(super) preview_page: u16,
    /// `(page_idx, avail_w, avail_h, rw, rh, pixels)`
    pub(super) preview_cache: Option<(u16, u32, u32, u32, u32, Vec<u32>)>,
    /// Inline modal feedback banner: `Some((message, is_error))`.
    pub(super) status_msg: Option<(String, bool)>,
}

impl HeaderFooterModalState {
    /// Opens cleanly with blank fields so saved style profiles never auto-contaminate other PDFs.
    pub(super) fn new(current_page: u16) -> Self {
        let mut blank = HeaderFooterProfile::default();
        blank.name.clear();
        Self {
            profile: blank,
            selected_profile_idx: None,
            all_pages: true,
            custom_range: String::new(),
            replace_existing: true,
            focused_field: HfField::HeaderCenter,
            select_all: false,
            preview_page: current_page,
            preview_cache: None,
            status_msg: None,
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
pub(super) enum PnNumericFocus {
    SkipPages,
    RomanPages,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PageNumberHit {
    PrevPage,
    NextPage,
    Pos(MarginPosition),
    Format(PageNumberFormat),
    SkipMinus,
    SkipPlus,
    FocusSkip,
    RomanMinus,
    RomanPlus,
    FocusRoman,
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
    pub(super) focused_numeric: Option<PnNumericFocus>,
    pub(super) preview_page: u16,
    /// `(page_idx, avail_w, avail_h, rw, rh, pixels)`
    pub(super) preview_cache: Option<(u16, u32, u32, u32, u32, Vec<u32>)>,
    pub(super) status_msg: Option<(String, bool)>,
}

impl PageNumberModalState {
    pub(super) fn new(current_page: u16, roman_end: usize, skip_pages: usize) -> Self {
        let mut cfg = PageNumberConfig::default();
        cfg.roman_end = roman_end as u16;
        cfg.skip_pages = skip_pages as u16;
        Self {
            config: cfg,
            focused_numeric: None,
            preview_page: current_page,
            preview_cache: None,
            status_msg: None,
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
    /// Renders a full-page scaled preview bitmap for the Header/Footer or Page Numbering modals.
    /// Uses `set_target_width` + `set_maximum_height` so PDFium auto-scales the entire page to fit
    /// cleanly (never cropping to the top-left quarter at 1:1 pt scale).
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
        let rw = (pw * scale).round().max(40.0) as i32;
        let rh = (ph * scale).round().max(40.0) as i32;
        let cfg = PdfRenderConfig::new()
            .set_target_width(rw)
            .set_maximum_height(rh)
            .render_form_data(true)
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
        self.show_bookmark_format_overlay = false;
        self.show_about_overlay = false;
        self.show_customize_toolbar = false;
        self.show_notifications = false;
        self.show_reading_mode_popover = false;
        self.show_tab_list_popover = false;
        self.tab_context_menu = None;
        self.thumb_context_menu = None;
        self.print_preview = None;
        self.combine_modal = None;
        self.page_number_modal = None;
        self.input_mode = InputMode::Normal;
        self.header_footer_modal = Some(HeaderFooterModalState::new(self.current_page_index));
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

        // 1. Style Profiles row (< Profile Name > Save Del)
        cy += lbl_h;
        let arr_w = (24.0 * s).round() as u32;
        let save_w = (46.0 * s).round() as u32;
        let del_w = (40.0 * s).round() as u32;
        let name_w = iw.saturating_sub(arr_w * 2 + save_w + del_w + (gap * 3) as u32 + 4);
        hits.push(ModalBox { x: ix, y: cy, w: arr_w, h: row_h, hit: HeaderFooterHit::ProfilePrev });
        hits.push(ModalBox { x: ix + arr_w as i32 + 2, y: cy, w: name_w, h: row_h, hit: HeaderFooterHit::FocusField(HfField::ProfileName) });
        let nx = ix + arr_w as i32 + name_w as i32 + 4;
        hits.push(ModalBox { x: nx, y: cy, w: arr_w, h: row_h, hit: HeaderFooterHit::ProfileNext });
        let sx = nx + arr_w as i32 + gap;
        hits.push(ModalBox { x: sx, y: cy, w: save_w, h: row_h, hit: HeaderFooterHit::ProfileSave });
        hits.push(ModalBox { x: sx + save_w as i32 + gap, y: cy, w: del_w, h: row_h, hit: HeaderFooterHit::ProfileDelete });
        // Compact "Reset Fields" button aligned right on the Profile header line
        let rst_w = (76.0 * s).round() as u32;
        let rst_h = (18.0 * s).round() as u32;
        hits.push(ModalBox {
            x: ix + iw as i32 - rst_w as i32,
            y: cy - lbl_h - (2.0 * s).round() as i32,
            w: rst_w,
            h: rst_h,
            hit: HeaderFooterHit::ResetFields,
        });
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

        // Ensure preview page bitmap is cached for current page and modal viewport size
        let need_render = self.header_footer_modal.as_ref().is_some_and(|m| {
            m.preview_cache
                .as_ref()
                .map(|c| (c.0, c.1, c.2))
                != Some((m.preview_page, avail_w, avail_h))
        });
        if need_render {
            let p_idx = self.header_footer_modal.as_ref().unwrap().preview_page;
            if let Some((rw, rh, px)) = self.render_doc_tool_preview_page(p_idx, avail_w, avail_h) {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.preview_cache = Some((p_idx, avail_w, avail_h, rw, rh, px));
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
        if let Some((_, _, _, rw, rh, ref pixels)) = m.preview_cache {
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

            // Overlay live Header/Footer preview only when preview_page is in selected range!
            let page_in_scope = self
                .resolve_hf_pages()
                .map(|pages| pages.contains(&m.preview_page))
                .unwrap_or(true);
            if page_in_scope {
                let px_per_pt = rw as f32 / p_pts.0;
                let band_h = ((m.profile.margin_pt + m.profile.font_size + 14.0).clamp(55.0, 96.0)
                    * px_per_pt)
                    .round()
                    .max(10.0) as u32;
                if m.replace_existing && m.profile.has_header() {
                    fill_rect_clipped(buffer, buf_w, buf_h, sx + 1, sy + 1, rw.saturating_sub(2), band_h.min(rh), 0x00FF_FFFF);
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
            } else {
                let badge = "Outside selected page range";
                let b_sz = (10.5 * s).clamp(9.0, 16.0);
                let bw = measure_text(badge, b_sz, true).ceil() as u32 + (18.0 * s).round() as u32;
                let bh = (22.0 * s).round() as u32;
                let bx = sx + ((rw.saturating_sub(bw)) / 2) as i32;
                let by = sy + (10.0 * s).round() as i32;
                fill_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, (bh as f32) * 0.5, theme.surface_elevated);
                stroke_rounded_rect(buffer, buf_w, buf_h, bx, by, bw, bh, (bh as f32) * 0.5, theme.border_subtle);
                draw_text_vcenter(buffer, buf_w, buf_h, bx + (9.0 * s).round() as i32, by, bh, bx + bw as i32 - 4, badge, b_sz, theme.text_muted, true);
            }
        }

        let title_sz = (14.0 * s).clamp(11.0, 22.0);
        let lbl_sz = (10.5 * s).clamp(9.0, 17.0);
        let body_sz = (11.5 * s).clamp(9.5, 18.0);
        let small_sz = (10.0 * s).clamp(8.5, 16.0);
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
            let sel = focused && m.select_all && !val.is_empty();
            let bg = if sel { theme.accent_soft_bg } else { theme.surface_bg };
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, if focused { theme.accent } else { theme.input_border });
            let show = if val.is_empty() && !focused {
                placeholder.to_string()
            } else if focused && !sel {
                format!("{val}|")
            } else {
                val.to_string()
            };
            let fg = if val.is_empty() && !focused {
                theme.text_muted
            } else if sel {
                theme.accent_soft_fg
            } else {
                theme.text_primary
            };
            let clipped = truncate_ellipsis(&show, (r.w as f32 - 10.0).max(12.0), body_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + 6, r.y, r.h, r.x + r.w as i32 - 4, &clipped, body_sz, fg, false);
        };

        // 1. Style Profiles + Reset Fields
        if let Some(r) = find(HeaderFooterHit::ProfilePrev) {
            let p_lbl = format!("Saved Style Profiles ({})", self.hf_profiles.len());
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, &p_lbl, lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, "<", false);
        }
        if let Some(r) = find(HeaderFooterHit::ResetFields) {
            let hov = self.hovered_widget == Some(UiHit::HeaderFooter(r.hit));
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, (4.0 * s).max(3.0), if hov { theme.hover_bg } else { theme.surface_elevated });
            stroke_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, (4.0 * s).max(3.0), theme.border_subtle);
            let txt = "Reset Fields";
            let tw = measure_text(txt, small_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + ((r.w as f32 - tw) * 0.5).round() as i32, r.y, r.h, r.x + r.w as i32 - 2, txt, small_sz, theme.text_secondary, false);
        }
        if let Some(r) = find(HeaderFooterHit::FocusField(HfField::ProfileName)) {
            draw_input(buffer, r, HfField::ProfileName, &m.profile.name, "Profile name...");
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

        // Inline status/error message banner right above Apply/Cancel (always visible inside modal!)
        if let Some((ref msg, is_err)) = m.status_msg {
            if let Some(ap) = find(HeaderFooterHit::Apply) {
                let col = if is_err { theme.danger } else { theme.success };
                let max_w = (layout.panel_w.saturating_sub((p * 2) as u32)) as f32;
                let clipped = truncate_ellipsis(msg, max_w, lbl_sz, true);
                draw_text(buffer, buf_w, buf_h, ix, ap.y - (20.0 * s).round() as i32, &clipped, lbl_sz, col, true);
            }
        } else if let Some(ap) = find(HeaderFooterHit::Apply) {
            let hint = "Tip: Ctrl+A selects all, Delete clears focused field";
            let max_w = (layout.panel_w.saturating_sub((p * 2) as u32)) as f32;
            let clipped = truncate_ellipsis(hint, max_w, small_sz, false);
            draw_text(buffer, buf_w, buf_h, ix, ap.y - (18.0 * s).round() as i32, &clipped, small_sz, theme.text_muted, false);
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
            HeaderFooterHit::ResetFields => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    let mut blank = HeaderFooterProfile::default();
                    blank.name.clear();
                    m.profile = blank;
                    m.selected_profile_idx = None;
                    m.select_all = false;
                    m.status_msg = Some(("Cleared all text fields.".to_string(), false));
                }
            }
            HeaderFooterHit::ProfilePrev | HeaderFooterHit::ProfileNext => {
                if self.hf_profiles.is_empty() {
                    if let Some(m) = self.header_footer_modal.as_mut() {
                        m.status_msg = Some(("No saved profiles yet — enter a name and click Save.".to_string(), true));
                    }
                } else if let Some(m) = self.header_footer_modal.as_mut() {
                    let len = self.hf_profiles.len();
                    let next_idx = match m.selected_profile_idx {
                        None => 0,
                        Some(i) if hit == HeaderFooterHit::ProfileNext => (i + 1) % len,
                        Some(i) => (i + len - 1) % len,
                    };
                    m.selected_profile_idx = Some(next_idx);
                    m.profile = self.hf_profiles[next_idx].clone();
                    m.select_all = false;
                    m.status_msg = Some((
                        format!("Loaded profile '{}' ({}/{})", m.profile.name, next_idx + 1, len),
                        false,
                    ));
                }
            }
            HeaderFooterHit::ProfileSave => {
                let mut saved_name: Option<String> = None;
                if let Some(m) = self.header_footer_modal.as_mut() {
                    let mut name = m.profile.name.trim().to_string();
                    if name.is_empty() {
                        let mut n = self.hf_profiles.len() + 1;
                        while self
                            .hf_profiles
                            .iter()
                            .any(|p| p.name.eq_ignore_ascii_case(&format!("Profile {n}")))
                        {
                            n += 1;
                        }
                        name = format!("Profile {n}");
                    }
                    m.profile.name = name.clone();
                    if let Some(pos) = self
                        .hf_profiles
                        .iter()
                        .position(|p| p.name.eq_ignore_ascii_case(&name))
                    {
                        self.hf_profiles[pos] = m.profile.clone();
                        m.selected_profile_idx = Some(pos);
                    } else {
                        self.hf_profiles.push(m.profile.clone());
                        m.selected_profile_idx = Some(self.hf_profiles.len() - 1);
                    }
                    m.select_all = false;
                    m.status_msg = Some((
                        format!("Saved profile '{}' ({} saved)", name, self.hf_profiles.len()),
                        false,
                    ));
                    saved_name = Some(name);
                }
                if let Some(name) = saved_name {
                    self.status_notice = Some(format!("Saved Header/Footer profile '{name}' to micropdf.ini"));
                    self.save_session_to_ini();
                }
            }
            HeaderFooterHit::ProfileDelete => {
                let mut deleted_name: Option<String> = None;
                if let Some(m) = self.header_footer_modal.as_mut() {
                    let target_idx = m
                        .selected_profile_idx
                        .filter(|&i| i < self.hf_profiles.len())
                        .or_else(|| {
                            let typed = m.profile.name.trim();
                            if typed.is_empty() {
                                None
                            } else {
                                self.hf_profiles
                                    .iter()
                                    .position(|p| p.name.eq_ignore_ascii_case(typed))
                            }
                        });
                    if let Some(idx) = target_idx {
                        let removed = self.hf_profiles.remove(idx);
                        let mut blank = HeaderFooterProfile::default();
                        blank.name.clear();
                        m.profile = blank;
                        m.selected_profile_idx = None;
                        m.select_all = false;
                        m.status_msg = Some((
                            format!("Deleted profile '{}' ({} remaining)", removed.name, self.hf_profiles.len()),
                            false,
                        ));
                        deleted_name = Some(removed.name);
                    } else {
                        m.status_msg = Some((
                            "Select a saved profile using < > first to delete.".to_string(),
                            true,
                        ));
                    }
                }
                if let Some(name) = deleted_name {
                    self.last_hf_profile = HeaderFooterProfile::default();
                    self.status_notice = Some(format!("Deleted Header/Footer profile '{name}'"));
                    self.save_session_to_ini();
                }
            }
            HeaderFooterHit::FocusField(f) => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.focused_field = f;
                    m.select_all = false;
                    if f == HfField::CustomRange {
                        m.all_pages = false;
                    }
                }
            }
            HeaderFooterHit::ScopeAll => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.all_pages = true;
                    m.status_msg = None;
                }
            }
            HeaderFooterHit::ScopeCustom => {
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.all_pages = false;
                    m.focused_field = HfField::CustomRange;
                    m.select_all = false;
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
                        if let Some(m) = self.header_footer_modal.as_mut() { m.status_msg = Some((msg, true)); }
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
                        self.thumbnail_cache.clear();
                        let what = match hit {
                            HeaderFooterHit::ClearHeader => "Headers",
                            HeaderFooterHit::ClearFooter => "Footers",
                            _ => "Headers & Footers",
                        };
                        let done_msg = format!("Cleared existing {what} on {} page(s)", pages.len());
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            m.preview_cache = None;
                            m.status_msg = Some((done_msg.clone(), false));
                        }
                        self.status_notice = Some(done_msg);
                        self.update_title_bar();
                    }
                    Err(e) => {
                        let _ = self.undo_stack.pop_back();
                        if let Some(m) = self.header_footer_modal.as_mut() {
                            m.status_msg = Some((format!("Clear failed: {e}"), true));
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
                if let Some(m) = self.header_footer_modal.as_mut() { m.status_msg = Some((msg, true)); }
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
                m.status_msg = Some(("Enter header/footer text or use Clear buttons.".to_string(), true));
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
                self.thumbnail_cache.clear();
                self.status_notice = Some(format!(
                    "Applied Header/Footer to {} page(s) (press S to save)",
                    pages.len()
                ));
                self.update_title_bar();
            }
            Err(e) => {
                let _ = self.undo_stack.pop_back();
                if let Some(m) = self.header_footer_modal.as_mut() {
                    m.status_msg = Some((format!("Failed: {e}"), true));
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
        self.show_bookmark_format_overlay = false;
        self.show_about_overlay = false;
        self.show_customize_toolbar = false;
        self.show_notifications = false;
        self.show_reading_mode_popover = false;
        self.show_tab_list_popover = false;
        self.tab_context_menu = None;
        self.thumb_context_menu = None;
        self.print_preview = None;
        self.combine_modal = None;
        self.header_footer_modal = None;
        self.input_mode = InputMode::Normal;
        self.page_number_modal = Some(PageNumberModalState::new(
            self.current_page_index,
            self.roman_end,
            self.skip_pages,
        ));
        self.request_redraw();
    }

    fn layout_page_number_modal(&self, buf_w: u32, buf_h: u32, sf: f64) -> SplitModalLayout<PageNumberHit> {
        let s = sf as f32;
        let mw = ((940.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let mh = ((625.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;
        let panel_w = ((405.0 * s).round() as u32).min(mw.saturating_sub(200));
        let left_w = mw.saturating_sub(panel_w);
        let panel_x = mx + left_w as i32;

        let p = (14.0 * s).round() as i32;
        let ix = panel_x + p;
        let iw = panel_w.saturating_sub((p * 2) as u32);
        let gap = (6.0 * s).round() as i32;
        let row_h = (26.0 * s).round() as u32;
        let lbl_h = (15.0 * s).round() as i32;
        let sec_gap = (9.0 * s).round() as i32;

        let mut hits = Vec::with_capacity(28);

        // Left pane bottom page stepper
        let step_w = (250.0 * s).round().min(left_w as f32 - 24.0) as u32;
        let step_h = (28.0 * s).round() as u32;
        let step_x = mx + ((left_w.saturating_sub(step_w)) / 2) as i32;
        let step_y = my + mh as i32 - (44.0 * s).round() as i32;
        let nav_w = (28.0 * s).round() as u32;
        hits.push(ModalBox { x: step_x, y: step_y, w: nav_w, h: step_h, hit: PageNumberHit::PrevPage });
        hits.push(ModalBox { x: step_x + step_w as i32 - nav_w as i32, y: step_y, w: nav_w, h: step_h, hit: PageNumberHit::NextPage });

        let mut cy = my + (42.0 * s).round() as i32;
        let col3 = (iw.saturating_sub((gap * 2) as u32)) / 3;
        let col3_last = iw.saturating_sub(col3 * 2 + (gap * 2) as u32);

        // 1. Position (2×3 grid — wide enough for full "Bottom Left / Bottom Center / Bottom Right")
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

        // 3. Skip Front Pages (Cover/Pictures) & Roman Front-Matter row (both clickable to type digits!)
        cy += lbl_h;
        let sm_btn = (26.0 * s).round() as u32;
        let mid_w = half_w.saturating_sub(sm_btn * 2 + 4);
        hits.push(ModalBox { x: ix, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::SkipMinus });
        hits.push(ModalBox { x: ix + sm_btn as i32 + 2, y: cy, w: mid_w, h: row_h, hit: PageNumberHit::FocusSkip });
        hits.push(ModalBox { x: ix + half_w as i32 - sm_btn as i32, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::SkipPlus });
        let rx = ix + half_w as i32 + gap;
        let r_half = iw.saturating_sub(half_w + gap as u32);
        let r_mid_w = r_half.saturating_sub(sm_btn * 2 + 4);
        hits.push(ModalBox { x: rx, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::RomanMinus });
        hits.push(ModalBox { x: rx + sm_btn as i32 + 2, y: cy, w: r_mid_w, h: row_h, hit: PageNumberHit::FocusRoman });
        hits.push(ModalBox { x: ix + iw as i32 - sm_btn as i32, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::RomanPlus });
        cy += row_h as i32 + sec_gap;

        // 4. Font Size row
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::FontMinus });
        hits.push(ModalBox { x: ix + half_w as i32 - sm_btn as i32, y: cy, w: sm_btn, h: row_h, hit: PageNumberHit::FontPlus });
        cy += row_h as i32 + sec_gap;

        // 5. Text Color
        cy += lbl_h;
        hits.push(ModalBox { x: ix, y: cy, w: col3, h: row_h, hit: PageNumberHit::ColorGray });
        hits.push(ModalBox { x: ix + col3 as i32 + gap, y: cy, w: col3, h: row_h, hit: PageNumberHit::ColorBlack });
        hits.push(ModalBox { x: ix + (col3 as i32 + gap) * 2, y: cy, w: col3_last, h: row_h, hit: PageNumberHit::ColorNavy });
        cy += row_h as i32 + sec_gap + (4.0 * s).round() as i32;

        // 6. Clear Existing Page Numbers
        hits.push(ModalBox { x: ix, y: cy, w: iw, h: row_h, hit: PageNumberHit::ClearExisting });

        // Bottom Apply & Cancel
        let foot_h = (32.0 * s).round() as u32;
        let foot_y = my + mh as i32 - p - foot_h as i32;
        let cancel_w = (84.0 * s).round() as u32;
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
            m.preview_cache
                .as_ref()
                .map(|c| (c.0, c.1, c.2))
                != Some((m.preview_page, avail_w, avail_h))
        });
        if need_render {
            let p_idx = self.page_number_modal.as_ref().unwrap().preview_page;
            if let Some((rw, rh, px)) = self.render_doc_tool_preview_page(p_idx, avail_w, avail_h) {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.preview_cache = Some((p_idx, avail_w, avail_h, rw, rh, px));
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
        if let Some((_, _, _, rw, rh, ref pixels)) = m.preview_cache {
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
                fill_rounded_rect(buffer, buf_w, buf_h, tx - 5, ty - 3, tw.ceil() as u32 + 10, preview_fs.ceil() as u32 + 6, 3.0, 0x00EF_F6FF);
                stroke_rounded_rect(buffer, buf_w, buf_h, tx - 5, ty - 3, tw.ceil() as u32 + 10, preview_fs.ceil() as u32 + 6, 3.0, theme.accent);
                draw_text(buffer, buf_w, buf_h, tx, ty, &label, preview_fs, m.config.color.ui_rgb(), false);
            }
        }

        let title_sz = (14.0 * s).clamp(11.0, 22.0);
        let lbl_sz = (10.5 * s).clamp(9.0, 17.0);
        let body_sz = (11.5 * s).clamp(9.5, 18.0);
        let small_sz = (10.0 * s).clamp(8.5, 16.0);
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

        // 3. Skip Front Pages & Roman Front-Matter (clickable boxes support direct digit entry!)
        if let (Some(sm), Some(sf_box), Some(sp)) = (
            find(PageNumberHit::SkipMinus),
            find(PageNumberHit::FocusSkip),
            find(PageNumberHit::SkipPlus),
        ) {
            draw_text(buffer, buf_w, buf_h, sm.x, sm.y - lbl_h, "Skip Front Pages", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, sm, "-", false);
            draw_pill(buffer, sp, "+", false);
            let focused = m.focused_numeric == Some(PnNumericFocus::SkipPages);
            fill_rounded_rect(buffer, buf_w, buf_h, sf_box.x, sf_box.y, sf_box.w, sf_box.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, sf_box.x, sf_box.y, sf_box.w, sf_box.h, pill_rad, if focused { theme.accent } else { theme.input_border });
            let txt = if focused {
                format!("{}|", m.config.skip_pages)
            } else if m.config.skip_pages == 0 {
                "0 (None)".to_string()
            } else if m.config.skip_pages == 1 {
                "1 page".to_string()
            } else {
                format!("{} pages", m.config.skip_pages)
            };
            let tw = measure_text(&txt, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, sf_box.x + ((sf_box.w as f32 - tw) * 0.5).round() as i32, sf_box.y, sf_box.h, sf_box.x + sf_box.w as i32 - 2, &txt, body_sz, theme.text_primary, true);
        }
        if let (Some(rm), Some(rf_box), Some(rp)) = (
            find(PageNumberHit::RomanMinus),
            find(PageNumberHit::FocusRoman),
            find(PageNumberHit::RomanPlus),
        ) {
            draw_text(buffer, buf_w, buf_h, rm.x, rm.y - lbl_h, "Roman Pages (i, ii...)", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, rm, "-", false);
            draw_pill(buffer, rp, "+", false);
            let focused = m.focused_numeric == Some(PnNumericFocus::RomanPages);
            fill_rounded_rect(buffer, buf_w, buf_h, rf_box.x, rf_box.y, rf_box.w, rf_box.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, rf_box.x, rf_box.y, rf_box.w, rf_box.h, pill_rad, if focused { theme.accent } else { theme.input_border });
            let txt = if focused {
                format!("{}|", m.config.roman_end)
            } else if m.config.roman_end == 0 {
                "0 (None)".to_string()
            } else {
                format!("{} (i–{})", m.config.roman_end, PdfEngine::to_roman(m.config.roman_end))
            };
            let clipped = truncate_ellipsis(&txt, (rf_box.w as f32 - 6.0).max(16.0), body_sz, true);
            let tw = measure_text(&clipped, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, rf_box.x + ((rf_box.w as f32 - tw) * 0.5).round() as i32, rf_box.y, rf_box.h, rf_box.x + rf_box.w as i32 - 2, &clipped, body_sz, theme.text_primary, true);
        }

        // 4. Font Size
        if let (Some(fm), Some(fp)) = (find(PageNumberHit::FontMinus), find(PageNumberHit::FontPlus)) {
            draw_text(buffer, buf_w, buf_h, fm.x, fm.y - lbl_h, "Font Size", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, fm, "-", false);
            draw_pill(buffer, fp, "+", false);
            let txt = format!("{:.0} pt", m.config.font_size);
            let tw = measure_text(&txt, body_sz, true);
            let cx = fm.x + fm.w as i32 + (((fp.x - (fm.x + fm.w as i32)) as f32 - tw) * 0.5).round() as i32;
            draw_text_vcenter(buffer, buf_w, buf_h, cx, fm.y, fm.h, fp.x, &txt, body_sz, theme.text_primary, true);
            // Live summary on right half of the Font Size row
            let sum_x = fp.x + fp.w as i32 + (12.0 * s).round() as i32;
            let first_arabic = m.config.skip_pages + m.config.roman_end + 1;
            let summary = if first_arabic <= total {
                format!("Body p.1 starts at sheet #{first_arabic}")
            } else {
                "All pages skipped/Roman".to_string()
            };
            let max_sw = (layout.panel_x + layout.panel_w as i32 - p - sum_x).max(40) as f32;
            let clip_sum = truncate_ellipsis(&summary, max_sw, small_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, sum_x, fm.y, fm.h, layout.panel_x + layout.panel_w as i32 - p, &clip_sum, small_sz, theme.text_muted, false);
        }

        // 5. Color
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

        // 6. Clear existing page numbers
        if let Some(r) = find(PageNumberHit::ClearExisting) {
            draw_pill(buffer, r, "Clear Existing Page Numbers", false);
        }

        // Inline status banner
        if let Some((ref msg, is_err)) = m.status_msg {
            if let Some(ap) = find(PageNumberHit::Apply) {
                let col = if is_err { theme.danger } else { theme.success };
                let max_w = (layout.panel_w.saturating_sub((p * 2) as u32)) as f32;
                let clipped = truncate_ellipsis(msg, max_w, lbl_sz, true);
                draw_text(buffer, buf_w, buf_h, ix, ap.y - (20.0 * s).round() as i32, &clipped, lbl_sz, col, true);
            }
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
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.position = pos;
                    m.focused_numeric = None;
                }
            }
            PageNumberHit::Format(fmt) => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.format = fmt;
                    m.focused_numeric = None;
                }
            }
            PageNumberHit::FocusSkip => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.focused_numeric = Some(PnNumericFocus::SkipPages);
                }
            }
            PageNumberHit::FocusRoman => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.focused_numeric = Some(PnNumericFocus::RomanPages);
                }
            }
            PageNumberHit::SkipMinus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.skip_pages = m.config.skip_pages.saturating_sub(1);
                }
            }
            PageNumberHit::SkipPlus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.skip_pages = (m.config.skip_pages + 1).min(total.saturating_sub(1));
                    let max_r = total.saturating_sub(m.config.skip_pages);
                    m.config.roman_end = m.config.roman_end.min(max_r);
                }
            }
            PageNumberHit::RomanMinus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.config.roman_end = m.config.roman_end.saturating_sub(1);
                }
            }
            PageNumberHit::RomanPlus => {
                if let Some(m) = self.page_number_modal.as_mut() {
                    let max_r = total.saturating_sub(m.config.skip_pages);
                    m.config.roman_end = (m.config.roman_end + 1).min(max_r);
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
                match res {
                    Ok(()) => {
                        self.has_unsaved_edits = true;
                        self.invalidate_page_caches();
                        self.thumbnail_cache.clear();
                        if let Some(m) = self.page_number_modal.as_mut() {
                            m.preview_cache = None;
                            m.status_msg = Some(("Cleared existing page numbers.".to_string(), false));
                        }
                        self.status_notice = Some("Cleared existing page numbers".to_string());
                        self.update_title_bar();
                    }
                    Err(e) => {
                        let _ = self.undo_stack.pop_back();
                        if let Some(m) = self.page_number_modal.as_mut() {
                            m.status_msg = Some((format!("Clear failed: {e}"), true));
                        }
                    }
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
                self.skip_pages = cfg.skip_pages as usize;
                let total_p = self.total_pages() as usize;
                PdfEngine::resync_bookmarks_logical_pages_with_skip(
                    &mut self.bookmarks,
                    self.skip_pages,
                    self.roman_end,
                    total_p,
                );
                self.has_unsaved_edits = true;
                self.page_number_modal = None;
                self.invalidate_page_caches();
                self.thumbnail_cache.clear();
                self.status_notice = Some(format!(
                    "Applied Page Numbers ({}, {}) — press S to save",
                    cfg.position.label(),
                    cfg.format.label()
                ));
                self.update_title_bar();
            }
            Err(e) => {
                let _ = self.undo_stack.pop_back();
                if let Some(m) = self.page_number_modal.as_mut() {
                    m.status_msg = Some((format!("Page Numbering failed: {e}"), true));
                }
            }
        }
        self.request_redraw();
    }
}
