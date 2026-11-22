//! Tier 2.3: Side-by-Side PDF Compare with Synchronized Navigation & CPU Visual Diff Overlay.
//! Zero extra dependencies, zero GPU, 0 bytes RAM when closed.

use pdfium_render::prelude::*;
use std::path::PathBuf;

use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

/// Viewing / scaling mode inside Side-by-Side Compare Mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CompareFitMode {
    /// Fit full page height & width into each half-pane (multiplied by `zoom_level`).
    FitPage,
    /// Fit page width to each half-pane (multiplied by `zoom_level`), enabling synced vertical scroll.
    FitWidth,
    /// True 100% actual size (`96 DPI / 72 pt * sf * zoom_level`), enabling synced 2D scroll & pan.
    ActualSize,
}

/// Computes `(target_w, target_h)` in physical pixels for a compare pane given page dimensions in
/// PDF points, available pane area, viewing mode, custom `zoom_level`, and display `scale_factor`.
pub(super) fn compute_compare_render_dimensions(
    pw_pts: f32,
    ph_pts: f32,
    pane_w: u32,
    pane_h: u32,
    fit_mode: CompareFitMode,
    zoom_level: f32,
    sf: f64,
) -> (u32, u32) {
    let pw = pw_pts.max(1.0);
    let ph = ph_pts.max(1.0);
    let avail_w = pane_w.saturating_sub(24).max(32) as f32;
    let avail_h = pane_h.saturating_sub(36).max(32) as f32;
    let base_scale = match fit_mode {
        CompareFitMode::FitPage => (avail_w / pw).min(avail_h / ph),
        CompareFitMode::FitWidth => avail_w / pw,
        CompareFitMode::ActualSize => (96.0 / 72.0) * (sf.max(0.5) as f32),
    };
    let z = zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
    let scale = (base_scale * z).max(0.05);
    // Keep peak bitmap RAM bounded even at 400%+ zoom
    let max_dim = 4096.0f32;
    let raw_w = (pw * scale).max(16.0);
    let raw_h = (ph * scale).max(16.0);
    let clamp_factor = (max_dim / raw_w.max(raw_h)).min(1.0);
    let bw = (raw_w * clamp_factor).round().max(16.0) as u32;
    let bh = (raw_h * clamp_factor).round().max(16.0) as u32;
    (bw, bh)
}

pub(super) struct CompareCachedBitmap {
    pub(super) page_idx: u16,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixels: Vec<u32>,
}

pub(super) struct CompareModeState<'a> {
    pub(super) other_doc: PdfDocument<'a>,
    #[allow(dead_code)]
    pub(super) other_path: PathBuf,
    pub(super) other_name: String,
    pub(super) fit_mode: CompareFitMode,
    pub(super) visual_diff: bool,
    pub(super) page_offset: i16,
    pub(super) swapped: bool,
    pub(super) max_scroll_y: f32,
    pub(super) max_pan_x: f32,
    pub(super) pan_drag_last: Option<(f64, f64)>,
    pub(super) cached_left: Option<CompareCachedBitmap>,
    pub(super) cached_right: Option<CompareCachedBitmap>,
    pub(super) cached_diff: Option<(u16, u16, u32, u32, Vec<u32>, f32)>,
}

impl<'a> CompareModeState<'a> {
    pub(super) fn new(other_doc: PdfDocument<'a>, other_path: PathBuf) -> Self {
        let other_name = other_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Compare.pdf")
            .to_string();
        Self {
            other_doc,
            other_path,
            other_name,
            fit_mode: CompareFitMode::FitPage,
            visual_diff: false,
            page_offset: 0,
            swapped: false,
            max_scroll_y: 0.0,
            max_pan_x: 0.0,
            pan_drag_last: None,
            cached_left: None,
            cached_right: None,
            cached_diff: None,
        }
    }

    pub(super) fn invalidate_caches(&mut self) {
        self.cached_left = None;
        self.cached_right = None;
        self.cached_diff = None;
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CompareHit {
    FitPage,
    FitWidth,
    ActualSize,
    HandTool,
    ToggleDiff,
    OffsetMinus,
    OffsetPlus,
    SwapPanes,
    OpenOther,
    CloseCompare,
    InsideBar,
}

/// Computes a pixel-accurate CPU Visual Diff image (`0x00RRGGBB`) between two same-sized bitmaps
/// plus the percentage of differing pixels (`0.0..=100.0`).
/// - Identical pixels are softly muted toward neutral light gray so context remains readable.
/// - Darker ink unique to `left` is highlighted in crisp Red (`#DC2626`).
/// - Darker ink unique to `right` is highlighted in crisp Blue (`#2563EB`).
/// - Differing colored regions are highlighted in bright Magenta (`#C026D3`).
pub(super) fn compute_visual_diff_buffer(left: &[u32], right: &[u32]) -> (Vec<u32>, f32) {
    let n = left.len().min(right.len());
    if n == 0 {
        return (Vec::new(), 0.0);
    }
    let mut out = Vec::with_capacity(n);
    let mut diff_count = 0usize;
    for i in 0..n {
        let lp = left[i];
        let rp = right[i];
        let lr = ((lp >> 16) & 0xFF) as i32;
        let lg = ((lp >> 8) & 0xFF) as i32;
        let lb = (lp & 0xFF) as i32;
        let rr = ((rp >> 16) & 0xFF) as i32;
        let rg = ((rp >> 8) & 0xFF) as i32;
        let rb = (rp & 0xFF) as i32;

        let delta = (lr - rr).abs() + (lg - rg).abs() + (lb - rb).abs();
        if delta <= 28 {
            // Soft muted grayscale context for matching pixels
            let lum = ((lr * 77 + lg * 150 + lb * 29) >> 8).clamp(0, 255);
            let soft = ((lum * 3 + 245 * 5) / 8).clamp(0, 255) as u32;
            out.push((soft << 16) | (soft << 8) | soft);
        } else {
            diff_count += 1;
            let l_lum = lr * 77 + lg * 150 + lb * 29;
            let r_lum = rr * 77 + rg * 150 + rb * 29;
            if l_lum + 3500 < r_lum {
                // Darker on Left (removed in Right) -> Red
                out.push(0x00DC_2626);
            } else if r_lum + 3500 < l_lum {
                // Darker on Right (added in Right) -> Blue
                out.push(0x0025_63EB);
            } else {
                // Hue/color shift -> Magenta
                out.push(0x00C0_26D3);
            }
        }
    }
    let pct = (diff_count as f32 * 100.0) / (n as f32);
    (out, pct)
}

impl<'a> PdfViewerApp<'a> {
    pub(super) fn compare_bar_height_px(&self, sf: f64) -> u32 {
        if self.compare_mode.is_some() && !self.is_fullscreen && !self.is_presentation {
            (38.0 * sf).round() as u32
        } else {
            0
        }
    }

    pub(super) fn open_compare_with_path(&mut self, path: PathBuf) {
        if self.doc.is_none() {
            self.status_notice = Some("Open a PDF first before starting Compare Mode".to_string());
            self.request_redraw();
            return;
        }
        match self.engine.load_pdf_safe(&path) {
            Ok(other_doc) => {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document.pdf")
                    .to_string();
                self.close_all_popovers();
                self.view_mode = ViewMode::FitPage;
                self.scroll_mode = ScrollMode::SinglePage;
                self.zoom_level = 1.0;
                self.pan_x = 0.0;
                self.scroll_y = 0.0;
                self.scroll_x = 0.0;
                self.invalidate_page_caches();
                self.compare_mode = Some(CompareModeState::new(other_doc, path));
                self.status_notice = Some(format!(
                    "Compare Mode: '{}' vs '{name}' (Arrows sync pages, Esc exits)",
                    self.filename_display()
                ));
            }
            Err(e) => {
                self.status_notice = Some(format!("Compare PDF failed to load: {e}"));
            }
        }
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn close_compare_mode(&mut self) {
        if self.compare_mode.take().is_some() {
            self.invalidate_page_caches();
            self.status_notice = Some("Closed Side-by-Side Compare Mode".to_string());
            self.update_title_bar();
            self.request_redraw();
        }
    }

    fn compare_bar_layout(
        &self,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<(i32, i32, u32, u32, Vec<(i32, i32, u32, u32, CompareHit)>)> {
        let cmp = self.compare_mode.as_ref()?;
        let bar_h = self.compare_bar_height_px(sf);
        if bar_h == 0 {
            return None;
        }
        let s = sf as f32;
        let x = sidebar_w as i32;
        let y = top_bars_h as i32;
        let w = buf_w.saturating_sub(sidebar_w);
        let pad = (10.0 * s).round() as i32;
        let gap = (6.0 * s).round() as i32;
        let btn_h = (24.0 * s).round() as u32;
        let by = y + ((bar_h.saturating_sub(btn_h)) / 2) as i32;

        let close_w = (58.0 * s).round() as u32;
        let open_w = (82.0 * s).round() as u32;
        let swap_w = (50.0 * s).round() as u32;
        let sm_w = (22.0 * s).round() as u32;
        let off_lbl_w = (56.0 * s).round() as u32;
        let diff_w = (96.0 * s).round() as u32;
        let hand_w = (46.0 * s).round() as u32;
        let fit_w = (38.0 * s).round() as u32;
        let wid_w = (48.0 * s).round() as u32;
        let act_w = (44.0 * s).round() as u32;

        let close_x = x + w as i32 - pad - close_w as i32;
        let open_x = close_x - gap - open_w as i32;
        let swap_x = open_x - gap - swap_w as i32;
        let plus_x = swap_x - gap - sm_w as i32;
        let minus_x = plus_x - off_lbl_w as i32 - sm_w as i32;
        let diff_x = minus_x - gap - diff_w as i32;
        let hand_x = diff_x - gap - hand_w as i32;
        let act_x = hand_x - gap - act_w as i32;
        let wid_x = act_x - 3 - wid_w as i32;
        let fit_x = wid_x - 3 - fit_w as i32;
        let _ = cmp;

        let hits = vec![
            (fit_x, by, fit_w, btn_h, CompareHit::FitPage),
            (wid_x, by, wid_w, btn_h, CompareHit::FitWidth),
            (act_x, by, act_w, btn_h, CompareHit::ActualSize),
            (hand_x, by, hand_w, btn_h, CompareHit::HandTool),
            (diff_x, by, diff_w, btn_h, CompareHit::ToggleDiff),
            (minus_x, by, sm_w, btn_h, CompareHit::OffsetMinus),
            (plus_x, by, sm_w, btn_h, CompareHit::OffsetPlus),
            (swap_x, by, swap_w, btn_h, CompareHit::SwapPanes),
            (open_x, by, open_w, btn_h, CompareHit::OpenOther),
            (close_x, by, close_w, btn_h, CompareHit::CloseCompare),
        ];
        Some((x, y, w, bar_h, hits))
    }

    pub(super) fn hit_test_compare_bar(
        &self,
        mx: f64,
        my: f64,
        buf_w: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        sf: f64,
    ) -> Option<CompareHit> {
        let (x, y, w, h, hits) = self.compare_bar_layout(buf_w, sidebar_w, top_bars_h, sf)?;
        let ix = mx.round() as i32;
        let iy = my.round() as i32;
        if ix < x || ix >= x + w as i32 || iy < y || iy >= y + h as i32 {
            return None;
        }
        for (bx, by, bw, bh, hit) in hits {
            if ix >= bx && ix < bx + bw as i32 && iy >= by && iy < by + bh as i32 {
                return Some(hit);
            }
        }
        Some(CompareHit::InsideBar)
    }

    pub(super) fn set_compare_fit_mode(&mut self, mode: CompareFitMode) {
        if let Some(ref mut cmp) = self.compare_mode {
            cmp.fit_mode = mode;
            cmp.invalidate_caches();
        }
        match mode {
            CompareFitMode::FitPage => self.view_mode = ViewMode::FitPage,
            CompareFitMode::FitWidth => self.view_mode = ViewMode::FitWidth,
            CompareFitMode::ActualSize => {
                self.view_mode = ViewMode::FitPage;
                self.zoom_level = 1.0;
            }
        }
        self.scroll_y = 0.0;
        self.pan_x = 0.0;
        self.update_title_bar();
        self.request_redraw();
    }

    pub(super) fn handle_compare_bar_click(&mut self, hit: CompareHit) {
        match hit {
            CompareHit::FitPage => {
                self.zoom_level = 1.0;
                self.set_compare_fit_mode(CompareFitMode::FitPage);
            }
            CompareHit::FitWidth => {
                self.zoom_level = 1.0;
                self.set_compare_fit_mode(CompareFitMode::FitWidth);
            }
            CompareHit::ActualSize => {
                self.set_compare_fit_mode(CompareFitMode::ActualSize);
            }
            CompareHit::HandTool => {
                self.mouse_mode = if self.mouse_mode == MouseMode::Hand {
                    MouseMode::Select
                } else {
                    MouseMode::Hand
                };
                self.status_notice = Some(if self.mouse_mode == MouseMode::Hand {
                    "Compare Hand Tool: ON (Click & drag anywhere to pan both panes)".to_string()
                } else {
                    "Compare Hand Tool: OFF".to_string()
                });
                self.request_redraw();
            }
            CompareHit::ToggleDiff => {
                if let Some(ref mut cmp) = self.compare_mode {
                    cmp.visual_diff = !cmp.visual_diff;
                    cmp.cached_diff = None;
                }
                self.request_redraw();
            }
            CompareHit::OffsetMinus => {
                if let Some(ref mut cmp) = self.compare_mode {
                    cmp.page_offset = (cmp.page_offset - 1).clamp(-500, 500);
                    cmp.invalidate_caches();
                }
                self.request_redraw();
            }
            CompareHit::OffsetPlus => {
                if let Some(ref mut cmp) = self.compare_mode {
                    cmp.page_offset = (cmp.page_offset + 1).clamp(-500, 500);
                    cmp.invalidate_caches();
                }
                self.request_redraw();
            }
            CompareHit::SwapPanes => {
                if let Some(ref mut cmp) = self.compare_mode {
                    cmp.swapped = !cmp.swapped;
                    cmp.invalidate_caches();
                }
                self.request_redraw();
            }
            CompareHit::OpenOther => {
                self.pending_dialog = Some(PendingDialog::PickComparePdf);
                self.request_redraw();
            }
            CompareHit::CloseCompare => {
                self.close_compare_mode();
            }
            CompareHit::InsideBar => {}
        }
    }

    fn render_compare_doc_page(
        doc: &PdfDocument,
        page_idx: u16,
        bw: u32,
        bh: u32,
        reading_filter: ReadingFilter,
    ) -> Option<CompareCachedBitmap> {
        let total = doc.pages().len();
        if total == 0 || page_idx >= total {
            return None;
        }
        let page = doc.pages().get(page_idx).ok()?;
        let cfg = PdfRenderConfig::new()
            .set_fixed_width(bw.max(16) as i32)
            .set_fixed_height(bh.max(16) as i32)
            .render_form_data(true)
            .render_annotations(true);
        let bmp = page.render_with_config(&cfg).ok()?;
        let raw = bmp.as_raw_bytes();
        let w = bmp.width() as u32;
        let h = bmp.height() as u32;
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for chunk in raw.chunks_exact(4) {
            let (b, g, r) = (chunk[0], chunk[1], chunk[2]);
            let px = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            pixels.push(Self::apply_reading_filter_pixel(px, reading_filter));
        }
        Some(CompareCachedBitmap {
            page_idx,
            width: w,
            height: h,
            pixels,
        })
    }

    /// Synchronized scrolling inside Compare Mode; returns `true` if scroll consumed the delta.
    pub(super) fn scroll_compare_synced(&mut self, dx: f32, dy: f32) -> bool {
        let Some(cmp) = self.compare_mode.as_ref() else {
            return false;
        };
        let max_y = cmp.max_scroll_y;
        let max_x = cmp.max_pan_x;
        let mut consumed = false;
        if dx.abs() > 0.1 && max_x > 0.5 {
            let prev = self.pan_x;
            self.pan_x = (self.pan_x + dx).clamp(0.0, max_x);
            if (self.pan_x - prev).abs() > 0.1 {
                consumed = true;
            }
        }
        if dy.abs() > 0.1 && max_y > 0.5 {
            let prev = self.scroll_y;
            self.scroll_y = (self.scroll_y + dy).clamp(0.0, max_y);
            if (self.scroll_y - prev).abs() > 0.1 {
                consumed = true;
            }
        }
        if consumed {
            self.request_redraw();
        }
        consumed
    }

    /// Ensures left/right bitmaps (and optional Visual Diff buffer) are rendered for the current
    /// compare frame, then blits the split-pane comparison view + top Compare Bar into `buffer`.
    pub(super) fn render_and_draw_compare_mode(
        &mut self,
        buffer: &mut [u32],
        buf_w: u32,
        buf_h: u32,
        sidebar_w: u32,
        top_bars_h: u32,
        status_bar_h: u32,
        sf: f64,
    ) {
        let bar_h = self.compare_bar_height_px(sf);
        let content_x = sidebar_w;
        let content_y = top_bars_h + bar_h;
        let content_w = buf_w.saturating_sub(sidebar_w).max(2);
        let content_h = buf_h.saturating_sub(content_y + status_bar_h).max(2);
        let half_w = content_w / 2;
        let left_idx = self.current_page_index;

        let active_name = self.filename_display();
        let active_total = self.total_pages();
        let reading_filter = self.reading_filter;
        let zoom_level = self.zoom_level;
        let lbl_h = (22.0 * sf as f32).round() as u32;
        let area_y = content_y + lbl_h + 4;
        let area_h = content_h.saturating_sub(lbl_h + 8).max(16);

        // Render left & right bitmaps (cached until page, offset, mode, zoom, or pane size changes)
        if let (Some(main_doc), Some(cmp)) = (self.doc.as_ref(), self.compare_mode.as_mut()) {
            let other_total = cmp.other_doc.pages().len();
            let right_signed = (left_idx as i32) + (cmp.page_offset as i32);
            let right_valid = right_signed >= 0 && (right_signed as u16) < other_total;
            let right_idx = right_signed.clamp(0, other_total.saturating_sub(1) as i32) as u16;

            let (doc_a, idx_a, valid_a, doc_b, idx_b, valid_b) = if cmp.swapped {
                (&cmp.other_doc, right_idx, right_valid, main_doc, left_idx, left_idx < active_total)
            } else {
                (main_doc, left_idx, left_idx < active_total, &cmp.other_doc, right_idx, right_valid)
            };

            let pane_w_eff = half_w.saturating_sub(8).max(32);
            let pane_h_eff = content_h.saturating_sub(24).max(32);
            let fit_mode = cmp.fit_mode;

            let target_a = if valid_a {
                doc_a.pages().get(idx_a).ok().map(|p| {
                    compute_compare_render_dimensions(
                        p.width().value,
                        p.height().value,
                        pane_w_eff,
                        pane_h_eff,
                        fit_mode,
                        zoom_level,
                        sf,
                    )
                })
            } else {
                None
            };
            let target_b = if valid_b {
                doc_b.pages().get(idx_b).ok().map(|p| {
                    compute_compare_render_dimensions(
                        p.width().value,
                        p.height().value,
                        pane_w_eff,
                        pane_h_eff,
                        fit_mode,
                        zoom_level,
                        sf,
                    )
                })
            } else {
                None
            };

            if let Some((tw, th)) = target_a {
                let left_stale = cmp.cached_left.as_ref().map_or(true, |c| {
                    c.page_idx != idx_a || c.width != tw || c.height != th
                });
                if left_stale {
                    cmp.cached_left =
                        Self::render_compare_doc_page(doc_a, idx_a, tw, th, reading_filter);
                    cmp.cached_diff = None;
                }
            } else {
                cmp.cached_left = None;
                cmp.cached_diff = None;
            }

            if let Some((tw, th)) = target_b {
                let right_stale = cmp.cached_right.as_ref().map_or(true, |c| {
                    c.page_idx != idx_b || c.width != tw || c.height != th
                });
                if right_stale {
                    cmp.cached_right =
                        Self::render_compare_doc_page(doc_b, idx_b, tw, th, reading_filter);
                    cmp.cached_diff = None;
                }
            } else {
                cmp.cached_right = None;
                cmp.cached_diff = None;
            }

            // Compute synchronized scroll & pan bounds across both panes
            let max_bw = cmp
                .cached_left
                .as_ref()
                .map_or(0, |c| c.width)
                .max(cmp.cached_right.as_ref().map_or(0, |c| c.width));
            let max_bh = cmp
                .cached_left
                .as_ref()
                .map_or(0, |c| c.height)
                .max(cmp.cached_right.as_ref().map_or(0, |c| c.height));
            cmp.max_pan_x = (max_bw as f32 + 24.0 - half_w as f32).max(0.0);
            cmp.max_scroll_y = (max_bh as f32 + 16.0 - area_h as f32).max(0.0);
            self.pan_x = self.pan_x.clamp(0.0, cmp.max_pan_x);
            self.scroll_y = self.scroll_y.clamp(0.0, cmp.max_scroll_y);

            if cmp.visual_diff {
                if let (Some(lb), Some(rb)) = (cmp.cached_left.as_ref(), cmp.cached_right.as_ref()) {
                    let diff_stale = cmp
                        .cached_diff
                        .as_ref()
                        .map_or(true, |d| d.0 != idx_a || d.1 != idx_b || d.2 != lb.width || d.3 != lb.height);
                    if diff_stale {
                        let right_aligned: Vec<u32> = if rb.width == lb.width && rb.height == lb.height {
                            rb.pixels.clone()
                        } else {
                            let mut resized = vec![0x00FF_FFFFu32; (lb.width * lb.height) as usize];
                            for y in 0..lb.height {
                                let sy = ((y as u64 * rb.height as u64) / lb.height.max(1) as u64)
                                    .min(rb.height.saturating_sub(1) as u64) as u32;
                                for x in 0..lb.width {
                                    let sx = ((x as u64 * rb.width as u64) / lb.width.max(1) as u64)
                                        .min(rb.width.saturating_sub(1) as u64) as u32;
                                    resized[(y * lb.width + x) as usize] =
                                        rb.pixels[(sy * rb.width + sx) as usize];
                                }
                            }
                            resized
                        };
                        let (diff_px, pct) = compute_visual_diff_buffer(&lb.pixels, &right_aligned);
                        cmp.cached_diff = Some((idx_a, idx_b, lb.width, lb.height, diff_px, pct));
                    }
                }
            }
        }

        let theme = self.ui_theme();
        let s = sf as f32;
        let sub_sz = (10.5 * s).clamp(9.0, 20.0);
        let body_sz = (11.5 * s).clamp(10.0, 22.0);
        let bottom_limit = buf_h.saturating_sub(status_bar_h);
        let pan_x_px = self.pan_x.round() as i32;
        let scroll_y_px = self.scroll_y.round() as i32;

        // Draw center pane splitter
        fill_rect_clipped(
            buffer,
            buf_w,
            bottom_limit,
            (content_x + half_w) as i32,
            content_y as i32,
            1,
            content_h,
            theme.divider,
        );

        let Some(cmp) = self.compare_mode.as_ref() else {
            return;
        };
        let other_total = cmp.other_doc.pages().len();
        let right_signed = (left_idx as i32) + (cmp.page_offset as i32);
        let (left_title, right_title) = if cmp.swapped {
            (
                format!(
                    "Left: {} (p.{}/{})",
                    cmp.other_name,
                    (right_signed + 1).max(0),
                    other_total
                ),
                format!("Right: {} (p.{}/{})", active_name, left_idx + 1, active_total),
            )
        } else {
            (
                format!("Left: {} (p.{}/{})", active_name, left_idx + 1, active_total),
                format!(
                    "Right: {} (p.{}/{})",
                    cmp.other_name,
                    (right_signed + 1).max(0),
                    other_total
                ),
            )
        };

        // Blit helper for a pane with synchronized 2D pan/scroll clipped cleanly below pane header
        let blit_pane = |buffer: &mut [u32],
                         pane_x: u32,
                         pane_w: u32,
                         title: &str,
                         bmp: Option<(&[u32], u32, u32)>| {
            let clipped = truncate_ellipsis(title, (pane_w.saturating_sub(24)) as f32, sub_sz, true);
            draw_text_vcenter(
                buffer,
                buf_w,
                bottom_limit,
                pane_x as i32 + (12.0 * s).round() as i32,
                content_y as i32 + 2,
                lbl_h,
                (pane_x + pane_w) as i32 - 10,
                &clipped,
                sub_sz,
                theme.text_secondary,
                true,
            );
            if let Some((pixels, bw, bh)) = bmp {
                let ox = if bw + 24 <= pane_w {
                    pane_x as i32 + ((pane_w.saturating_sub(bw)) / 2) as i32
                } else {
                    pane_x as i32 + 12 - pan_x_px
                };
                let oy = if bh + 16 <= area_h {
                    area_y as i32 + ((area_h.saturating_sub(bh)) / 2) as i32
                } else {
                    area_y as i32 + 8 - scroll_y_px
                };
                let clip_top = area_y as i32;
                let clip_bot = bottom_limit as i32;
                let clip_left = pane_x as i32;
                let clip_right = (pane_x + pane_w) as i32;

                let r_start = (clip_top - oy).max(0) as u32;
                let r_end = (clip_bot - oy).max(0).min(bh as i32) as u32;
                let c_start = (clip_left - ox).max(0) as u32;
                let c_end = (clip_right - ox).max(0).min(bw as i32) as u32;
                if r_start < r_end && c_start < c_end {
                    let copy_w = (c_end - c_start) as usize;
                    for r in r_start..r_end {
                        let py = (oy + r as i32) as u32;
                        let dst_off = (py * buf_w) as usize + (ox + c_start as i32) as usize;
                        let src_off = (r * bw + c_start) as usize;
                        buffer[dst_off..dst_off + copy_w]
                            .copy_from_slice(&pixels[src_off..src_off + copy_w]);
                    }
                }
                // Crisp border clipped inside pane bounds
                if oy >= clip_top && oy < clip_bot {
                    let bx0 = ox.max(clip_left);
                    let bx1 = (ox + bw as i32).min(clip_right);
                    if bx1 > bx0 {
                        fill_rect_clipped(buffer, buf_w, bottom_limit, bx0, oy, (bx1 - bx0) as u32, 1, theme.page_border);
                    }
                }
                let bot_y = oy + bh as i32 - 1;
                if bot_y >= clip_top && bot_y < clip_bot {
                    let bx0 = ox.max(clip_left);
                    let bx1 = (ox + bw as i32).min(clip_right);
                    if bx1 > bx0 {
                        fill_rect_clipped(buffer, buf_w, bottom_limit, bx0, bot_y, (bx1 - bx0) as u32, 1, theme.page_border);
                    }
                }
            } else {
                let msg = "No matching page at this offset";
                let mw = measure_text(msg, body_sz, false);
                draw_text_vcenter(
                    buffer,
                    buf_w,
                    bottom_limit,
                    pane_x as i32 + ((pane_w as f32 - mw) * 0.5).round().max(12.0) as i32,
                    content_y as i32,
                    content_h,
                    (pane_x + pane_w) as i32 - 8,
                    msg,
                    body_sz,
                    theme.text_muted,
                    false,
                );
            }
        };

        let left_bmp = cmp
            .cached_left
            .as_ref()
            .map(|c| (c.pixels.as_slice(), c.width, c.height));
        let (right_hdr, right_bmp) = if cmp.visual_diff {
            if let Some((_, _, dw, dh, ref dpx, pct)) = cmp.cached_diff {
                (
                    format!("Visual Diff ({pct:.2}% changed — Red: Left, Blue: Right)"),
                    Some((dpx.as_slice(), dw, dh)),
                )
            } else {
                (
                    right_title,
                    cmp.cached_right
                        .as_ref()
                        .map(|c| (c.pixels.as_slice(), c.width, c.height)),
                )
            }
        } else {
            (
                right_title,
                cmp.cached_right
                    .as_ref()
                    .map(|c| (c.pixels.as_slice(), c.width, c.height)),
            )
        };

        blit_pane(buffer, content_x, half_w, &left_title, left_bmp);
        blit_pane(
            buffer,
            content_x + half_w + 1,
            content_w.saturating_sub(half_w + 1),
            &right_hdr,
            right_bmp,
        );

        // Draw top Compare Control Bar
        if let Some((bx, by, bw, bh, hits)) = self.compare_bar_layout(buf_w, sidebar_w, top_bars_h, sf) {
            fill_rect_clipped(buffer, buf_w, buf_h, bx, by, bw, bh, theme.topbar_bg);
            fill_rect_clipped(buffer, buf_w, buf_h, bx, by + bh as i32 - 1, bw, 1, theme.divider);

            let first_btn_x = hits.first().map(|h| h.0).unwrap_or(bx + bw as i32);
            let title_lbl = format!(
                "Compare ({:.0}%): {} vs {}",
                self.zoom_level * 100.0,
                active_name,
                cmp.other_name
            );
            let clipped_title = truncate_ellipsis(&title_lbl, (first_btn_x - bx - 16).max(40) as f32, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, bx + (12.0 * s).round() as i32, by, bh, first_btn_x - 8, &clipped_title, body_sz, theme.text_primary, true);

            let rad = (5.0 * s).max(3.0);
            let mut minus_end_x = 0i32;
            let mut plus_start_x = 0i32;
            for &(rx, ry, rw, rh, hit) in &hits {
                let hov = self.hovered_widget == Some(UiHit::Compare(hit));
                match hit {
                    CompareHit::FitPage | CompareHit::FitWidth | CompareHit::ActualSize => {
                        let (lbl, active) = match hit {
                            CompareHit::FitPage => ("Fit", cmp.fit_mode == CompareFitMode::FitPage),
                            CompareHit::FitWidth => ("Width", cmp.fit_mode == CompareFitMode::FitWidth),
                            _ => ("100%", cmp.fit_mode == CompareFitMode::ActualSize),
                        };
                        let bg = if active { theme.accent } else if hov { theme.hover_bg } else { theme.surface_elevated };
                        let fg = if active { theme.text_on_accent } else { theme.text_primary };
                        let bdr = if active { theme.accent } else { theme.border_subtle };
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bg);
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bdr);
                        let tw = measure_text(lbl, sub_sz, active);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, lbl, sub_sz, fg, active);
                    }
                    CompareHit::HandTool => {
                        let on = self.mouse_mode == MouseMode::Hand;
                        let bg = if on { theme.accent } else if hov { theme.hover_bg } else { theme.surface_elevated };
                        let fg = if on { theme.text_on_accent } else { theme.text_primary };
                        let bdr = if on { theme.accent } else { theme.border_subtle };
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bg);
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bdr);
                        let tw = measure_text("Hand", sub_sz, on);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "Hand", sub_sz, fg, on);
                    }
                    CompareHit::ToggleDiff => {
                        let on = cmp.visual_diff;
                        let bg = if on { theme.accent } else if hov { theme.hover_bg } else { theme.surface_elevated };
                        let fg = if on { theme.text_on_accent } else { theme.text_primary };
                        let bdr = if on { theme.accent } else { theme.border_subtle };
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bg);
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bdr);
                        let lbl = if on { "Diff: ON" } else { "Diff: OFF" };
                        let tw = measure_text(lbl, sub_sz, true);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, lbl, sub_sz, fg, true);
                    }
                    CompareHit::OffsetMinus => {
                        minus_end_x = rx + rw as i32;
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, theme.border_subtle);
                        let tw = measure_text("-", sub_sz, true);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "-", sub_sz, theme.text_primary, true);
                    }
                    CompareHit::OffsetPlus => {
                        plus_start_x = rx;
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, theme.border_subtle);
                        let tw = measure_text("+", sub_sz, true);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "+", sub_sz, theme.text_primary, true);
                    }
                    CompareHit::SwapPanes => {
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, theme.border_subtle);
                        let tw = measure_text("Swap", sub_sz, false);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "Swap", sub_sz, theme.text_primary, false);
                    }
                    CompareHit::OpenOther => {
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, if hov { theme.hover_bg } else { theme.surface_elevated });
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, theme.border_subtle);
                        let tw = measure_text("Change PDF", sub_sz, false);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "Change PDF", sub_sz, theme.text_primary, false);
                    }
                    CompareHit::CloseCompare => {
                        let bg = if hov { theme.danger } else { theme.danger_soft_bg };
                        let fg = if hov { theme.text_on_accent } else { theme.danger };
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bg);
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, theme.danger);
                        let tw = measure_text("Close", sub_sz, true);
                        draw_text_vcenter(buffer, buf_w, buf_h, rx + ((rw as f32 - tw) * 0.5).round() as i32, ry, rh, rx + rw as i32, "Close", sub_sz, fg, true);
                    }
                    CompareHit::InsideBar => {}
                }
            }
            if plus_start_x > minus_end_x {
                let off_lbl = format!("Off {:+}", cmp.page_offset);
                let tw = measure_text(&off_lbl, sub_sz, true);
                let cx = minus_end_x + (((plus_start_x - minus_end_x) as f32 - tw) * 0.5).round() as i32;
                draw_text_vcenter(buffer, buf_w, buf_h, cx, by, bh, plus_start_x, &off_lbl, sub_sz, theme.text_secondary, true);
            }
        }
    }
}
