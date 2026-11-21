//! Split-view Print Preview modal with live paper sheet placement, printer selection,
//! page scope/parity/reverse, duplex (long/short edge + Guided Manual Duplex for simplex printers),
//! Booklet (saddle-stitch 2-up imposition), Collate (default ON), and Auto-Trim / Zoom margin cropping.
//! Holds zero RAM when closed and at most one screen-sized preview bitmap while open.

use crate::print::{
    build_manual_duplex_passes, compute_booklet_sheets, compute_effective_crop_box,
    place_page_on_printable_area, resolve_print_pages, BookletSheet, DuplexMode, PageParity,
    PrintOrientation, PrintPaperSize, PrintRenderOptions,
};
use pdfium_render::prelude::*;

use super::render::{blend_rect_rgba, fill_rect_clipped};
use super::state::*;
use super::ui_draw::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PrintPageScope {
    All,
    Current,
    Custom,
}

pub(super) struct CachedPreviewSheet {
    pub(super) key_left: Option<u16>,
    pub(super) key_right: Option<u16>,
    pub(super) booklet: bool,
    pub(super) render_w: u32,
    pub(super) render_h: u32,
    pub(super) auto_trim: bool,
    pub(super) zoom_percent: u16,
    pub(super) grayscale: bool,
    pub(super) pixels: Vec<u32>,
}

pub(super) struct PrintPreviewState {
    pub(super) printers: Vec<String>,
    pub(super) selected_printer: usize,
    pub(super) page_scope: PrintPageScope,
    pub(super) custom_range: String,
    pub(super) parity: PageParity,
    pub(super) reverse: bool,
    pub(super) copies: u16,
    pub(super) paper: PrintPaperSize,
    pub(super) duplex: DuplexMode,
    /// Guided two-pass duplex for simplex (single-sided) printers.
    pub(super) manual_duplex: bool,
    pub(super) render_opts: PrintRenderOptions,
    pub(super) sheet_pos: usize,
    pub(super) resolved_pages: Vec<u16>,
    pub(super) range_error: Option<String>,
    pub(super) cached_sheet: Option<CachedPreviewSheet>,
}

impl PrintPreviewState {
    pub(super) fn new(
        printers: Vec<String>,
        selected_printer: usize,
        total_pages: u16,
        current_page: u16,
    ) -> Self {
        let mut st = Self {
            printers,
            selected_printer,
            page_scope: PrintPageScope::All,
            custom_range: String::new(),
            parity: PageParity::All,
            reverse: false,
            copies: 1,
            paper: PrintPaperSize::A4,
            duplex: DuplexMode::Off,
            manual_duplex: false,
            render_opts: PrintRenderOptions::default(),
            sheet_pos: 0,
            resolved_pages: Vec::new(),
            range_error: None,
            cached_sheet: None,
        };
        st.recompute_pages(total_pages, current_page);
        st
    }

    pub(super) fn recompute_pages(&mut self, total_pages: u16, current_page: u16) {
        let range_str = match self.page_scope {
            PrintPageScope::All => "all".to_string(),
            PrintPageScope::Current => format!("{}", current_page + 1),
            PrintPageScope::Custom => self.custom_range.clone(),
        };
        match resolve_print_pages(&range_str, total_pages, self.parity, self.reverse) {
            Ok(pages) => {
                self.resolved_pages = pages;
                self.range_error = None;
                let max_sheets = self.sheet_count();
                if self.sheet_pos >= max_sheets {
                    self.sheet_pos = max_sheets.saturating_sub(1);
                }
            }
            Err(e) => {
                self.resolved_pages.clear();
                self.sheet_pos = 0;
                self.range_error = Some(e.to_string());
            }
        }
    }

    pub(super) fn booklet_sheets(&self) -> Vec<BookletSheet> {
        compute_booklet_sheets(&self.resolved_pages)
    }

    pub(super) fn sheet_count(&self) -> usize {
        if self.render_opts.booklet {
            self.booklet_sheets().len()
        } else {
            self.resolved_pages.len()
        }
    }

    pub(super) fn active_page_index(&self) -> Option<u16> {
        self.resolved_pages.get(self.sheet_pos).copied()
    }
}

/// Waiting modal between Pass 1 (Fronts) and Pass 2 (Backs) during Guided Manual Duplex printing.
pub(super) struct ManualDuplexWaitState {
    pub(super) printer: Option<String>,
    pub(super) title: String,
    pub(super) resolved_pages: Vec<u16>,
    pub(super) copies: u16,
    pub(super) paper: PrintPaperSize,
    pub(super) render_opts: PrintRenderOptions,
    pub(super) reverse_backs: bool,
    pub(super) sheets_per_pass: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ManualDuplexHit {
    ToggleReverseBacks,
    PrintPass2,
    Cancel,
    InsideModal,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PrintPreviewHit {
    PrinterPrev,
    PrinterNext,
    ScopeAll,
    ScopeCurrent,
    ScopeCustom,
    ParityAll,
    ParityOdd,
    ParityEven,
    ToggleReverse,
    CopiesMinus,
    CopiesPlus,
    ToggleCollate,
    PaperA4,
    PaperLetter,
    PaperLegal,
    OrientAuto,
    OrientPortrait,
    OrientLandscape,
    DuplexOff,
    DuplexLong,
    DuplexShort,
    ToggleBooklet,
    ToggleManualDuplex,
    ScaleFit,
    ScaleActual,
    ColorColor,
    ColorGray,
    ToggleAutoTrim,
    ZoomMinus,
    ZoomPlus,
    PrevSheet,
    NextSheet,
    PrintNow,
    #[cfg(target_os = "windows")]
    SystemDialog,
    Cancel,
    InsideModal,
}

#[derive(Clone, Copy)]
struct RectHit {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hit: PrintPreviewHit,
}

impl RectHit {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

struct PreviewLayout {
    mx: i32,
    my: i32,
    mw: u32,
    mh: u32,
    left_w: u32,
    panel_x: i32,
    panel_w: u32,
    hits: Vec<RectHit>,
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn open_print_preview(&mut self) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first to print".to_string());
            self.request_redraw();
            return;
        }
        if self.print_job.is_some() || self.lp_job.is_some() {
            self.status_notice = Some("Already printing — press Esc to cancel".to_string());
            self.request_redraw();
            return;
        }
        self.open_menu = None;
        self.show_customize_toolbar = false;
        self.show_help_overlay = false;
        self.show_about_overlay = false;
        self.combine_modal = None;

        let (printers, selected) = crate::print::discover_system_printers();
        let total = self.total_pages();
        let current = self.current_page_index;
        self.print_preview = Some(PrintPreviewState::new(printers, selected, total, current));
        self.request_redraw();
    }

    pub(super) fn close_print_preview(&mut self) {
        self.print_preview = None;
        self.request_redraw();
    }

    fn compute_print_preview_layout(&self, buf_w: u32, buf_h: u32, sf: f64) -> PreviewLayout {
        let s = sf as f32;
        let mw = ((960.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let mh = ((672.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;

        let panel_w = ((318.0 * s).round() as u32).min(mw.saturating_sub(180));
        let left_w = mw.saturating_sub(panel_w);
        let panel_x = mx + left_w as i32;

        let p = (13.0 * s).round() as i32;
        let ix = panel_x + p;
        let iw = panel_w.saturating_sub((p * 2) as u32);
        let gap = (5.0 * s).round() as i32;
        let row_h = (25.0 * s).round() as u32;
        let lbl_h = (15.0 * s).round() as i32;
        let sec_gap = (7.0 * s).round() as i32;

        let mut hits = Vec::with_capacity(34);

        // Left canvas bottom sheet stepper pill
        let step_w = (240.0 * s).round().min(left_w as f32 - 24.0) as u32;
        let step_h = (28.0 * s).round() as u32;
        let step_x = mx + ((left_w.saturating_sub(step_w)) / 2) as i32;
        let step_y = my + mh as i32 - (44.0 * s).round() as i32;
        let nav_btn = (28.0 * s).round() as u32;
        hits.push(RectHit { x: step_x, y: step_y, w: nav_btn, h: step_h, hit: PrintPreviewHit::PrevSheet });
        hits.push(RectHit { x: step_x + step_w as i32 - nav_btn as i32, y: step_y, w: nav_btn, h: step_h, hit: PrintPreviewHit::NextSheet });

        // Right panel controls (top-down)
        let mut cy = my + (38.0 * s).round() as i32;

        // 1. Printer selector row
        cy += lbl_h;
        let p_arrow = (24.0 * s).round() as u32;
        hits.push(RectHit { x: ix, y: cy, w: p_arrow, h: row_h, hit: PrintPreviewHit::PrinterPrev });
        hits.push(RectHit { x: ix + iw as i32 - p_arrow as i32, y: cy, w: p_arrow, h: row_h, hit: PrintPreviewHit::PrinterNext });
        cy += row_h as i32 + sec_gap;

        // 2. Pages scope (All / Current / Custom)
        cy += lbl_h;
        let w3 = (iw.saturating_sub((gap * 2) as u32)) / 3;
        hits.push(RectHit { x: ix, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::ScopeAll });
        hits.push(RectHit { x: ix + w3 as i32 + gap, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::ScopeCurrent });
        hits.push(RectHit { x: ix + (w3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(w3 * 2 + (gap * 2) as u32), h: row_h, hit: PrintPreviewHit::ScopeCustom });
        cy += row_h as i32 + gap;
        // Custom range input box (click focuses Custom)
        hits.push(RectHit { x: ix, y: cy, w: iw, h: row_h, hit: PrintPreviewHit::ScopeCustom });
        cy += row_h as i32 + gap;
        // Parity (All / Odd / Even) + Reverse toggle
        let par_w = (iw * 60 / 100).saturating_sub((gap * 2) as u32) / 3;
        hits.push(RectHit { x: ix, y: cy, w: par_w, h: row_h, hit: PrintPreviewHit::ParityAll });
        hits.push(RectHit { x: ix + par_w as i32 + gap, y: cy, w: par_w, h: row_h, hit: PrintPreviewHit::ParityOdd });
        hits.push(RectHit { x: ix + (par_w as i32 + gap) * 2, y: cy, w: par_w, h: row_h, hit: PrintPreviewHit::ParityEven });
        let rev_x = ix + (par_w as i32 + gap) * 3 + gap;
        let rev_w = (ix + iw as i32 - rev_x).max(20) as u32;
        hits.push(RectHit { x: rev_x, y: cy, w: rev_w, h: row_h, hit: PrintPreviewHit::ToggleReverse });
        cy += row_h as i32 + sec_gap;

        // 3. Duplex (1-Sided / Long Edge / Short Edge) + Booklet / Manual 2-Sided row
        cy += lbl_h;
        hits.push(RectHit { x: ix, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::DuplexOff });
        hits.push(RectHit { x: ix + w3 as i32 + gap, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::DuplexLong });
        hits.push(RectHit { x: ix + (w3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(w3 * 2 + (gap * 2) as u32), h: row_h, hit: PrintPreviewHit::DuplexShort });
        cy += row_h as i32 + gap;
        let half_row = (iw.saturating_sub(gap as u32)) / 2;
        hits.push(RectHit { x: ix, y: cy, w: half_row, h: row_h, hit: PrintPreviewHit::ToggleBooklet });
        hits.push(RectHit { x: ix + half_row as i32 + gap, y: cy, w: iw.saturating_sub(half_row + gap as u32), h: row_h, hit: PrintPreviewHit::ToggleManualDuplex });
        cy += row_h as i32 + sec_gap;

        // 4. Paper Size (A4 / Letter / Legal)
        cy += lbl_h;
        hits.push(RectHit { x: ix, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::PaperA4 });
        hits.push(RectHit { x: ix + w3 as i32 + gap, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::PaperLetter });
        hits.push(RectHit { x: ix + (w3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(w3 * 2 + (gap * 2) as u32), h: row_h, hit: PrintPreviewHit::PaperLegal });
        cy += row_h as i32 + sec_gap;

        // 5. Orientation (Auto / Portrait / Landscape)
        cy += lbl_h;
        hits.push(RectHit { x: ix, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::OrientAuto });
        hits.push(RectHit { x: ix + w3 as i32 + gap, y: cy, w: w3, h: row_h, hit: PrintPreviewHit::OrientPortrait });
        hits.push(RectHit { x: ix + (w3 as i32 + gap) * 2, y: cy, w: iw.saturating_sub(w3 * 2 + (gap * 2) as u32), h: row_h, hit: PrintPreviewHit::OrientLandscape });
        cy += row_h as i32 + sec_gap;

        // 6. Scale (Fit / Actual) + Color (Color / Gray)
        cy += lbl_h;
        let half_w = (iw.saturating_sub(gap as u32 * 3)) / 2;
        let q_w = (half_w.saturating_sub(gap as u32)) / 2;
        hits.push(RectHit { x: ix, y: cy, w: q_w, h: row_h, hit: PrintPreviewHit::ScaleFit });
        hits.push(RectHit { x: ix + q_w as i32 + gap, y: cy, w: q_w, h: row_h, hit: PrintPreviewHit::ScaleActual });
        let col_x = ix + half_w as i32 + gap * 2;
        hits.push(RectHit { x: col_x, y: cy, w: q_w, h: row_h, hit: PrintPreviewHit::ColorColor });
        hits.push(RectHit { x: col_x + q_w as i32 + gap, y: cy, w: (ix + iw as i32 - (col_x + q_w as i32 + gap)).max(20) as u32, h: row_h, hit: PrintPreviewHit::ColorGray });
        cy += row_h as i32 + sec_gap;

        // 7. Crop White Space: Auto-Trim + Zoom stepper (- 100% +)
        cy += lbl_h;
        let trim_w = iw * 52 / 100;
        hits.push(RectHit { x: ix, y: cy, w: trim_w, h: row_h, hit: PrintPreviewHit::ToggleAutoTrim });
        let z_x = ix + trim_w as i32 + gap * 2;
        let z_btn = (24.0 * s).round() as u32;
        hits.push(RectHit { x: z_x, y: cy, w: z_btn, h: row_h, hit: PrintPreviewHit::ZoomMinus });
        hits.push(RectHit { x: ix + iw as i32 - z_btn as i32, y: cy, w: z_btn, h: row_h, hit: PrintPreviewHit::ZoomPlus });
        cy += row_h as i32 + sec_gap;

        // 8. Copies stepper (- 1 +) + Collate checkbox (checked by default)
        let coll_w = (88.0 * s).round() as u32;
        let coll_x = ix + iw as i32 - coll_w as i32;
        let c_btn = (24.0 * s).round() as u32;
        let cop_step_w = (80.0 * s).round() as i32;
        let cop_x = coll_x - gap * 2 - cop_step_w;
        hits.push(RectHit { x: cop_x, y: cy, w: c_btn, h: row_h, hit: PrintPreviewHit::CopiesMinus });
        hits.push(RectHit { x: cop_x + cop_step_w - c_btn as i32, y: cy, w: c_btn, h: row_h, hit: PrintPreviewHit::CopiesPlus });
        hits.push(RectHit { x: coll_x, y: cy, w: coll_w, h: row_h, hit: PrintPreviewHit::ToggleCollate });

        // Footer buttons
        let foot_h = (46.0 * s).round() as u32;
        let fy = my + mh as i32 - foot_h as i32;
        let btn_h = (30.0 * s).round() as u32;
        let by = fy + ((foot_h.saturating_sub(btn_h)) / 2) as i32;
        let cancel_w = (72.0 * s).round() as u32;
        #[cfg(target_os = "windows")]
        let sys_w = (76.0 * s).round() as u32;
        #[cfg(not(target_os = "windows"))]
        let sys_w = 0u32;
        let extra_gap = if sys_w > 0 { gap as u32 } else { 0 };
        let print_w = iw.saturating_sub(cancel_w + sys_w + gap as u32 + extra_gap);

        hits.push(RectHit { x: ix, y: by, w: print_w, h: btn_h, hit: PrintPreviewHit::PrintNow });
        #[cfg(target_os = "windows")]
        hits.push(RectHit { x: ix + print_w as i32 + gap, y: by, w: sys_w, h: btn_h, hit: PrintPreviewHit::SystemDialog });
        hits.push(RectHit { x: ix + iw as i32 - cancel_w as i32, y: by, w: cancel_w, h: btn_h, hit: PrintPreviewHit::Cancel });

        PreviewLayout { mx, my, mw, mh, left_w, panel_x, panel_w, hits }
    }

    fn find_hit(layout: &PreviewLayout, hit: PrintPreviewHit) -> Option<RectHit> {
        layout.hits.iter().copied().find(|r| r.hit == hit)
    }

    fn render_single_preview_subpage(
        &self,
        doc: &PdfDocument,
        page_idx: u16,
        max_w: u32,
        max_h: u32,
        sheet_scale: f32,
        opts: PrintRenderOptions,
    ) -> Option<(u32, u32, Vec<u32>)> {
        let page = doc.pages().get(page_idx).ok()?;
        let page_w_pts = page.width().value.max(1.0);
        let page_h_pts = page.height().value.max(1.0);

        let scan_pixels = if opts.auto_trim_margins {
            let sw = 160i32;
            let sh = ((160.0 * page_h_pts / page_w_pts).round() as i32).clamp(32, 320);
            self.render_page_into_u32_vec(&page, sw, sh, false, Vec::new())
                .ok()
                .map(|px| (px, sw as u32, sh as u32))
        } else {
            None
        };
        let scan_ref = scan_pixels.as_ref().map(|(v, w, h)| (v.as_slice(), *w, *h));
        let (cx0, cy0, cx1, cy1) =
            compute_effective_crop_box(&page, opts.auto_trim_margins, opts.zoom_percent, scan_ref);
        let eff_w_pts = (cx1 - cx0).max(1.0);
        let eff_h_pts = (cy1 - cy0).max(1.0);
        let dpi = (sheet_scale * 72.0).round().max(72.0) as u32;
        let placement = place_page_on_printable_area(
            eff_w_pts,
            eff_h_pts,
            dpi,
            dpi,
            max_w,
            max_h,
            opts.fit_to_page || opts.booklet || opts.auto_trim_margins || opts.zoom_percent > 100,
        );
        let rw = placement.render_w.max(1);
        let rh = placement.render_h.max(1);
        let pixel_count = (rw as usize) * (rh as usize);
        let mut pixels = vec![0x00FF_FFFFu32; pixel_count];

        let sx = rw as f32 / eff_w_pts;
        let sy = rh as f32 / eff_h_pts;
        let shift_x = -(cx0 * sx);
        let shift_y = -((page_h_pts - cy1).max(0.0) * sy);

        let bindings = self.engine.pdfium.bindings();
        let form_copy = if crate::engine::PdfEngine::page_has_form_widgets(&page) {
            self.engine.flattened_page_copy(doc, page_idx).ok()
        } else {
            None
        };
        let target_page = match &form_copy {
            Some(copy) => copy.pages().get(0).ok()?,
            None => doc.pages().get(page_idx).ok()?,
        };
        let cfg = PdfRenderConfig::new()
            .set_fixed_width(rw as i32)
            .set_fixed_height(rh as i32)
            .render_form_data(false)
            .render_annotations(true)
            .use_print_quality(true)
            .limit_render_image_cache_size(true)
            .set_reverse_byte_order(false)
            .reset_matrix(PdfMatrix::new(sx, 0.0, 0.0, sy, shift_x, shift_y))
            .ok()?;
        let byte_slice: &mut [u8] = unsafe {
            std::slice::from_raw_parts_mut(pixels.as_mut_ptr() as *mut u8, pixel_count * 4)
        };
        let mut bitmap = unsafe {
            PdfBitmap::from_bytes(rw as i32, rh as i32, PdfBitmapFormat::BGRx, byte_slice, bindings)
        }
        .ok()?;
        target_page.render_into_bitmap_with_config(&mut bitmap, &cfg).ok()?;

        if opts.grayscale {
            for px in &mut pixels {
                let c = *px & 0x00FF_FFFF;
                let r = (c >> 16) & 0xFF;
                let g = (c >> 8) & 0xFF;
                let b = c & 0xFF;
                let y = (77 * r + 150 * g + 29 * b) >> 8;
                *px = (y << 16) | (y << 8) | y;
            }
        } else {
            for px in &mut pixels {
                *px &= 0x00FF_FFFF;
            }
        }
        Some((rw, rh, pixels))
    }

    /// Ensures the single preview sheet bitmap for the active sheet (normal or 2-up Booklet spread)
    /// is rendered into `self.print_preview.cached_sheet`.
    pub(super) fn ensure_print_preview_sheet_rendered(&mut self, buf_w: u32, buf_h: u32, sf: f64) {
        let Some(pp) = self.print_preview.as_ref() else { return };
        let opts = pp.render_opts;
        let paper = pp.paper;

        let (key_left, key_right) = if opts.booklet {
            let sheets = pp.booklet_sheets();
            let Some(sh) = sheets.get(pp.sheet_pos).copied() else { return };
            (sh.left, sh.right)
        } else {
            let Some(p) = pp.active_page_index() else { return };
            (Some(p), None)
        };

        let layout = self.compute_print_preview_layout(buf_w, buf_h, sf);
        let s = sf as f32;
        let avail_w = layout.left_w.saturating_sub((56.0 * s).round() as u32).max(80);
        let avail_h = layout.mh.saturating_sub((110.0 * s).round() as u32).max(80);

        let Some(doc) = self.doc.as_ref() else { return };
        let (p_w, p_h) = paper.portrait_pts();
        let landscape = if opts.booklet {
            true
        } else if let Some(pidx) = key_left {
            doc.pages()
                .get(pidx)
                .map(|p| opts.orientation.is_landscape(p.width().value, p.height().value))
                .unwrap_or(opts.orientation == PrintOrientation::Landscape)
        } else {
            opts.orientation == PrintOrientation::Landscape
        };
        let (sheet_w_pts, sheet_h_pts) = if landscape { (p_h, p_w) } else { (p_w, p_h) };
        let sheet_scale = (avail_w as f32 / sheet_w_pts).min(avail_h as f32 / sheet_h_pts);
        let sheet_px_w = (sheet_w_pts * sheet_scale).round().max(40.0) as u32;
        let sheet_px_h = (sheet_h_pts * sheet_scale).round().max(40.0) as u32;
        let margin_x = ((sheet_px_w as f32) * 0.05).round() as u32;
        let margin_y = ((sheet_px_h as f32) * 0.05).round() as u32;
        let print_w = sheet_px_w.saturating_sub(margin_x * 2).max(16);
        let print_h = sheet_px_h.saturating_sub(margin_y * 2).max(16);

        let already_valid = self
            .print_preview
            .as_ref()
            .and_then(|p| p.cached_sheet.as_ref())
            .is_some_and(|c| {
                c.key_left == key_left
                    && c.key_right == key_right
                    && c.booklet == opts.booklet
                    && c.auto_trim == opts.auto_trim_margins
                    && c.zoom_percent == opts.zoom_percent
                    && c.grayscale == opts.grayscale
                    && ((opts.booklet && c.render_w == print_w && c.render_h == print_h) || !opts.booklet)
            });
        if already_valid {
            return;
        }

        if opts.booklet {
            let half_w = (print_w / 2).max(8);
            let mut canvas = vec![0x00FF_FFFFu32; (print_w as usize) * (print_h as usize)];
            let blit_half = |dst: &mut [u32], half_x0: u32, half_box_w: u32, src: &(u32, u32, Vec<u32>)| {
                let (sw, sh, ref spx) = *src;
                let ox = half_x0 + half_box_w.saturating_sub(sw) / 2;
                let oy = print_h.saturating_sub(sh) / 2;
                for y in 0..sh.min(print_h) {
                    let dy = oy + y;
                    if dy >= print_h {
                        break;
                    }
                    let d_row = (dy * print_w) as usize;
                    let s_row = (y * sw) as usize;
                    for x in 0..sw.min(half_box_w) {
                        let dx = ox + x;
                        if dx < print_w {
                            dst[d_row + dx as usize] = spx[s_row + x as usize];
                        }
                    }
                }
            };
            if let Some(lp) = key_left {
                if let Some(sub) = self.render_single_preview_subpage(doc, lp, half_w.saturating_sub(4), print_h, sheet_scale, opts) {
                    blit_half(&mut canvas, 0, half_w, &sub);
                }
            }
            if let Some(rp) = key_right {
                if let Some(sub) = self.render_single_preview_subpage(doc, rp, half_w.saturating_sub(4), print_h, sheet_scale, opts) {
                    blit_half(&mut canvas, half_w, print_w.saturating_sub(half_w), &sub);
                }
            }
            if let Some(pp) = self.print_preview.as_mut() {
                pp.cached_sheet = Some(CachedPreviewSheet {
                    key_left,
                    key_right,
                    booklet: true,
                    render_w: print_w,
                    render_h: print_h,
                    auto_trim: opts.auto_trim_margins,
                    zoom_percent: opts.zoom_percent,
                    grayscale: opts.grayscale,
                    pixels: canvas,
                });
            }
        } else if let Some(lp) = key_left {
            if let Some((rw, rh, pixels)) = self.render_single_preview_subpage(doc, lp, print_w, print_h, sheet_scale, opts) {
                if let Some(pp) = self.print_preview.as_mut() {
                    pp.cached_sheet = Some(CachedPreviewSheet {
                        key_left,
                        key_right: None,
                        booklet: false,
                        render_w: rw,
                        render_h: rh,
                        auto_trim: opts.auto_trim_margins,
                        zoom_percent: opts.zoom_percent,
                        grayscale: opts.grayscale,
                        pixels,
                    });
                }
            }
        }
    }

    pub(super) fn draw_print_preview_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(pp) = self.print_preview.as_ref() else { return };
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let layout = self.compute_print_preview_layout(buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        draw_soft_shadow(
            buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad,
            (12.0 * s).round() as i32, (5.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.canvas_bg);
        // Right panel surface (preserve modal's rounded right corners)
        let r_seam = (rad.ceil() as u32).min(layout.panel_w);
        fill_rounded_rect(
            buffer, buf_w, buf_h,
            layout.panel_x, layout.my, layout.panel_w, layout.mh,
            rad, theme.surface_bg,
        );
        fill_rect_clipped(
            buffer, buf_w, buf_h,
            layout.panel_x, layout.my + 1, r_seam, layout.mh.saturating_sub(2),
            theme.surface_bg,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x, layout.my + 1, 1, layout.mh.saturating_sub(2), theme.divider);
        stroke_rounded_rect(buffer, buf_w, buf_h, layout.mx, layout.my, layout.mw, layout.mh, rad, theme.border_subtle);

        let title_sz = (14.5 * s).clamp(12.0, 32.0);
        let lbl_sz = (10.5 * s).clamp(9.0, 22.0);
        let body_sz = (11.0 * s).clamp(9.5, 24.0);
        let p = (13.0 * s).round() as i32;

        // Left canvas: Virtual paper sheet
        let avail_w = layout.left_w.saturating_sub((56.0 * s).round() as u32).max(80);
        let avail_h = layout.mh.saturating_sub((110.0 * s).round() as u32).max(80);
        let (page_w_pts, page_h_pts) = pp
            .active_page_index()
            .and_then(|idx| self.doc.as_ref()?.pages().page_size(idx).ok())
            .map(|r| (r.width().value.max(1.0), r.height().value.max(1.0)))
            .unwrap_or((595.28, 841.89));
        let landscape = pp.render_opts.booklet || pp.render_opts.orientation.is_landscape(page_w_pts, page_h_pts);
        let (pw, ph) = pp.paper.portrait_pts();
        let (sheet_w_pts, sheet_h_pts) = if landscape { (ph, pw) } else { (pw, ph) };
        let sheet_scale = (avail_w as f32 / sheet_w_pts).min(avail_h as f32 / sheet_h_pts);
        let sheet_w = (sheet_w_pts * sheet_scale).round().max(40.0) as u32;
        let sheet_h = (sheet_h_pts * sheet_scale).round().max(40.0) as u32;
        let sheet_x = layout.mx + ((layout.left_w.saturating_sub(sheet_w)) / 2) as i32;
        let sheet_y = layout.my + (22.0 * s).round() as i32 + ((avail_h.saturating_sub(sheet_h)) / 2) as i32;

        draw_soft_shadow(
            buffer, buf_w, buf_h, sheet_x, sheet_y, sheet_w, sheet_h, 2.0,
            (8.0 * s).round() as i32, (3.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rect_clipped(buffer, buf_w, buf_h, sheet_x, sheet_y, sheet_w, sheet_h, 0x00FF_FFFF);

        let mx_in = ((sheet_w as f32) * 0.05).round() as u32;
        let my_in = ((sheet_h as f32) * 0.05).round() as u32;
        let print_w = sheet_w.saturating_sub(mx_in * 2).max(16);
        let print_h = sheet_h.saturating_sub(my_in * 2).max(16);

        if let Some(ref sheet) = pp.cached_sheet {
            let draw_x = sheet_x + mx_in as i32 + ((print_w as i32 - sheet.render_w as i32) / 2);
            let draw_y = sheet_y + my_in as i32 + ((print_h as i32 - sheet.render_h as i32) / 2);
            let clip_x0 = (sheet_x + mx_in as i32).max(0);
            let clip_y0 = (sheet_y + my_in as i32).max(0);
            let clip_x1 = (sheet_x + (sheet_w - mx_in) as i32).min(buf_w as i32);
            let clip_y1 = (sheet_y + (sheet_h - my_in) as i32).min(buf_h as i32);
            let y0 = draw_y.max(clip_y0);
            let y1 = (draw_y + sheet.render_h as i32).min(clip_y1);
            let x0 = draw_x.max(clip_x0);
            let x1 = (draw_x + sheet.render_w as i32).min(clip_x1);
            for y in y0..y1 {
                let sy = (y - draw_y) as u32;
                let dst_row = (y as u32 * buf_w) as usize;
                let src_row = (sy * sheet.render_w) as usize;
                for x in x0..x1 {
                    let sx = (x - draw_x) as u32;
                    buffer[dst_row + x as usize] = sheet.pixels[src_row + sx as usize];
                }
            }
        } else if let Some(ref err) = pp.range_error {
            let ew = measure_text(err, body_sz, false);
            draw_text(buffer, buf_w, buf_h, sheet_x + ((sheet_w as f32 - ew) * 0.5).round() as i32, sheet_y + (sheet_h / 2) as i32, err, body_sz, theme.danger, false);
        }

        // Dashed printable margin guide (+ center fold guide in Booklet mode!)
        let dash_px = (4.0 * s).round().max(3.0) as i32;
        draw_dashed_rect(
            buffer, buf_w, buf_h,
            sheet_x + mx_in as i32, sheet_y + my_in as i32, print_w, print_h,
            dash_px, dash_px, theme.accent,
        );
        if pp.render_opts.booklet {
            let fold_x = sheet_x + (sheet_w / 2) as i32;
            let dash = (5.0 * s).round().max(3.0) as i32;
            let mut fy = sheet_y + 4;
            while fy < sheet_y + sheet_h as i32 - 4 {
                fill_rect_clipped(buffer, buf_w, buf_h, fold_x, fy, 1, (dash as u32).min((sheet_y + sheet_h as i32 - fy) as u32), theme.accent);
                fy += dash * 2;
            }
        }
        stroke_rounded_rect(buffer, buf_w, buf_h, sheet_x, sheet_y, sheet_w, sheet_h, 1.0, theme.page_border);

        // Bottom sheet navigation pill
        if let (Some(prev_r), Some(next_r)) = (
            Self::find_hit(&layout, PrintPreviewHit::PrevSheet),
            Self::find_hit(&layout, PrintPreviewHit::NextSheet),
        ) {
            let pill_w = (next_r.x + next_r.w as i32 - prev_r.x) as u32;
            let pill_rad = (6.0 * s).clamp(4.0, 10.0);
            fill_rounded_rect(buffer, buf_w, buf_h, prev_r.x, prev_r.y, pill_w, prev_r.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, prev_r.x, prev_r.y, pill_w, prev_r.h, pill_rad, theme.border_subtle);

            let ic = (12.0 * s).round() as u32;
            let prev_col = if pp.sheet_pos > 0 { theme.text_primary } else { theme.text_muted };
            let total_sheets = pp.sheet_count();
            let next_col = if pp.sheet_pos + 1 < total_sheets { theme.text_primary } else { theme.text_muted };
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronLeft, prev_r.x + ((prev_r.w.saturating_sub(ic)) / 2) as i32, prev_r.y + ((prev_r.h.saturating_sub(ic)) / 2) as i32, ic, prev_col);
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronRight, next_r.x + ((next_r.w.saturating_sub(ic)) / 2) as i32, next_r.y + ((next_r.h.saturating_sub(ic)) / 2) as i32, ic, next_col);

            let sheet_lbl = if total_sheets == 0 {
                "0 sheets".to_string()
            } else if pp.render_opts.booklet {
                let sheets = pp.booklet_sheets();
                if let Some(sh) = sheets.get(pp.sheet_pos) {
                    let side = if sh.is_front { "Front" } else { "Back" };
                    let l_s = sh.left.map(|p| format!("p.{}", p + 1)).unwrap_or_else(|| "Blank".to_string());
                    let r_s = sh.right.map(|p| format!("p.{}", p + 1)).unwrap_or_else(|| "Blank".to_string());
                    format!("Sheet {} {} [{} | {}]", pp.sheet_pos / 2 + 1, side, l_s, r_s)
                } else {
                    format!("Booklet {}/{}", pp.sheet_pos + 1, total_sheets)
                }
            } else {
                let pnum = pp.active_page_index().map(|x| x + 1).unwrap_or(1);
                format!("Sheet {} / {} (p.{})", pp.sheet_pos + 1, total_sheets, pnum)
            };
            let lw = measure_text(&sheet_lbl, body_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, prev_r.x + ((pill_w as f32 - lw) * 0.5).round() as i32, prev_r.y, prev_r.h, next_r.x, &sheet_lbl, body_sz, theme.text_primary, false);
        }

        // Right panel header
        let ix = layout.panel_x + p;
        draw_text(buffer, buf_w, buf_h, ix, layout.my + (11.0 * s).round() as i32, "Print Preview", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x + 1, layout.my + (35.0 * s).round() as i32, layout.panel_w.saturating_sub(2), 1, theme.divider);

        let lbl_h = (15.0 * s).round() as i32;
        let pill_rad = (5.0 * s).clamp(3.0, 8.0);
        let ic_sz = (11.0 * s).round() as u32;

        let draw_pill = |buffer: &mut [u32], r: RectHit, text: &str, active: bool| {
            let hov = self.hovered_widget == Some(UiHit::PrintPreview(r.hit));
            let bg = if active {
                theme.accent_soft_bg
            } else if hov {
                theme.hover_bg
            } else {
                theme.surface_elevated
            };
            let bd = if active { theme.accent } else { theme.border_subtle };
            let fg = if active { theme.accent_soft_fg } else { theme.text_primary };
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, pill_rad, bd);
            let clipped = truncate_ellipsis(text, (r.w as f32 - 8.0).max(12.0), body_sz, active);
            let tw = measure_text(&clipped, body_sz, active);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + ((r.w as f32 - tw) * 0.5).round() as i32, r.y, r.h, r.x + r.w as i32 - 2, &clipped, body_sz, fg, active);
        };

        // 1. Printer
        if let (Some(lp), Some(rp)) = (
            Self::find_hit(&layout, PrintPreviewHit::PrinterPrev),
            Self::find_hit(&layout, PrintPreviewHit::PrinterNext),
        ) {
            draw_text(buffer, buf_w, buf_h, ix, lp.y - lbl_h, "Printer", lbl_sz, theme.text_muted, true);
            let full_w = (rp.x + rp.w as i32 - lp.x) as u32;
            fill_rounded_rect(buffer, buf_w, buf_h, lp.x, lp.y, full_w, lp.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, lp.x, lp.y, full_w, lp.h, pill_rad, theme.input_border);
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronLeft, lp.x + ((lp.w.saturating_sub(ic_sz)) / 2) as i32, lp.y + ((lp.h.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
            draw_icon(buffer, buf_w, buf_h, UiIcon::ChevronRight, rp.x + ((rp.w.saturating_sub(ic_sz)) / 2) as i32, rp.y + ((rp.h.saturating_sub(ic_sz)) / 2) as i32, ic_sz, theme.text_secondary);
            let pname = pp.printers.get(pp.selected_printer).map(String::as_str).unwrap_or("No printer detected");
            let max_pw = (rp.x - (lp.x + lp.w as i32) - 8).max(20) as f32;
            let p_trunc = truncate_ellipsis(pname, max_pw, body_sz, false);
            draw_text_vcenter(buffer, buf_w, buf_h, lp.x + lp.w as i32 + 4, lp.y, lp.h, rp.x - 4, &p_trunc, body_sz, theme.text_primary, false);
        }

        // 2. Pages
        if let Some(r_all) = Self::find_hit(&layout, PrintPreviewHit::ScopeAll) {
            let total_sh = pp.sheet_count() * pp.copies as usize;
            let page_hdr = if pp.render_opts.booklet {
                format!("Pages ({} booklet sides × {})", pp.sheet_count(), pp.copies)
            } else {
                format!("Pages ({} × {} = {} sheets)", pp.resolved_pages.len(), pp.copies, total_sh)
            };
            draw_text(buffer, buf_w, buf_h, ix, r_all.y - lbl_h, &page_hdr, lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r_all, "All", pp.page_scope == PrintPageScope::All);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ScopeCurrent) {
            draw_pill(buffer, r, "Current", pp.page_scope == PrintPageScope::Current);
        }
        let custom_rects: Vec<RectHit> = layout.hits.iter().copied().filter(|r| r.hit == PrintPreviewHit::ScopeCustom).collect();
        if let Some(&tab_r) = custom_rects.first() {
            draw_pill(buffer, tab_r, "Custom", pp.page_scope == PrintPageScope::Custom);
        }
        if let Some(&box_r) = custom_rects.get(1) {
            let active = pp.page_scope == PrintPageScope::Custom;
            let bd = if pp.range_error.is_some() {
                theme.danger
            } else if active {
                theme.accent
            } else {
                theme.border_subtle
            };
            fill_rounded_rect(buffer, buf_w, buf_h, box_r.x, box_r.y, box_r.w, box_r.h, pill_rad, theme.surface_bg);
            stroke_rounded_rect(buffer, buf_w, buf_h, box_r.x, box_r.y, box_r.w, box_r.h, pill_rad, bd);
            let txt = if pp.custom_range.is_empty() {
                if active { "e.g. 1-3, 5, 8-|".to_string() } else { "e.g. 1-3, 5, 8-".to_string() }
            } else if active {
                format!("{}|", pp.custom_range)
            } else {
                pp.custom_range.clone()
            };
            let fg = if pp.custom_range.is_empty() { theme.text_muted } else { theme.text_primary };
            draw_text_vcenter(buffer, buf_w, buf_h, box_r.x + (8.0 * s).round() as i32, box_r.y, box_r.h, box_r.x + box_r.w as i32 - 6, &txt, body_sz, fg, false);
        }

        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ParityAll) {
            draw_pill(buffer, r, "All", pp.parity == PageParity::All);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ParityOdd) {
            draw_pill(buffer, r, "Odd", pp.parity == PageParity::Odd);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ParityEven) {
            draw_pill(buffer, r, "Even", pp.parity == PageParity::Even);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ToggleReverse) {
            draw_pill(buffer, r, if pp.reverse { "✓ Reverse" } else { "Reverse" }, pp.reverse);
        }

        // 3. Duplex + Booklet & Guided Manual Duplex
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::DuplexOff) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Both Sides, Booklet & Simplex Wizard", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, DuplexMode::Off.label(), !pp.manual_duplex && pp.duplex == DuplexMode::Off);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::DuplexLong) {
            draw_pill(buffer, r, DuplexMode::LongEdge.label(), !pp.manual_duplex && pp.duplex == DuplexMode::LongEdge);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::DuplexShort) {
            draw_pill(buffer, r, DuplexMode::ShortEdge.label(), !pp.manual_duplex && pp.duplex == DuplexMode::ShortEdge);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ToggleBooklet) {
            draw_pill(buffer, r, if pp.render_opts.booklet { "✓ Booklet (2-Up)" } else { "Booklet (2-Up)" }, pp.render_opts.booklet);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ToggleManualDuplex) {
            draw_pill(buffer, r, if pp.manual_duplex { "✓ Simplex 2-Sided" } else { "Simplex 2-Sided" }, pp.manual_duplex);
        }

        // 4. Paper Size
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::PaperA4) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Paper Size", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, PrintPaperSize::A4.label(), pp.paper == PrintPaperSize::A4);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::PaperLetter) {
            draw_pill(buffer, r, PrintPaperSize::Letter.label(), pp.paper == PrintPaperSize::Letter);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::PaperLegal) {
            draw_pill(buffer, r, PrintPaperSize::Legal.label(), pp.paper == PrintPaperSize::Legal);
        }

        // 5. Orientation
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::OrientAuto) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Orientation", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, PrintOrientation::Auto.label(), !pp.render_opts.booklet && pp.render_opts.orientation == PrintOrientation::Auto);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::OrientPortrait) {
            draw_pill(buffer, r, PrintOrientation::Portrait.label(), !pp.render_opts.booklet && pp.render_opts.orientation == PrintOrientation::Portrait);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::OrientLandscape) {
            draw_pill(buffer, r, PrintOrientation::Landscape.label(), pp.render_opts.booklet || pp.render_opts.orientation == PrintOrientation::Landscape);
        }

        // 6. Scale & Color
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ScaleFit) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Scale & Color", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, "Fit", pp.render_opts.fit_to_page);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ScaleActual) {
            draw_pill(buffer, r, "100%", !pp.render_opts.fit_to_page);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ColorColor) {
            draw_pill(buffer, r, "Color", !pp.render_opts.grayscale);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ColorGray) {
            draw_pill(buffer, r, "Gray", pp.render_opts.grayscale);
        }

        // 7. Crop White Space (Auto-Trim + Zoom stepper)
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ToggleAutoTrim) {
            draw_text(buffer, buf_w, buf_h, ix, r.y - lbl_h, "Crop White Space & Zoom", lbl_sz, theme.text_muted, true);
            draw_pill(buffer, r, if pp.render_opts.auto_trim_margins { "✓ Auto-Trim" } else { "Auto-Trim" }, pp.render_opts.auto_trim_margins);
        }
        if let (Some(zm), Some(zp)) = (
            Self::find_hit(&layout, PrintPreviewHit::ZoomMinus),
            Self::find_hit(&layout, PrintPreviewHit::ZoomPlus),
        ) {
            draw_pill(buffer, zm, "−", false);
            draw_pill(buffer, zp, "+", false);
            let zlbl = format!("{}%", pp.render_opts.zoom_percent);
            let zw = measure_text(&zlbl, body_sz, false);
            let mid_x = zm.x + zm.w as i32;
            let mid_w = (zp.x - mid_x).max(1) as f32;
            draw_text_vcenter(buffer, buf_w, buf_h, mid_x + ((mid_w - zw) * 0.5).round() as i32, zm.y, zm.h, zp.x, &zlbl, body_sz, theme.text_primary, false);
        }

        // 8. Copies + Collate (checked by default)
        if let (Some(cm), Some(cp)) = (
            Self::find_hit(&layout, PrintPreviewHit::CopiesMinus),
            Self::find_hit(&layout, PrintPreviewHit::CopiesPlus),
        ) {
            draw_text_vcenter(buffer, buf_w, buf_h, ix, cm.y, cm.h, cm.x - 4, "Copies", body_sz, theme.text_secondary, true);
            draw_pill(buffer, cm, "−", false);
            draw_pill(buffer, cp, "+", false);
            let clbl = format!("{}", pp.copies);
            let cw = measure_text(&clbl, body_sz, true);
            let mid_x = cm.x + cm.w as i32;
            let mid_w = (cp.x - mid_x).max(1) as f32;
            draw_text_vcenter(buffer, buf_w, buf_h, mid_x + ((mid_w - cw) * 0.5).round() as i32, cm.y, cm.h, cp.x, &clbl, body_sz, theme.text_primary, true);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::ToggleCollate) {
            draw_pill(buffer, r, if pp.render_opts.collate { "✓ Collate" } else { "Collate" }, pp.render_opts.collate);
        }

        // Footer divider & buttons
        let foot_h = (46.0 * s).round() as u32;
        let fy = layout.my + layout.mh as i32 - foot_h as i32;
        fill_rect_clipped(buffer, buf_w, buf_h, layout.panel_x + 1, fy, layout.panel_w.saturating_sub(2), 1, theme.divider);

        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::PrintNow) {
            let can_print = !pp.resolved_pages.is_empty();
            let hov = self.hovered_widget == Some(UiHit::PrintPreview(PrintPreviewHit::PrintNow));
            let bg = if !can_print {
                theme.surface_elevated
            } else if hov {
                theme.accent_hover
            } else {
                theme.accent
            };
            let fg = if can_print { theme.text_on_accent } else { theme.text_muted };
            fill_rounded_rect(buffer, buf_w, buf_h, r.x, r.y, r.w, r.h, (6.0 * s).max(4.0), bg);
            let btn_lbl = if pp.manual_duplex { "Print Pass 1" } else { "Print" };
            let tw = measure_text(btn_lbl, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, r.x + ((r.w as f32 - tw) * 0.5).round() as i32, r.y, r.h, r.x + r.w as i32, btn_lbl, body_sz, fg, true);
        }
        #[cfg(target_os = "windows")]
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::SystemDialog) {
            draw_pill(buffer, r, "System...", false);
        }
        if let Some(r) = Self::find_hit(&layout, PrintPreviewHit::Cancel) {
            draw_pill(buffer, r, "Cancel", false);
        }
    }

    pub(super) fn hit_test_print_preview(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<PrintPreviewHit> {
        self.print_preview.as_ref()?;
        let layout = self.compute_print_preview_layout(buf_w, buf_h, sf);
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < layout.mx || ix >= layout.mx + layout.mw as i32 || iy < layout.my || iy >= layout.my + layout.mh as i32 {
            return None;
        }
        for r in &layout.hits {
            if r.contains(ix, iy) {
                return Some(r.hit);
            }
        }
        Some(PrintPreviewHit::InsideModal)
    }

    pub(super) fn handle_print_preview_click(&mut self, hit: PrintPreviewHit) {
        let total = self.total_pages();
        let cur = self.current_page_index;
        match hit {
            PrintPreviewHit::Cancel => {
                self.close_print_preview();
                return;
            }
            PrintPreviewHit::PrintNow => {
                self.execute_print_from_preview();
                return;
            }
            #[cfg(target_os = "windows")]
            PrintPreviewHit::SystemDialog => {
                self.execute_windows_system_print_dialog();
                return;
            }
            PrintPreviewHit::InsideModal => return,
            _ => {}
        }

        if let Some(pp) = self.print_preview.as_mut() {
            match hit {
                PrintPreviewHit::PrinterPrev => {
                    if !pp.printers.is_empty() {
                        pp.selected_printer = if pp.selected_printer == 0 {
                            pp.printers.len() - 1
                        } else {
                            pp.selected_printer - 1
                        };
                    }
                }
                PrintPreviewHit::PrinterNext => {
                    if !pp.printers.is_empty() {
                        pp.selected_printer = (pp.selected_printer + 1) % pp.printers.len();
                    }
                }
                PrintPreviewHit::ScopeAll => {
                    pp.page_scope = PrintPageScope::All;
                    pp.recompute_pages(total, cur);
                }
                PrintPreviewHit::ScopeCurrent => {
                    pp.page_scope = PrintPageScope::Current;
                    pp.recompute_pages(total, cur);
                }
                PrintPreviewHit::ScopeCustom => {
                    pp.page_scope = PrintPageScope::Custom;
                    pp.recompute_pages(total, cur);
                }
                PrintPreviewHit::ParityAll => {
                    pp.parity = PageParity::All;
                    pp.recompute_pages(total, cur);
                }
                PrintPreviewHit::ParityOdd => {
                    pp.parity = PageParity::Odd;
                    pp.recompute_pages(total, cur);
                }
                PrintPreviewHit::ParityEven => {
                    pp.parity = PageParity::Even;
                    pp.recompute_pages(total, cur);
                }
                PrintPreviewHit::ToggleReverse => {
                    pp.reverse = !pp.reverse;
                    pp.recompute_pages(total, cur);
                }
                PrintPreviewHit::CopiesMinus => {
                    pp.copies = pp.copies.saturating_sub(1).max(1);
                }
                PrintPreviewHit::CopiesPlus => {
                    pp.copies = (pp.copies + 1).min(99);
                }
                PrintPreviewHit::ToggleCollate => {
                    pp.render_opts.collate = !pp.render_opts.collate;
                }
                PrintPreviewHit::PaperA4 => pp.paper = PrintPaperSize::A4,
                PrintPreviewHit::PaperLetter => pp.paper = PrintPaperSize::Letter,
                PrintPreviewHit::PaperLegal => pp.paper = PrintPaperSize::Legal,
                PrintPreviewHit::OrientAuto => pp.render_opts.orientation = PrintOrientation::Auto,
                PrintPreviewHit::OrientPortrait => pp.render_opts.orientation = PrintOrientation::Portrait,
                PrintPreviewHit::OrientLandscape => pp.render_opts.orientation = PrintOrientation::Landscape,
                PrintPreviewHit::DuplexOff => {
                    pp.manual_duplex = false;
                    pp.duplex = DuplexMode::Off;
                }
                PrintPreviewHit::DuplexLong => {
                    pp.manual_duplex = false;
                    pp.duplex = DuplexMode::LongEdge;
                }
                PrintPreviewHit::DuplexShort => {
                    pp.manual_duplex = false;
                    pp.duplex = DuplexMode::ShortEdge;
                }
                PrintPreviewHit::ToggleBooklet => {
                    pp.render_opts.booklet = !pp.render_opts.booklet;
                    if pp.render_opts.booklet && !pp.manual_duplex && pp.duplex == DuplexMode::Off {
                        pp.duplex = DuplexMode::ShortEdge;
                    }
                    pp.sheet_pos = 0;
                    pp.cached_sheet = None;
                }
                PrintPreviewHit::ToggleManualDuplex => {
                    pp.manual_duplex = !pp.manual_duplex;
                    if pp.manual_duplex {
                        pp.duplex = DuplexMode::Off;
                    }
                }
                PrintPreviewHit::ScaleFit => pp.render_opts.fit_to_page = true,
                PrintPreviewHit::ScaleActual => pp.render_opts.fit_to_page = false,
                PrintPreviewHit::ColorColor => pp.render_opts.grayscale = false,
                PrintPreviewHit::ColorGray => pp.render_opts.grayscale = true,
                PrintPreviewHit::ToggleAutoTrim => {
                    pp.render_opts.auto_trim_margins = !pp.render_opts.auto_trim_margins;
                }
                PrintPreviewHit::ZoomMinus => {
                    pp.render_opts.zoom_percent = pp.render_opts.zoom_percent.saturating_sub(5).max(100);
                }
                PrintPreviewHit::ZoomPlus => {
                    pp.render_opts.zoom_percent = (pp.render_opts.zoom_percent + 5).min(200);
                }
                PrintPreviewHit::PrevSheet => {
                    if pp.sheet_pos > 0 {
                        pp.sheet_pos -= 1;
                    }
                }
                PrintPreviewHit::NextSheet => {
                    if pp.sheet_pos + 1 < pp.sheet_count() {
                        pp.sheet_pos += 1;
                    }
                }
                _ => {}
            }
        }
        self.request_redraw();
    }

    pub(super) fn handle_print_preview_key(&mut self, logical_key: winit::keyboard::Key, text: Option<&str>) {
        use winit::keyboard::{Key, NamedKey};
        let total = self.total_pages();
        let cur = self.current_page_index;
        match logical_key {
            Key::Named(NamedKey::Escape) => {
                self.close_print_preview();
            }
            Key::Named(NamedKey::Enter) => {
                self.execute_print_from_preview();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if let Some(pp) = self.print_preview.as_mut() {
                    if pp.sheet_pos > 0 {
                        pp.sheet_pos -= 1;
                    }
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some(pp) = self.print_preview.as_mut() {
                    if pp.sheet_pos + 1 < pp.sheet_count() {
                        pp.sheet_pos += 1;
                    }
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(pp) = self.print_preview.as_mut() {
                    pp.page_scope = PrintPageScope::Custom;
                    pp.custom_range.pop();
                    pp.recompute_pages(total, cur);
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Tab) => {
                if let Some(pp) = self.print_preview.as_mut() {
                    if !pp.printers.is_empty() {
                        pp.selected_printer = (pp.selected_printer + 1) % pp.printers.len();
                    }
                }
                self.request_redraw();
            }
            _ => {
                let s_in = text.or_else(|| match &logical_key {
                    Key::Character(c) => Some(c.as_str()),
                    _ => None,
                });
                if let Some(typed) = s_in {
                    if let Some(pp) = self.print_preview.as_mut() {
                        let mut changed = false;
                        for c in typed.chars() {
                            if c.is_ascii_digit() || c == '-' || c == ',' || c == ' ' {
                                pp.page_scope = PrintPageScope::Custom;
                                pp.custom_range.push(c);
                                changed = true;
                            }
                        }
                        if changed {
                            pp.recompute_pages(total, cur);
                            self.request_redraw();
                        }
                    }
                }
            }
        }
    }

    fn dispatch_print_slots(
        &mut self,
        printer: Option<String>,
        title: String,
        slots: Vec<Option<u16>>,
        copies: u16,
        paper: PrintPaperSize,
        duplex: DuplexMode,
        render_opts: PrintRenderOptions,
    ) -> bool {
        #[cfg(target_os = "windows")]
        {
            if self.doc.is_none() {
                return false;
            }
            let plain_indices: Vec<u16> = slots.iter().flatten().copied().collect();
            let setup_res = if let Some(ref pname) = printer {
                crate::print::open_windows_printer_direct(pname, &title, plain_indices, copies, duplex, paper)
                    .map(Some)
            } else {
                let owner_hwnd = self.window.as_ref().and_then(|w| {
                    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    match w.window_handle().ok()?.as_raw() {
                        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                        _ => None,
                    }
                });
                crate::print::show_windows_print_dialog(&title, self.total_pages(), owner_hwnd)
            };
            match setup_res {
                Ok(None) => {
                    self.status_notice = Some("Print cancelled".to_string());
                    false
                }
                Err(e) => {
                    self.status_notice = Some(format!("Print error: {e:#}"));
                    false
                }
                Ok(Some(setup)) => {
                    let job_res = self.doc.as_ref().map(|doc| {
                        crate::print::PagedPrintJob::new_with_optional_pages(
                            self.engine,
                            doc,
                            &slots,
                            setup.loop_copies,
                            setup.reported_copies,
                            self.redacted,
                            render_opts,
                            &std::env::temp_dir(),
                            Box::new(setup.sink),
                        )
                    });
                    if let Some(res) = job_res {
                        match res {
                            Ok(job) => {
                                self.start_print_job(job, title);
                                true
                            }
                            Err(e) => {
                                self.status_notice = Some(format!("Print error: {e:#}"));
                                false
                            }
                        }
                    } else {
                        false
                    }
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let Some(printer_name) = printer else {
                self.status_notice = Some("Print failed: no printers found via 'lpstat -p -d'".to_string());
                return false;
            };
            let Some(doc) = self.doc.as_ref() else { return false };
            let req = crate::print::CupsPrintExtendedRequest {
                printer: &printer_name,
                copies,
                optional_pages: Some(&slots),
                page_indices: &[],
                title: &title,
                paper,
                duplex,
                render_opts,
            };
            match crate::print::start_cups_print_extended(
                self.engine,
                doc,
                &req,
                self.redacted,
                &self.lp_program,
                &std::env::temp_dir(),
            ) {
                Ok(job) => {
                    self.status_notice = Some(format!("Sending to printer '{printer_name}'..."));
                    self.lp_job = Some(ActiveLpJob { job, printer: printer_name });
                    true
                }
                Err(e) => {
                    self.status_notice = Some(format!("Print error: {e:#}"));
                    false
                }
            }
        }
    }

    pub(super) fn execute_print_from_preview(&mut self) {
        let Some(pp) = self.print_preview.as_ref() else { return };
        if pp.resolved_pages.is_empty() {
            self.status_notice = Some(
                pp.range_error
                    .clone()
                    .unwrap_or_else(|| "No pages selected to print".to_string()),
            );
            self.update_title_bar();
            self.request_redraw();
            return;
        }
        let printer = pp.printers.get(pp.selected_printer).cloned();
        let page_indices = pp.resolved_pages.clone();
        let copies = pp.copies;
        let paper = pp.paper;
        let duplex = pp.duplex;
        let manual_duplex = pp.manual_duplex;
        let render_opts = pp.render_opts;
        let title = self.filename_display();
        self.close_print_preview();

        if manual_duplex {
            let passes = build_manual_duplex_passes(&page_indices, render_opts.booklet, true);
            let started = self.dispatch_print_slots(
                printer.clone(),
                format!("{title} (Pass 1 - Fronts)"),
                passes.pass1_pages,
                copies,
                paper,
                DuplexMode::Off,
                render_opts,
            );
            if started {
                self.manual_duplex_wait = Some(ManualDuplexWaitState {
                    printer,
                    title,
                    resolved_pages: page_indices,
                    copies,
                    paper,
                    render_opts,
                    reverse_backs: true,
                    sheets_per_pass: passes.sheets_per_pass,
                });
            }
        } else {
            let slots: Vec<Option<u16>> = if render_opts.booklet {
                let sheets = compute_booklet_sheets(&page_indices);
                crate::print::flatten_booklet_sheets(&sheets)
            } else {
                page_indices.into_iter().map(Some).collect()
            };
            self.dispatch_print_slots(printer, title, slots, copies, paper, duplex, render_opts);
        }

        self.update_title_bar();
        self.request_redraw();
    }

    // -------------------------------------------------------------------------
    // Guided Manual Duplex (Simplex Printer) Pass 2 Confirmation Modal
    // -------------------------------------------------------------------------

    fn manual_duplex_modal_geometry(buf_w: u32, buf_h: u32, sf: f64) -> (i32, i32, u32, u32) {
        let s = sf as f32;
        let mw = ((470.0 * s).round() as u32).min(buf_w.saturating_sub(28));
        let mh = ((256.0 * s).round() as u32).min(buf_h.saturating_sub(28));
        let mx = (buf_w.saturating_sub(mw) / 2) as i32;
        let my = (buf_h.saturating_sub(mh) / 2) as i32;
        (mx, my, mw, mh)
    }

    pub(super) fn draw_manual_duplex_modal(
        &self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) {
        let Some(md) = self.manual_duplex_wait.as_ref() else { return };
        let theme = self.ui_theme();
        let s = sf as f32;
        blend_rect_rgba(buffer, buf_w, buf_h, 0, 0, buf_w, buf_h, theme.modal_scrim);

        let (mx, my, mw, mh) = Self::manual_duplex_modal_geometry(buf_w, buf_h, sf);
        let rad = (10.0 * s).clamp(6.0, 16.0);
        let pad = (18.0 * s).round() as i32;
        draw_soft_shadow(
            buffer, buf_w, buf_h, mx, my, mw, mh, rad,
            (12.0 * s).round() as i32, (4.0 * s).round() as i32, theme.shadow_rgb, theme.shadow_max_alpha,
        );
        fill_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.surface_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx, my, mw, mh, rad, theme.border_subtle);

        let title_sz = (14.5 * s).clamp(12.0, 30.0);
        let body_sz = (12.0 * s).clamp(10.0, 24.0);
        let small_sz = (11.0 * s).clamp(9.0, 22.0);

        draw_text(buffer, buf_w, buf_h, mx + pad, my + (16.0 * s).round() as i32, "Manual 2-Sided Printing — Pass 2 Ready", title_sz, theme.text_primary, true);
        fill_rect_clipped(buffer, buf_w, buf_h, mx + 1, my + (44.0 * s).round() as i32, mw.saturating_sub(2), 1, theme.divider);

        let l1 = format!("Pass 1 (Front sides: {} sheet(s)) has been sent to the printer.", md.sheets_per_pass);
        let l2 = if md.render_opts.booklet {
            "1. Wait for Pass 1 to finish, flip the stack along the short edge,"
        } else {
            "1. Wait for Pass 1 to finish, flip the printed stack,"
        };
        let l3 = "2. Reload it into the paper tray, then click 'Print Pass 2 (Backs)'.";
        draw_text_clipped(buffer, buf_w, buf_h, mx + pad, my + (56.0 * s).round() as i32, mx + mw as i32 - pad, &l1, body_sz, theme.text_primary, true);
        draw_text_clipped(buffer, buf_w, buf_h, mx + pad, my + (82.0 * s).round() as i32, mx + mw as i32 - pad, l2, small_sz, theme.text_secondary, false);
        draw_text_clipped(buffer, buf_w, buf_h, mx + pad, my + (102.0 * s).round() as i32, mx + mw as i32 - pad, l3, small_sz, theme.text_secondary, false);

        // Toggle: Reverse Back Pages (default ON)
        let tog_y = my + (134.0 * s).round() as i32;
        let tog_h = (28.0 * s).round() as u32;
        let tog_w = mw.saturating_sub((pad * 2) as u32);
        let tog_bg = if md.reverse_backs { theme.accent_soft_bg } else { theme.surface_elevated };
        let tog_bd = if md.reverse_backs { theme.accent } else { theme.border_subtle };
        fill_rounded_rect(buffer, buf_w, buf_h, mx + pad, tog_y, tog_w, tog_h, (6.0 * s).max(4.0), tog_bg);
        stroke_rounded_rect(buffer, buf_w, buf_h, mx + pad, tog_y, tog_w, tog_h, (6.0 * s).max(4.0), tog_bd);
        let tog_lbl = if md.reverse_backs {
            "✓ Reverse Pass 2 Order (Recommended for face-up trays)"
        } else {
            "Normal Pass 2 Order (Use for face-down reversing trays)"
        };
        draw_text_vcenter(buffer, buf_w, buf_h, mx + pad + (10.0 * s).round() as i32, tog_y, tog_h, mx + mw as i32 - pad - 4, tog_lbl, small_sz, if md.reverse_backs { theme.accent_soft_fg } else { theme.text_primary }, md.reverse_backs);

        // Footer buttons
        let btn_h = (32.0 * s).round() as u32;
        let by = my + mh as i32 - (46.0 * s).round() as i32;
        let cancel_w = (88.0 * s).round() as u32;
        let ok_w = (176.0 * s).round() as u32;
        let cancel_x = mx + mw as i32 - pad - cancel_w as i32;
        let ok_x = cancel_x - (10.0 * s).round() as i32 - ok_w as i32;

        let ok_hov = self.hovered_widget == Some(UiHit::ManualDuplex(ManualDuplexHit::PrintPass2));
        fill_rounded_rect(buffer, buf_w, buf_h, ok_x, by, ok_w, btn_h, (6.0 * s).max(4.0), if ok_hov { theme.accent_hover } else { theme.accent });
        let ok_lbl = "Print Pass 2 (Backs)";
        let ow = measure_text(ok_lbl, body_sz, true);
        draw_text_vcenter(buffer, buf_w, buf_h, ok_x + ((ok_w as f32 - ow) * 0.5).round() as i32, by, btn_h, ok_x + ok_w as i32, ok_lbl, body_sz, theme.text_on_accent, true);

        let c_hov = self.hovered_widget == Some(UiHit::ManualDuplex(ManualDuplexHit::Cancel));
        fill_rounded_rect(buffer, buf_w, buf_h, cancel_x, by, cancel_w, btn_h, (6.0 * s).max(4.0), if c_hov { theme.hover_bg } else { theme.surface_elevated });
        stroke_rounded_rect(buffer, buf_w, buf_h, cancel_x, by, cancel_w, btn_h, (6.0 * s).max(4.0), theme.border_subtle);
        let cw = measure_text("Cancel", body_sz, false);
        draw_text_vcenter(buffer, buf_w, buf_h, cancel_x + ((cancel_w as f32 - cw) * 0.5).round() as i32, by, btn_h, cancel_x + cancel_w as i32, "Cancel", body_sz, theme.text_primary, false);
    }

    pub(super) fn hit_test_manual_duplex_modal(
        &self,
        mx_px: f64,
        my_px: f64,
        buf_w: u32,
        buf_h: u32,
        sf: f64,
    ) -> Option<ManualDuplexHit> {
        self.manual_duplex_wait.as_ref()?;
        let s = sf as f32;
        let (mx, my, mw, mh) = Self::manual_duplex_modal_geometry(buf_w, buf_h, sf);
        let ix = mx_px.round() as i32;
        let iy = my_px.round() as i32;
        if ix < mx || ix >= mx + mw as i32 || iy < my || iy >= my + mh as i32 {
            return None;
        }
        let pad = (18.0 * s).round() as i32;
        let tog_y = my + (134.0 * s).round() as i32;
        let tog_h = (28.0 * s).round() as i32;
        if ix >= mx + pad && ix < mx + mw as i32 - pad && iy >= tog_y && iy < tog_y + tog_h {
            return Some(ManualDuplexHit::ToggleReverseBacks);
        }
        let btn_h = (32.0 * s).round() as i32;
        let by = my + mh as i32 - (46.0 * s).round() as i32;
        let cancel_w = (88.0 * s).round() as i32;
        let ok_w = (176.0 * s).round() as i32;
        let cancel_x = mx + mw as i32 - pad - cancel_w;
        let ok_x = cancel_x - (10.0 * s).round() as i32 - ok_w;
        if iy >= by && iy < by + btn_h {
            if ix >= ok_x && ix < ok_x + ok_w {
                return Some(ManualDuplexHit::PrintPass2);
            }
            if ix >= cancel_x && ix < cancel_x + cancel_w {
                return Some(ManualDuplexHit::Cancel);
            }
        }
        Some(ManualDuplexHit::InsideModal)
    }

    pub(super) fn handle_manual_duplex_click(&mut self, hit: ManualDuplexHit) {
        match hit {
            ManualDuplexHit::ToggleReverseBacks => {
                if let Some(md) = self.manual_duplex_wait.as_mut() {
                    md.reverse_backs = !md.reverse_backs;
                }
            }
            ManualDuplexHit::Cancel => {
                self.manual_duplex_wait = None;
                self.status_notice = Some("Manual 2-Sided Pass 2 cancelled".to_string());
            }
            ManualDuplexHit::PrintPass2 => {
                if let Some(md) = self.manual_duplex_wait.take() {
                    let passes = build_manual_duplex_passes(
                        &md.resolved_pages,
                        md.render_opts.booklet,
                        md.reverse_backs,
                    );
                    self.dispatch_print_slots(
                        md.printer,
                        format!("{} (Pass 2 - Backs)", md.title),
                        passes.pass2_pages,
                        md.copies,
                        md.paper,
                        DuplexMode::Off,
                        md.render_opts,
                    );
                }
            }
            ManualDuplexHit::InsideModal => {}
        }
        self.update_title_bar();
        self.request_redraw();
    }

    #[cfg(target_os = "windows")]
    fn execute_windows_system_print_dialog(&mut self) {
        let render_opts = self
            .print_preview
            .as_ref()
            .map(|p| p.render_opts)
            .unwrap_or_default();
        self.close_print_preview();
        let title = self.filename_display();
        let owner_hwnd = self.window.as_ref().and_then(|w| {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                _ => None,
            }
        });
        let total = self.total_pages();
        match crate::print::show_windows_print_dialog(&title, total, owner_hwnd) {
            Ok(None) => self.status_notice = Some("Print cancelled".to_string()),
            Err(e) => self.status_notice = Some(format!("Print error: {e:#}")),
            Ok(Some(setup)) => {
                let job_res = self.doc.as_ref().map(|doc| {
                    crate::print::PagedPrintJob::new_with_options(
                        self.engine,
                        doc,
                        &setup.page_indices,
                        setup.loop_copies,
                        setup.reported_copies,
                        self.redacted,
                        render_opts,
                        &std::env::temp_dir(),
                        Box::new(setup.sink),
                    )
                });
                if let Some(res) = job_res {
                    match res {
                        Ok(job) => self.start_print_job(job, title),
                        Err(e) => self.status_notice = Some(format!("Print error: {e:#}")),
                    }
                }
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }
}
