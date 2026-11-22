//! Tier 2.3: Side-by-Side PDF Compare with Synchronized Navigation & CPU Visual Diff Overlay.
//! Zero extra dependencies, zero GPU, 0 bytes RAM when closed.

use pdfium_render::prelude::*;
use std::path::PathBuf;

use super::render::fill_rect_clipped;
use super::state::*;
use super::ui_draw::*;

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
    pub(super) visual_diff: bool,
    pub(super) page_offset: i16,
    pub(super) swapped: bool,
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
            visual_diff: false,
            page_offset: 0,
            swapped: false,
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

        let close_w = (68.0 * s).round() as u32;
        let open_w = (92.0 * s).round() as u32;
        let swap_w = (58.0 * s).round() as u32;
        let sm_w = (24.0 * s).round() as u32;
        let off_lbl_w = (68.0 * s).round() as u32;
        let diff_w = (128.0 * s).round() as u32;

        let close_x = x + w as i32 - pad - close_w as i32;
        let open_x = close_x - gap - open_w as i32;
        let swap_x = open_x - gap - swap_w as i32;
        let plus_x = swap_x - gap - sm_w as i32;
        let minus_x = plus_x - off_lbl_w as i32 - sm_w as i32;
        let diff_x = minus_x - gap - diff_w as i32;
        let _ = cmp;

        let hits = vec![
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

    pub(super) fn handle_compare_bar_click(&mut self, hit: CompareHit) {
        match hit {
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
        pane_w: u32,
        pane_h: u32,
        reading_filter: ReadingFilter,
    ) -> Option<CompareCachedBitmap> {
        let total = doc.pages().len();
        if total == 0 || page_idx >= total {
            return None;
        }
        let page = doc.pages().get(page_idx).ok()?;
        let pw = page.width().value.max(1.0);
        let ph = page.height().value.max(1.0);
        let avail_w = pane_w.saturating_sub(24).max(32) as f32;
        let avail_h = pane_h.saturating_sub(36).max(32) as f32;
        let scale = (avail_w / pw).min(avail_h / ph);
        let bw = (pw * scale).round().clamp(16.0, avail_w) as i32;
        let bh = (ph * scale).round().clamp(16.0, avail_h) as i32;

        let cfg = PdfRenderConfig::new()
            .set_target_width(bw)
            .set_target_height(bh)
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

        // Render left & right bitmaps (cached until page, offset, or pane size changes)
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

            let target_w = half_w.saturating_sub(8).max(32);
            let target_h = content_h.saturating_sub(24).max(32);

            let left_stale = cmp.cached_left.as_ref().map_or(true, |c| {
                c.page_idx != idx_a || c.width > target_w || c.height > target_h
            });
            if left_stale {
                cmp.cached_left = if valid_a {
                    Self::render_compare_doc_page(doc_a, idx_a, target_w, target_h, reading_filter)
                } else {
                    None
                };
                cmp.cached_diff = None;
            }

            let right_stale = cmp.cached_right.as_ref().map_or(true, |c| {
                c.page_idx != idx_b || c.width > target_w || c.height > target_h
            });
            if right_stale {
                cmp.cached_right = if valid_b {
                    Self::render_compare_doc_page(doc_b, idx_b, target_w, target_h, reading_filter)
                } else {
                    None
                };
                cmp.cached_diff = None;
            }

            if cmp.visual_diff {
                if let (Some(lb), Some(rb)) = (cmp.cached_left.as_ref(), cmp.cached_right.as_ref()) {
                    let diff_stale = cmp
                        .cached_diff
                        .as_ref()
                        .map_or(true, |d| d.0 != idx_a || d.1 != idx_b || d.2 != lb.width || d.3 != lb.height);
                    if diff_stale {
                        // Resample right onto left's exact dimensions if page sizes differ slightly
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

        // Blit helper for a pane
        let blit_pane = |buffer: &mut [u32],
                         pane_x: u32,
                         pane_w: u32,
                         title: &str,
                         bmp: Option<(&[u32], u32, u32)>| {
            let lbl_h = (22.0 * s).round() as u32;
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
                let area_y = content_y + lbl_h + 4;
                let area_h = content_h.saturating_sub(lbl_h + 8);
                let ox = pane_x as i32 + ((pane_w.saturating_sub(bw)) / 2) as i32;
                let oy = area_y as i32 + ((area_h.saturating_sub(bh)) / 2) as i32;
                draw_soft_shadow(
                    buffer,
                    buf_w,
                    bottom_limit,
                    ox,
                    oy,
                    bw,
                    bh,
                    2.0,
                    8,
                    3,
                    theme.shadow_rgb,
                    theme.shadow_max_alpha,
                );
                for r in 0..bh {
                    let py = oy + r as i32;
                    if py < content_y as i32 || py >= bottom_limit as i32 {
                        continue;
                    }
                    let dst_row = (py as u32 * buf_w) as usize;
                    let src_row = (r * bw) as usize;
                    for c in 0..bw {
                        let px = ox + c as i32;
                        if px < pane_x as i32 || px >= (pane_x + pane_w) as i32 {
                            continue;
                        }
                        buffer[dst_row + px as usize] = pixels[src_row + c as usize];
                    }
                }
                stroke_rounded_rect(buffer, buf_w, bottom_limit, ox, oy, bw, bh, 2.0, theme.page_border);
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
            let title_lbl = format!("Compare Mode: {}  vs  {}", active_name, cmp.other_name);
            let clipped_title = truncate_ellipsis(&title_lbl, (first_btn_x - bx - 20).max(40) as f32, body_sz, true);
            draw_text_vcenter(buffer, buf_w, buf_h, bx + (12.0 * s).round() as i32, by, bh, first_btn_x - 8, &clipped_title, body_sz, theme.text_primary, true);

            let rad = (5.0 * s).max(3.0);
            let mut minus_end_x = 0i32;
            let mut plus_start_x = 0i32;
            for &(rx, ry, rw, rh, hit) in &hits {
                let hov = self.hovered_widget == Some(UiHit::Compare(hit));
                match hit {
                    CompareHit::ToggleDiff => {
                        let on = cmp.visual_diff;
                        let bg = if on { theme.accent } else if hov { theme.hover_bg } else { theme.surface_elevated };
                        let fg = if on { theme.text_on_accent } else { theme.text_primary };
                        let bdr = if on { theme.accent } else { theme.border_subtle };
                        fill_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bg);
                        stroke_rounded_rect(buffer, buf_w, buf_h, rx, ry, rw, rh, rad, bdr);
                        let lbl = if on { "Visual Diff: ON" } else { "Visual Diff: OFF" };
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
