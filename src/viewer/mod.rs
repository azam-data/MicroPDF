mod book_ui;
mod bookmarks;
mod bottom_bar;
mod combine_modal;
mod comic_view;
mod compare_mode;
mod deskew;
mod doc_props_modal;
mod doc_tools_modal;
mod excel_export_modal;
mod favorites_modal;
mod font_dropdown;
mod grid;
mod image_tools_modal;
mod input;
mod markdown_export_modal;
mod markdown_view;
mod measure;
mod menu;
mod overlays;
mod print_preview;
pub(crate) mod render;
mod split_pages_modal;
mod stamp_manager_modal;
mod state;
mod statusbar;
mod tabs;
mod text_editor;
mod text_view;
mod theme;
mod toolbar;
pub(crate) mod ui_draw;
mod user_guide;
mod watermark_modal;

use crate::engine::PdfEngine;
use anyhow::{Context, Result};
use std::path::Path;
use winit::event_loop::EventLoop;

#[allow(unused_imports)]
pub use state::{MenuCategory, MouseMode, PdfViewerApp, ReadingFilter, ScrollMode, ViewMode};

/// Returns the current resident memory (RSS) of the process in megabytes (MB).
pub fn get_current_rss_mb() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct MachTaskBasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time: [u32; 2],
            system_time: [u32; 2],
            policy: i32,
            suspend_count: i32,
        }

        unsafe extern "C" {
            fn mach_task_self() -> u32;
            fn task_info(
                target_task: u32,
                flavor: u32,
                task_info_out: *mut MachTaskBasicInfo,
                task_info_outCnt: *mut u32,
            ) -> i32;
        }

        const MACH_TASK_BASIC_INFO: u32 = 20;
        const MACH_TASK_BASIC_INFO_COUNT: u32 =
            (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;

        let mut info = unsafe { std::mem::zeroed::<MachTaskBasicInfo>() };
        let mut count = MACH_TASK_BASIC_INFO_COUNT;
        let kr = unsafe {
            task_info(
                mach_task_self(),
                MACH_TASK_BASIC_INFO,
                &mut info,
                &mut count,
            )
        };
        if kr == 0 {
            return Some(info.resident_size as f64 / (1024.0 * 1024.0));
        }
        None
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = statm.split_whitespace().nth(1) {
                if let Ok(pages) = rss_pages.parse::<u64>() {
                    return Some((pages * 4096) as f64 / (1024.0 * 1024.0));
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }

        #[link(name = "psapi")]
        unsafe extern "system" {
            fn K32GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                ppsmemCounters: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }

        let mut pmc = unsafe { std::mem::zeroed::<ProcessMemoryCounters>() };
        pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) };
        if ok != 0 {
            return Some(pmc.working_set_size as f64 / (1024.0 * 1024.0));
        }
        None
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Attempts to forward `pdf_path` to an already-running MicroPDF instance over the local loopback socket.
/// Returns `true` if a running instance accepted the connection and payload (completes in < 10 ms, well under 200 ms).
pub fn try_forward_to_running_instance(pdf_path: &Path, custom_lock_path: Option<&Path>) -> bool {
    use std::io::Write;
    use std::net::{Shutdown, SocketAddr, TcpStream};
    use std::time::Duration;

    let lock_path = custom_lock_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(PdfViewerApp::default_single_instance_lock_path);
    let Ok(lock_content) = std::fs::read_to_string(&lock_path) else {
        return false;
    };
    let trimmed = lock_content.trim();
    let Some((port_str, token)) = trimmed.split_once('|') else {
        return false;
    };
    let Ok(port) = port_str.trim().parse::<u16>() else {
        return false;
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(100)) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(80)));

    let abs_path = std::fs::canonicalize(pdf_path).unwrap_or_else(|_| pdf_path.to_path_buf());
    let payload = format!(
        "{}|{}\n",
        token.trim(),
        abs_path.to_string_lossy().replace('\n', "").replace('\r', "")
    );
    if stream.write_all(payload.as_bytes()).is_err() || stream.flush().is_err() {
        return false;
    }
    let _ = stream.shutdown(Shutdown::Both);
    true
}

pub fn run_viewer(engine: &PdfEngine, initial_pdf: Option<&Path>) -> Result<()> {
    let event_loop = EventLoop::new().context("Failed to create winit EventLoop")?;
    let mut app = PdfViewerApp::new(engine, initial_pdf.map(|p| p.to_path_buf()))?;
    let _ = app.start_single_instance_listener(None);
    event_loop
        .run_app(&mut app)
        .context("Error running winit application event loop")?;
    app.stop_single_instance_listener();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfium_render::prelude::*;

    #[test]
    fn test_phase0_manual_checks_suite() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir();

        // Check 1: Scanned PDF redaction -> black under box, intact outside, no leftover .micropdf-tmp
        let scanned_path = tmp.join("phase0_scanned_check.pdf");
        {
            let mut doc = engine.pdfium.create_new_pdf()?;
            let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;
            let scan_img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                300,
                300,
                image::Rgba([40, 180, 220, 255]),
            ));
            let mut io = PdfPageImageObject::new_with_size(
                &doc,
                &scan_img,
                PdfPoints::new(300.0),
                PdfPoints::new(300.0),
            )?;
            io.translate(PdfPoints::new(100.0), PdfPoints::new(400.0))?;
            page.objects_mut().add_image_object(io)?;
            PdfEngine::save_doc(&doc, &scanned_path)?;
        }

        let mut app = PdfViewerApp::new(&engine, Some(scanned_path.clone()))?;
        let box_rect = PdfRect::new_from_values(500.0, 150.0, 600.0, 250.0);
        if let Some(doc) = app.doc.as_mut() {
            let removed = PdfEngine::redact_rect_on_page(doc, 0, box_rect)?;
            assert!(removed >= 1);
            app.redacted = true;
        }
        // Check 4: Save over currently open file + verify no .micropdf-tmp left behind
        if let Some(doc) = app.doc.as_ref() {
            assert!(
                PdfEngine::save_doc_with_metadata(
                    doc,
                    &scanned_path,
                    app.roman_end,
                    &app.bookmarks,
                    app.redacted
                )?
                .is_none()
            );
        }
        let tmp_artifact = tmp.join(".phase0_scanned_check.pdf.micropdf-tmp");
        assert!(!tmp_artifact.exists(), ".micropdf-tmp file left behind");

        // Reopen and verify area under box is black (0,0,0) and outside is intact
        let reopened = engine.load_pdf_safe(&scanned_path)?;
        let p0 = reopened.pages().get(0)?;
        let mut intact_outside = 0usize;
        for obj in p0.objects().iter() {
            if let Some(io) = obj.as_image_object() {
                let rgba = io.get_raw_image()?.to_rgba8();
                // Box is x=150..250 (u=50/300..150/300 -> px 50..150), y=500..600 (v=100/300..200/300 -> py 100..200)
                let mid_px = rgba.get_pixel(100, 150);
                assert_eq!([mid_px[0], mid_px[1], mid_px[2]], [0, 0, 0]);
                let out_px = rgba.get_pixel(10, 10);
                if out_px[2] > 200 {
                    intact_outside += 1;
                }
            }
        }
        assert!(intact_outside >= 1);

        // Check 2: Make 3 edits in Tab A, switch to Tab B and back, press Ctrl/Cmd+Z repeatedly
        let tab_b_path = tmp.join("phase0_tab_b.pdf");
        engine.create_sample_pdf(&tab_b_path, 2)?;
        // Tab A currently has 1 page. Make 3 edits (insert 3 blank pages -> 2, 3, 4 pages)
        app.insert_blank_page_after_current();
        assert_eq!(app.total_pages(), 2);
        app.insert_blank_page_after_current();
        assert_eq!(app.total_pages(), 3);
        app.insert_blank_page_after_current();
        assert_eq!(app.total_pages(), 4);
        assert_eq!(app.undo_stack.len(), 3);

        // Open Tab B, switch back to Tab A, and undo 3 times
        app.open_document_in_new_tab(tab_b_path.clone())?;
        assert_eq!(app.active_tab, 1);
        assert_eq!(app.total_pages(), 2);
        app.switch_to_tab(0);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.total_pages(), 4);
        assert_eq!(app.undo_stack.len(), 3);

        app.perform_undo();
        assert_eq!(app.total_pages(), 3);
        app.perform_undo();
        assert_eq!(app.total_pages(), 2);
        app.perform_undo();
        assert_eq!(app.total_pages(), 1);

        // Check 3: Digitally signed PDF + U warning on first press, proceeds on second press within 5s
        let signed_path = tmp.join("phase0_signed.pdf");
        {
            let raw = std::fs::read(&tab_b_path)?;
            let mut ldoc = lopdf::Document::load_mem(&raw)?;
            // Add a /Sig field widget annotation and AcroForm SignatureFlags so PDFium reports signatures().len() > 0
            let page_id = *ldoc.get_pages().get(&1).unwrap();
            let mut sig_val = lopdf::Dictionary::new();
            sig_val.set("Type", lopdf::Object::Name(b"Sig".to_vec()));
            sig_val.set("Filter", lopdf::Object::Name(b"Adobe.PPKLite".to_vec()));
            sig_val.set("SubFilter", lopdf::Object::Name(b"adbe.pkcs7.detached".to_vec()));
            let sig_val_id = ldoc.add_object(lopdf::Object::Dictionary(sig_val));

            let mut sig_annot = lopdf::Dictionary::new();
            sig_annot.set("Type", lopdf::Object::Name(b"Annot".to_vec()));
            sig_annot.set("Subtype", lopdf::Object::Name(b"Widget".to_vec()));
            sig_annot.set("FT", lopdf::Object::Name(b"Sig".to_vec()));
            sig_annot.set("T", lopdf::Object::string_literal("Sig1"));
            sig_annot.set("V", lopdf::Object::Reference(sig_val_id));
            sig_annot.set(
                "Rect",
                lopdf::Object::Array(vec![
                    lopdf::Object::Integer(50),
                    lopdf::Object::Integer(50),
                    lopdf::Object::Integer(150),
                    lopdf::Object::Integer(100),
                ]),
            );
            sig_annot.set("P", lopdf::Object::Reference(page_id));
            let sig_annot_id = ldoc.add_object(lopdf::Object::Dictionary(sig_annot));

            if let Ok(lopdf::Object::Dictionary(pdict)) = ldoc.get_object_mut(page_id) {
                pdict.set(
                    "Annots",
                    lopdf::Object::Array(vec![lopdf::Object::Reference(sig_annot_id)]),
                );
            }
            let mut acro = lopdf::Dictionary::new();
            acro.set(
                "Fields",
                lopdf::Object::Array(vec![lopdf::Object::Reference(sig_annot_id)]),
            );
            acro.set("SigFlags", lopdf::Object::Integer(3));
            let acro_id = ldoc.add_object(lopdf::Object::Dictionary(acro));
            if let Ok(cat) = ldoc.catalog_mut() {
                cat.set("AcroForm", lopdf::Object::Reference(acro_id));
            }
            let mut signed_bytes = Vec::new();
            ldoc.save_to(&mut signed_bytes)?;
            std::fs::write(&signed_path, &signed_bytes)?;
        }

        app.open_document_in_new_tab(signed_path.clone())?;
        let sig_count = app.doc.as_ref().map(|d| d.signatures().len()).unwrap_or(0);
        assert_eq!(sig_count, 1, "PDFium should detect 1 digital signature");

        // First press of U: warns and sets flatten_confirm_at without flattening
        app.flatten_and_unlock_current_pdf();
        assert!(app.flatten_confirm_at.is_some());
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("INVALIDATE")
        );

        // Second press of U within 5s: proceeds and clears flatten_confirm_at
        app.flatten_and_unlock_current_pdf();
        assert!(app.flatten_confirm_at.is_none());
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("Flattened annotations/forms")
        );

        let _ = std::fs::remove_file(&scanned_path);
        let _ = std::fs::remove_file(PdfEngine::bookmarks_sidecar_path(&scanned_path));
        let _ = std::fs::remove_file(&tab_b_path);
        let _ = std::fs::remove_file(PdfEngine::bookmarks_sidecar_path(&tab_b_path));
        let _ = std::fs::remove_file(&signed_path);
        let _ = std::fs::remove_file(PdfEngine::bookmarks_sidecar_path(&signed_path));
        Ok(())
    }

    #[test]
    fn test_continuous_scroll_eviction_and_centre_tracking() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_cont_scroll.pdf");
        engine.create_sample_pdf(&tmp, 20)?;

        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        let ram_before = get_current_rss_mb().unwrap_or(0.0);
        app.set_view_mode(ViewMode::ContinuousScroll);
        assert_eq!(app.view_mode, ViewMode::ContinuousScroll);

        // Render initial frame at 1024x1080
        app.ensure_current_page_rendered(1024, 1080, 0, 52, 22)?;
        assert_eq!(app.current_page_index, 0);
        let first_vis = app.continuous_draw_rects.first().unwrap().0;
        let last_vis = app.continuous_draw_rects.last().unwrap().0;
        assert!(app.continuous_page_cache.len() <= (last_vis - first_vis + 3) as usize);

        // Scroll down through 10 pages smoothly and verify cache eviction and page counter tracking
        for _ in 0..50 {
            app.scroll_or_turn_page_vertical(220.0);
            app.ensure_current_page_rendered(1024, 1080, 0, 52, 22)?;
            let fv = app.continuous_draw_rects.first().unwrap().0;
            let lv = app.continuous_draw_rects.last().unwrap().0;
            let keep_start = fv.saturating_sub(1);
            let keep_end = (lv + 1).min(19);
            for &cached_idx in app.continuous_page_cache.keys() {
                assert!(
                    cached_idx >= keep_start && cached_idx <= keep_end,
                    "Page {cached_idx} outside allowed range {keep_start}..={keep_end}"
                );
            }
        }
        assert!(app.current_page_index >= 8);
        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 1 Continuous Scroll RAM] before: {:.2} MB, after scrolling 10+ pages: {:.2} MB (cached pages: {})",
            ram_before,
            ram_after,
            app.continuous_page_cache.len()
        );

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    #[test]
    fn test_zoom_tile_rendering_and_lru_48mb_cap() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_zoom_tiles.pdf");
        engine.create_sample_pdf(&tmp, 4)?;

        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        app.ensure_current_page_rendered(1024, 1080, 0, 52, 22)?;
        let ram_before = get_current_rss_mb().unwrap_or(0.0);

        // Zoom to 400% around cursor (500, 500) -> above 2x screen size, must use 512x512 tiles only
        app.set_zoom_around_cursor(4.0, 500.0, 500.0);
        assert!((app.zoom_level - 4.0).abs() < 0.001);
        app.ensure_current_page_rendered(1024, 1080, 0, 52, 22)?;
        let cache = app.cached_page.as_ref().unwrap();
        assert!(
            cache.pixels.is_empty(),
            "Full page bitmap must NEVER be allocated above 2x screen size"
        );
        assert!(!app.tile_cache.is_empty(), "Visible tiles must be rendered");
        assert!(app.tile_cache.len() <= state::MAX_TILE_CACHE_ENTRIES);

        // Zoom to 800% (max) and pan across the entire huge virtual page to exceed 48 tiles and trigger LRU eviction
        app.set_zoom_around_cursor(10.0, 500.0, 500.0); // Clamps to 8.0 (800%)
        assert!((app.zoom_level - 8.0).abs() < 0.001);
        for x_step in 0..8 {
            for y_step in 0..8 {
                app.pan_x = (x_step * 512) as f32;
                app.scroll_y = (y_step * 512) as f32;
                app.ensure_current_page_rendered(1024, 1080, 0, 52, 22)?;
                assert!(
                    app.tile_cache.len() <= state::MAX_TILE_CACHE_ENTRIES,
                    "Tile cache exceeded 48 MB cap: {} entries",
                    app.tile_cache.len()
                );
            }
        }
        let ram_at_800_zoom = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 2 Zoom RAM] before: {:.2} MB, after panning full page at 800% zoom: {:.2} MB (tiles cached: {}/{})",
            ram_before,
            ram_at_800_zoom,
            app.tile_cache.len(),
            state::MAX_TILE_CACHE_ENTRIES
        );

        // Reset zoom ('0')
        app.reset_zoom();
        assert!((app.zoom_level - 1.0).abs() < 0.001);
        assert!(app.tile_cache.is_empty());

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    #[test]
    fn test_links_and_history_cap() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_links_history.pdf");
        engine.create_sample_pdf(&tmp, 10)?;

        // Add an external URI link annotation on page 0 using PDFium
        {
            let mut doc = engine.load_pdf_safe(&tmp)?;
            let mut p0 = doc.pages_mut().get(0)?;
            let mut annot = p0
                .annotations_mut()
                .create_link_annotation("https://example.com/micropdf")?;
            annot.set_bounds(PdfRect::new_from_values(400.0, 100.0, 450.0, 300.0))?;
            PdfEngine::save_doc(&doc, &tmp)?;
        }

        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        let ram_before = get_current_rss_mb().unwrap_or(0.0);
        app.ensure_current_page_rendered(1024, 1080, 0, 52, 22)?;

        // Convert PDF link midpoint (200, 425) to screen coordinates and verify link resolution
        let (dx, dy, bw, bh) = app.page_draw_rect;
        let sx = dx as f64 + (200.0 / 595.0) * (bw as f64);
        let sy = dy as f64 + (1.0 - (425.0 / 842.0)) * (bh as f64);
        let resolved = app.resolve_link_at_screen_pos(sx, sy);
        assert_eq!(
            resolved,
            Some(state::ResolvedLinkTarget::ExternalUrl(
                "https://example.com/micropdf".to_string()
            ))
        );

        // Push 65 navigation jumps and verify history is strictly capped at 50 entries
        for i in 0..65u16 {
            app.push_nav_history();
            app.current_page_index = (i + 1) % 10;
            app.scroll_y = (i as f32) * 3.5;
            assert!(
                app.nav_back_stack.len() + app.nav_forward_stack.len() <= state::MAX_NAV_HISTORY
            );
        }
        assert_eq!(app.nav_back_stack.len(), state::MAX_NAV_HISTORY);

        let cur_before_back = (app.current_page_index, app.scroll_y);
        app.navigate_back();
        assert_ne!((app.current_page_index, app.scroll_y), cur_before_back);
        assert!(app.nav_back_stack.len() + app.nav_forward_stack.len() <= state::MAX_NAV_HISTORY);

        app.navigate_forward();
        assert_eq!((app.current_page_index, app.scroll_y), cur_before_back);
        assert!(app.nav_back_stack.len() + app.nav_forward_stack.len() <= state::MAX_NAV_HISTORY);

        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 3 Links + History RAM] before: {:.2} MB, after 65 history entries & link test: {:.2} MB",
            ram_before, ram_after
        );

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    #[test]
    fn test_text_selection_word_double_click_and_page_eviction() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_text_select.pdf");
        engine.create_sample_pdf(&tmp, 3)?;

        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        let ram_before = get_current_rss_mb().unwrap_or(0.0);
        assert_eq!(
            app.mouse_mode,
            MouseMode::Select,
            "Select must be the default mouse mode"
        );
        app.ensure_current_page_rendered(1024, 1080, 0, 52, 22)?;

        // Drag-select text across the entire page 0
        let count = app.select_text_in_pdf_rect(
            0,
            PdfRect::new_from_values(0.0, 0.0, 842.0, 595.0),
        );
        assert!(count > 0, "Drag selection must select text on page 0");
        assert!(app.selected_text.contains("MicroPDF"));
        assert_eq!(app.selected_text_page, Some(0));
        assert!(!app.selected_text_rects.is_empty());
        assert_eq!(app.cached_char_boxes.as_ref().map(|(p, _)| *p), Some(0));

        // Double-click word selection at the first non-whitespace character's center
        let word_pt = app
            .cached_char_boxes
            .as_ref()
            .unwrap()
            .1
            .iter()
            .find(|c| c.ch.is_alphabetic() && c.rect.is_some())
            .and_then(|c| c.rect)
            .unwrap();
        let mid_x = PdfPoints::new((word_pt.left().value + word_pt.right().value) * 0.5);
        let mid_y = PdfPoints::new((word_pt.bottom().value + word_pt.top().value) * 0.5);
        assert!(app.select_word_at_pdf_point(0, mid_x, mid_y));
        assert!(!app.selected_text.is_empty());
        assert!(!app.selected_text.contains(' '));

        let ram_during = get_current_rss_mb().unwrap_or(0.0);

        // RAM rule: Navigating to another page immediately frees character boxes and clears selection
        app.navigate_to_page(1);
        assert!(
            app.cached_char_boxes.is_none(),
            "Character boxes must be freed immediately on page change"
        );
        assert!(app.selected_text.is_empty());
        assert!(app.selected_text_rects.is_empty());

        eprintln!(
            "[Feature 4 Text Selection RAM] before: {:.2} MB, during selection: {:.2} MB",
            ram_before, ram_during
        );

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    #[test]
    fn test_password_prompt_masked_3_attempts() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_encrypted_pwd.pdf");
        {
            // Create a valid PDF and attach a standard /Encrypt dictionary so PDFium returns PasswordError
            engine.create_sample_pdf(&tmp, 1)?;
            let raw = std::fs::read(&tmp)?;
            let mut ldoc = lopdf::Document::load_mem(&raw)?;
            let mut enc = lopdf::Dictionary::new();
            enc.set("Filter", lopdf::Object::Name(b"Standard".to_vec()));
            enc.set("V", lopdf::Object::Integer(2));
            enc.set("R", lopdf::Object::Integer(3));
            enc.set("Length", lopdf::Object::Integer(128));
            enc.set("P", lopdf::Object::Integer(-4));
            enc.set("O", lopdf::Object::String(vec![0x11; 32], lopdf::StringFormat::Hexadecimal));
            enc.set("U", lopdf::Object::String(vec![0x22; 32], lopdf::StringFormat::Hexadecimal));
            let enc_id = ldoc.add_object(lopdf::Object::Dictionary(enc));
            ldoc.trailer.set("Encrypt", lopdf::Object::Reference(enc_id));
            ldoc.trailer.set(
                "ID",
                lopdf::Object::Array(vec![
                    lopdf::Object::String(vec![0x33; 16], lopdf::StringFormat::Hexadecimal),
                    lopdf::Object::String(vec![0x33; 16], lopdf::StringFormat::Hexadecimal),
                ]),
            );
            let mut out = Vec::new();
            ldoc.save_to(&mut out)?;
            std::fs::write(&tmp, &out)?;
        }

        let ram_before = get_current_rss_mb().unwrap_or(0.0);
        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        assert!(
            matches!(
                app.input_mode,
                state::InputMode::PasswordPrompt {
                    attempts_left: 3,
                    ..
                }
            ),
            "Opening encrypted PDF must enter PasswordPrompt with 3 attempts left"
        );

        // Type secret password characters and verify prompt text is masked with '*'
        app.handle_key_input(winit::keyboard::Key::Character("s".into()), Some("s"));
        app.handle_key_input(winit::keyboard::Key::Character("e".into()), Some("e"));
        app.handle_key_input(winit::keyboard::Key::Character("c".into()), Some("c"));
        let prompt_str = app.format_active_prompt_text().unwrap();
        assert!(prompt_str.contains("***_"));
        assert!(!prompt_str.contains("sec"));

        // Attempt 1 (wrong password) -> 2 attempts left
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
            None,
        );
        assert!(matches!(
            app.input_mode,
            state::InputMode::PasswordPrompt {
                attempts_left: 2,
                ref password_input,
                ..
            } if password_input.is_empty()
        ));

        // Attempt 2 (wrong password) -> 1 attempt left
        app.handle_key_input(winit::keyboard::Key::Character("x".into()), Some("x"));
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
            None,
        );
        assert!(matches!(
            app.input_mode,
            state::InputMode::PasswordPrompt {
                attempts_left: 1,
                ref password_input,
                ..
            } if password_input.is_empty()
        ));

        // Attempt 3 (wrong password) -> exits PasswordPrompt back to Normal
        app.handle_key_input(winit::keyboard::Key::Character("y".into()), Some("y"));
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
            None,
        );
        assert_eq!(app.input_mode, state::InputMode::Normal);
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("maximum 3 password attempts exceeded")
        );

        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 5 Password Prompt RAM] before: {:.2} MB, after 3 attempts: {:.2} MB",
            ram_before, ram_after
        );

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    #[test]
    fn test_session_restore_and_recent_files_ini() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp_dir = std::env::temp_dir().join("micropdf_session_test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let ini_path = tmp_dir.join("micropdf.ini");
        let _ = std::fs::remove_file(&ini_path);

        // Create 12 small test PDFs to verify Recent Files cap at 10
        let mut pdf_paths = Vec::new();
        for i in 0..12 {
            let p = tmp_dir.join(format!("recent_{i}.pdf"));
            engine.create_sample_pdf(&p, 5)?;
            pdf_paths.push(p);
        }

        let ram_before = get_current_rss_mb().unwrap_or(0.0);
        let mut app = PdfViewerApp::new(&engine, Some(pdf_paths[0].clone()))?;
        app.custom_session_ini_path = Some(ini_path.clone());
        for p in &pdf_paths[1..12] {
            app.record_recent_file(p);
        }
        assert_eq!(
            app.recent_files.len(),
            state::MAX_RECENT_FILES,
            "Recent files must be capped at 10"
        );
        let file_menu_items = app.menu_items_for_category(state::MenuCategory::File);
        assert!(file_menu_items.iter().any(|s| s.starts_with("Recent 1:")));
        assert!(file_menu_items.iter().any(|s| s.starts_with("Recent 10:")));

        // Open a second tab, set different pages and view modes, and save session to micropdf.ini
        app.navigate_to_page(3);
        app.set_view_mode(ViewMode::FitWidth);
        app.open_document_in_new_tab(pdf_paths[1].clone())?;
        app.navigate_to_page(2);
        app.set_view_mode(ViewMode::ContinuousScroll);
        app.save_session_to_ini();
        assert!(ini_path.exists(), "micropdf.ini must be written on save_session_to_ini");

        // Launch a new empty viewer instance and load the saved session -> offers to restore
        let mut app2 = PdfViewerApp::new(&engine, None)?;
        app2.custom_session_ini_path = Some(ini_path.clone());
        app2.load_session_from_ini(true);
        assert_eq!(
            app2.input_mode,
            state::InputMode::RestoreSessionPrompt { tab_count: 2 }
        );
        assert_eq!(app2.recent_files.len(), 10);

        // Press Enter to confirm restoring the session
        app2.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
            None,
        );
        assert_eq!(app2.tabs.len(), 2);
        assert_eq!(app2.active_tab, 1);
        assert_eq!(app2.current_page_index, 2);
        assert_eq!(app2.view_mode, ViewMode::ContinuousScroll);
        app2.switch_to_tab(0);
        assert_eq!(app2.current_page_index, 3);
        assert_eq!(app2.view_mode, ViewMode::FitWidth);

        // Verify fallback to OS config folder when portable path is unwritable (e.g., points to a directory)
        app2.custom_session_ini_path = Some(tmp_dir.clone()); // Writing a file over a directory fails
        app2.save_session_to_ini();
        assert!(
            app2.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("OS config folder"),
            "Fallback to OS config folder must notify the user in the status bar"
        );
        let _ = std::fs::remove_file(PdfViewerApp::fallback_os_config_ini_path());

        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 6 Session Restore + Recent Files RAM] before: {:.2} MB, after restoring 2 tabs & 10 recent files: {:.2} MB",
            ram_before, ram_after
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
        Ok(())
    }

    #[test]
    fn test_single_instance_socket_forwarding_under_200ms() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp_dir = std::env::temp_dir().join("micropdf_single_inst_test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let lock_path = tmp_dir.join("instance.lock");
        let pdf_a = tmp_dir.join("first.pdf");
        let pdf_b = tmp_dir.join("second.pdf");
        engine.create_sample_pdf(&pdf_a, 2)?;
        engine.create_sample_pdf(&pdf_b, 4)?;

        let ram_before = get_current_rss_mb().unwrap_or(0.0);
        let mut app = PdfViewerApp::new(&engine, Some(pdf_a.clone()))?;
        app.start_single_instance_listener(Some(lock_path.clone()))?;
        assert_eq!(app.tabs.len(), 1);

        // Simulate a second process forwarding pdf_b and verify it completes in < 200 ms
        let t0 = std::time::Instant::now();
        let forwarded = try_forward_to_running_instance(&pdf_b, Some(&lock_path));
        let elapsed = t0.elapsed();
        assert!(forwarded, "Second instance must successfully forward path to primary");
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "Second instance forwarding took {:?} (must be < 200 ms)",
            elapsed
        );

        // Primary instance polls listener and opens pdf_b in Tab 2. Reads are non-blocking
        // (Phase 4) and loopback data can arrive just after the client's write returns, so
        // poll until it opens, within the same 200 ms budget.
        let t_poll = std::time::Instant::now();
        let mut opened = 0;
        while opened == 0 && t_poll.elapsed() < std::time::Duration::from_millis(200) {
            opened += app.poll_single_instance_listener();
            if opened == 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(opened, 1);
        // Further polls must not open the same file again.
        for _ in 0..5 {
            assert_eq!(app.poll_single_instance_listener(), 0, "forwarded file opened twice");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 1);
        assert_eq!(app.total_pages(), 4);

        app.stop_single_instance_listener();
        assert!(!lock_path.exists(), "Lock file must be cleaned up on stop");

        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 7 Single Instance RAM] before: {:.2} MB, after forwarding new tab: {:.2} MB (forward latency: {:.2?})",
            ram_before, ram_after, elapsed
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
        Ok(())
    }

    #[test]
    fn test_auto_reload_preserves_page_scroll_and_respects_unsaved_edits() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_auto_reload.pdf");
        engine.create_sample_pdf(&tmp, 3)?;

        let ram_before = get_current_rss_mb().unwrap_or(0.0);
        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        app.navigate_to_page(2);
        app.scroll_y = 145.5;
        assert_eq!(app.total_pages(), 3);
        assert!(!app.has_unsaved_edits);

        // Simulate external modification on disk (e.g. LaTeX recompile producing 6 pages)
        std::thread::sleep(std::time::Duration::from_millis(20));
        engine.create_sample_pdf(&tmp, 6)?;
        // Force stored mtime 2 seconds in the past so filesystem mtime granularity never flakes
        app.last_file_mtime = Some(
            std::time::SystemTime::now() - std::time::Duration::from_secs(2),
        );

        // 1. While unfocused, poll_auto_reload_if_due must not reload
        app.window_focused = false;
        app.last_mtime_poll = std::time::Instant::now() - std::time::Duration::from_secs(2);
        app.poll_auto_reload_if_due();
        assert_eq!(app.total_pages(), 3, "Must not poll/reload while window is unfocused");

        // 2. When focused and no unsaved edits, reloads and preserves page & scroll_y
        //    v1.2.1: a stale pre-reload snapshot must not survive (Ctrl+Z would revert the change)
        app.undo_stack.push_back(state::UndoSnapshot {
            pdf_bytes: Some(b"stale snapshot".to_vec()),
            bookmarks: Vec::new(),
            bw_pages: std::collections::HashSet::new(),
            roman_end: 0,
            skip_pages: 0,
            redacted: false,
            generation: 0,
        });
        app.window_focused = true;
        app.poll_auto_reload_if_due();
        assert!(app.undo_stack.is_empty(), "undo history must be cleared on auto-reload");
        assert_eq!(app.total_pages(), 6, "Must auto-reload updated 6-page PDF from disk");
        assert_eq!(app.current_page_index, 2, "Must preserve current_page_index");
        assert!((app.scroll_y - 145.5).abs() < 0.01, "Must preserve scroll_y");
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("Auto-reloaded")
        );

        // 3. Make an unsaved edit (insert blank page -> 7 pages), then modify disk file (to 4 pages):
        // must NOT reload and must show status-bar notice about unsaved edits
        app.insert_blank_page_after_current();
        assert_eq!(app.total_pages(), 7);
        assert!(app.has_unsaved_edits);

        engine.create_sample_pdf(&tmp, 4)?;
        app.last_file_mtime = Some(
            std::time::SystemTime::now() - std::time::Duration::from_secs(2),
        );
        app.last_mtime_poll = std::time::Instant::now() - std::time::Duration::from_secs(2);
        app.poll_auto_reload_if_due();
        assert_eq!(
            app.total_pages(),
            7,
            "Must NOT overwrite unsaved in-memory edits on disk change"
        );
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("unsaved edits")
        );

        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 8 Auto-Reload RAM] before: {:.2} MB, after auto-reload & unsaved check: {:.2} MB",
            ram_before, ram_after
        );

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    #[test]
    fn test_fullscreen_and_presentation_mode() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_fullscreen_presentation.pdf");
        engine.create_sample_pdf(&tmp, 5)?;

        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        app.show_bookmarks = true;
        app.set_view_mode(ViewMode::ContinuousScroll);
        app.zoom_level = 1.75;
        app.scroll_y = 140.0;
        app.pan_x = 30.0;

        let ram_before = get_current_rss_mb().unwrap_or(0.0);

        // 1. Toggle Fullscreen (F11): hides top menu bar, tab bar, status bar, and bookmark sidebar
        assert_eq!(app.menu_bar_height_px(1.0), state::MENU_BAR_HEIGHT_LOGICAL);
        assert_eq!(app.tab_bar_height_px(1.0), state::TAB_BAR_HEIGHT_LOGICAL);
        assert_eq!(app.status_bar_height_px(1.0), state::STATUS_BAR_HEIGHT_LOGICAL);
        assert!(app.sidebar_width_px(1.0) > 0);

        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::F11),
            None,
        );
        assert!(app.is_fullscreen);
        assert!(!app.is_presentation);
        assert_eq!(app.menu_bar_height_px(1.0), 0);
        assert_eq!(app.tab_bar_height_px(1.0), 0);
        assert_eq!(app.status_bar_height_px(1.0), 0);
        assert_eq!(app.sidebar_width_px(1.0), 0);

        // F11 again exits fullscreen
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::F11),
            None,
        );
        assert!(!app.is_fullscreen);
        assert_eq!(app.menu_bar_height_px(1.0), state::MENU_BAR_HEIGHT_LOGICAL);

        // 2. Enter Presentation Mode (F5): forces FitPage, zoom 1.0, hides all bars, renders full screen
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::F5),
            None,
        );
        assert!(app.is_presentation);
        assert!(app.is_fullscreen);
        assert_eq!(app.view_mode, ViewMode::FitPage);
        assert!((app.zoom_level - 1.0).abs() < 1e-4);
        assert_eq!(app.menu_bar_height_px(1.0), 0);
        assert_eq!(app.status_bar_height_px(1.0), 0);

        app.ensure_current_page_rendered(1920, 1080, 0, 0, 0)?;
        assert!(app.cached_page.is_some());

        // Arrows advance / step back pages in Presentation Mode
        assert_eq!(app.current_page_index, 0);
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowRight),
            None,
        );
        assert_eq!(app.current_page_index, 1);

        // Esc exits Presentation Mode and restores previous view mode, zoom, scroll, and pan
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape),
            None,
        );
        assert!(!app.is_presentation);
        assert!(!app.is_fullscreen);
        assert_eq!(app.view_mode, ViewMode::ContinuousScroll);
        assert!((app.zoom_level - 1.75).abs() < 1e-4);
        assert!((app.scroll_y - 140.0).abs() < 1e-4);
        assert!((app.pan_x - 30.0).abs() < 1e-4);

        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 9 Fullscreen/Presentation RAM] before: {:.2} MB, after F11/F5/Esc cycle: {:.2} MB",
            ram_before, ram_after
        );

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    #[test]
    fn test_incremental_search_time_sliced_and_esc_cancel() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir().join("test_incremental_search.pdf");
        engine.create_sample_pdf(&tmp, 12)?;

        let mut app = PdfViewerApp::new(&engine, Some(tmp.clone()))?;
        let ram_before = get_current_rss_mb().unwrap_or(0.0);

        // 1. Simulate time-sliced incremental search (3 pages per frame slice)
        let query = "Page";
        let q_chars: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
        app.search_query = query.to_string();
        app.search_matches.clear();
        app.current_search_match = 0;
        app.incremental_search = Some(state::IncrementalSearchState {
            query: query.to_string(),
            q_chars: q_chars.clone(),
            case_sensitive: false,
            whole_word: false,
            next_page: 0,
            total_pages: 12,
            origin_page: 0,
            jumped_to_first: false,
        });

        let still_running =
            app.step_incremental_search_with_limit(std::time::Duration::from_secs(1), 3);
        assert!(still_running, "Should still be searching after 3 of 12 pages");
        assert_eq!(
            app.incremental_search.as_ref().map(|s| s.next_page),
            Some(3)
        );
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("Searching 'Page' (Page 3/12"),
            "Status bar must show incremental search progress"
        );
        let matches_after_3_pages = app.search_matches.len();
        assert!(matches_after_3_pages >= 3);

        // 2. Pressing Esc cancels the in-progress search while preserving matches found so far
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape),
            None,
        );
        assert!(
            app.incremental_search.is_none(),
            "Esc must cancel active incremental search"
        );
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("Search cancelled at Page 3/12")
        );
        assert_eq!(app.search_matches.len(), matches_after_3_pages);

        // 3. Full search completes across slices and finds all 12 pages without caching per-page text
        app.execute_text_search("Page");
        while app.incremental_search.is_some() {
            app.step_incremental_search(std::time::Duration::from_millis(8));
        }
        assert_eq!(app.search_matches.len(), matches_after_3_pages * 4);
        assert!(
            app.cached_char_boxes.is_none(),
            "Incremental search must never populate or retain per-page character caches"
        );

        let ram_after = get_current_rss_mb().unwrap_or(0.0);
        eprintln!(
            "[Feature 10 Incremental Search RAM] before: {:.2} MB, after 12-page incremental search: {:.2} MB (matches: {})",
            ram_before,
            ram_after,
            app.search_matches.len()
        );

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    /// Cache caps hold while scrolling, zooming and browsing the grid. (v1.2.1: RSS figures were
    /// removed from this test: inside a shared test process they mostly measure other tests.
    /// Real RAM is measured by `ram_realistic_measurement` below, in its own process.)
    #[test]
    fn test_phase4_cache_caps_hold() -> Result<()> {
        let engine = PdfEngine::new()?;
        let path = std::env::temp_dir().join("micropdf_cache_caps.pdf");
        PdfEngine::write_atomic(&path, &build_text_pdf(60)?)?;
        let (vw, vh, sw, th, sb) = (1024u32, 768u32, 0u32, 52u32, 22u32);
        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
        app.set_view_mode(ViewMode::ContinuousScroll);
        for step in 0..50u16 {
            app.current_page_index = step;
            app.scroll_y = 120.0;
            app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            assert!(app.continuous_page_cache.len() <= 5);
        }
        app.set_view_mode(ViewMode::FitPage);
        app.set_zoom_around_cursor(4.0, 512.0, 384.0);
        for k in 0..8 {
            app.pan_x = k as f32 * 320.0;
            app.scroll_y = k as f32 * 280.0;
            app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            assert!(app.tile_cache.len() <= state::MAX_TILE_CACHE_ENTRIES);
        }
        app.reset_zoom();
        app.set_view_mode(ViewMode::ThumbnailGrid);
        let mut gs = 0.0f32;
        while gs < 20_000.0 {
            app.grid_scroll_y = gs;
            app.update_visible_thumbnails(vw, vh, sw, th, sb);
            assert!(app.thumbnail_cache.len() <= state::MAX_THUMB_CACHE_ENTRIES);
            gs += 600.0;
        }
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn test_phase1_optimize_menu_step_and_cancel() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_phase1_opt_menu");
        let _ = std::fs::create_dir_all(&dir);
        let src_path = dir.join("scan_doc.pdf");
        let opt_path = dir.join("scan_doc_optimized.pdf");
        let _ = std::fs::remove_file(&src_path);
        let _ = std::fs::remove_file(&opt_path);

        let scan_bytes = build_scan_pdf(3)?;
        PdfEngine::write_atomic(&src_path, &scan_bytes)?;

        let mut app = PdfViewerApp::new(&engine, Some(src_path.clone()))?;
        app.ensure_current_page_rendered(1024, 768, 0, 52, 22)?;

        // 1. Open Tools -> Optimize...
        let opt_idx = state::MenuCategory::Tools
            .items()
            .iter()
            .position(|&s| s == "Optimize...")
            .expect("Optimize... must exist in Tools menu");
        app.execute_menu_item(state::MenuCategory::Tools, opt_idx);
        assert_eq!(
            app.input_mode,
            state::InputMode::OptimizePrompt {
                preset: crate::optimize::OptimizePreset::Balanced,
                mode: crate::optimize::ColorMode::Color,
                clean_scans: true,
                previewing: false,
            }
        );
        let prompt0 = app.format_active_prompt_text().unwrap();
        assert!(prompt0.contains("Balanced(200dpi)"));
        assert!(prompt0.contains("Color"));
        assert!(prompt0.contains("Clean:ON"));

        // 2. Toggle preset ('1' -> Smaller), colour mode ('b' -> B/W), clean scans ('s' -> OFF)
        app.handle_key_input(winit::keyboard::Key::Character("1".into()), Some("1"));
        app.handle_key_input(winit::keyboard::Key::Character("b".into()), Some("b"));
        app.handle_key_input(winit::keyboard::Key::Character("s".into()), Some("s"));
        let prompt_bw = app.format_active_prompt_text().unwrap();
        assert!(prompt_bw.contains("Smaller(150dpi)"));
        assert!(
            prompt_bw.contains("signatures will look jagged"),
            "B/W mode must show 'signatures will look jagged' warning, got: {prompt_bw}"
        );
        assert!(prompt_bw.contains("Clean:OFF"));

        // 3. Toggle Preview current page ('p' -> ON, then 'p' -> OFF)
        app.handle_key_input(winit::keyboard::Key::Character("p".into()), Some("p"));
        assert!(app.format_active_prompt_text().unwrap().contains("[P]Preview:ON"));
        assert!(app.cached_page.is_some());
        app.handle_key_input(winit::keyboard::Key::Character("p".into()), Some("p"));
        assert!(!app.format_active_prompt_text().unwrap().contains("[P]Preview:ON"));

        // 4. Switch back to Balanced + Color + Clean:ON, press Enter -> starts OptimizeJob
        app.handle_key_input(winit::keyboard::Key::Character("2".into()), Some("2"));
        app.handle_key_input(winit::keyboard::Key::Character("c".into()), Some("c"));
        app.handle_key_input(winit::keyboard::Key::Character("s".into()), Some("s"));
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
            None,
        );
        assert_eq!(app.input_mode, state::InputMode::Normal);
        assert!(app.optimize_job.is_some(), "Enter must start an active OptimizeJob");
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("Optimizing image 0 / 3")
        );

        // Step once (1 of 3 images) and verify progress string "Optimizing image 1 / 3"
        assert!(app.step_optimize_job(), "Should still have 2 images remaining");
        assert!(
            app.status_notice
                .as_deref()
                .unwrap_or("")
                .contains("Optimizing image 1 / 3"),
            "got status: {:?}",
            app.status_notice
        );

        // Press Esc to cancel -> job is dropped and nothing is written
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape),
            None,
        );
        assert!(app.optimize_job.is_none(), "Esc must drop the active OptimizeJob");
        assert!(!opt_path.exists(), "Cancelled OptimizeJob must write nothing");

        // 5. Re-open Tools -> Optimize... and run all steps to completion
        app.execute_menu_item(state::MenuCategory::Tools, opt_idx);
        app.handle_key_input(
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
            None,
        );
        let mut steps = 0usize;
        while app.optimize_job.is_some() {
            app.step_optimize_job();
            steps += 1;
        }
        assert_eq!(steps, 3, "Must execute exactly 1 step per image");
        assert!(opt_path.exists(), "Finished OptimizeJob must write <name>_optimized.pdf");
        assert_eq!(
            std::fs::read(&src_path)?,
            scan_bytes,
            "Original PDF on disk must never be modified"
        );
        let done_msg = app.status_notice.as_deref().unwrap_or("");
        assert!(
            done_msg.contains("Optimized") && done_msg.contains("->") && done_msg.contains("scan_doc_optimized.pdf"),
            "Completion notice must show before/after sizes and output filename, got: {done_msg}"
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// `n`-page text PDF whose page `i` reads "`label` page `i`". Each content stream carries
    /// about 16 KB of comment padding, so PDFium's read-ahead cannot already hold the bytes of
    /// pages far from the one viewed at open.
    fn build_labeled_padded_pdf(n: u32, label: &str) -> Result<Vec<u8>> {
        use lopdf::dictionary;
        let mut ldoc = lopdf::Document::with_version("1.5");
        let pages_id = ldoc.new_object_id();
        let font_id = ldoc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let resources_id = ldoc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let padding = format!("% {}\n", "x".repeat(16 * 1024));
        let mut kids = Vec::new();
        for i in 1..=n {
            let content = format!("{padding}BT /F1 14 Tf 72 720 Td ({label} page {i}) Tj ET\n{padding}");
            let content_id = ldoc.add_object(lopdf::Stream::new(dictionary! {}, content.into_bytes()));
            kids.push(
                ldoc.add_object(dictionary! {
                    "Type" => "Page", "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                    "Resources" => resources_id, "Contents" => content_id,
                })
                .into(),
            );
        }
        ldoc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }),
        );
        let catalog_id = ldoc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        ldoc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        ldoc.save_to(&mut out)?;
        Ok(out)
    }

    /// Extracted text of page `idx` in the active document, trimmed.
    fn active_page_text(app: &PdfViewerApp, idx: u16) -> Result<String> {
        let doc = app.doc.as_ref().context("no document")?;
        let page = doc.pages().get(idx)?;
        Ok(page.text()?.all().trim().to_string())
    }

    /// Navigates to `idx` and renders it the way a frame does, recording any error in the
    /// status bar. Returns whether the page bitmap was produced.
    fn render_page_like_frame(app: &mut PdfViewerApp, idx: u16) -> bool {
        app.navigate_to_page(idx);
        app.render_page_view_or_notice(1024, 768, 0, 52, 22)
    }

    /// (a) Saving over the open file (atomic temp + rename), then rendering pages never viewed
    /// before the save: they must show the saved content (the app reloads after a save), and a
    /// second document opened on the old file must keep showing the old content.
    #[test]
    fn test_streaming_save_over_open_file_then_render_unviewed_pages() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_stream_save_over_open");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        std::fs::write(&path, build_labeled_padded_pdf(20, "Alpha")?)?;

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        assert!(render_page_like_frame(&mut app, 0));
        // A second handle on the same file, like another tab, with no page ever loaded.
        let other = engine.load_pdf_safe(&path)?;

        // Unsaved edit: blank page after page 1, so the saved file differs from the old one.
        app.insert_blank_page_after_current();
        assert_eq!(app.total_pages(), 21);
        app.save_active_doc_to(path.clone());
        assert!(!app.has_unsaved_edits, "save failed: {:?}", app.status_notice);
        assert!(
            !PdfEngine::save_temp_path(&path).exists(),
            "atomic save must not leave its temp file behind"
        );

        // Saved content: page 2 is the blank page, so old page i is now at index i.
        assert_eq!(app.total_pages(), 21);
        for idx in 10..21u16 {
            assert!(
                render_page_like_frame(&mut app, idx),
                "page {} failed after save: {:?}",
                idx + 1,
                app.status_notice
            );
            assert_eq!(active_page_text(&app, idx)?, format!("Alpha page {idx}"));
        }

        // The other handle still reads the replaced file's old bytes: old content, not garbage.
        for idx in 10..20u16 {
            let text = other.pages().get(idx)?.text()?.all();
            assert_eq!(text.trim(), format!("Alpha page {}", idx + 1));
        }

        drop(other);
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// (b) Another process overwrites the open file in place (truncate + write, as
    /// `cp other.pdf open.pdf` does). Unviewed pages must render the old content or fail with
    /// a status-bar error, never show the new file's bytes at the old offsets, and auto-reload
    /// must then pick up the new file. Covers a shorter, a same-size (identical layout, so old
    /// offsets land on valid objects of the new file) and a longer replacement.
    #[test]
    fn test_streaming_open_file_overwritten_in_place() -> Result<()> {
        for new_pages in [7u32, 20, 30] {
            overwrite_in_place_case(new_pages)?;
        }
        Ok(())
    }

    fn overwrite_in_place_case(new_pages: u32) -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join(format!("micropdf_stream_overwrite_{new_pages}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        std::fs::write(&path, build_labeled_padded_pdf(20, "Alpha")?)?;

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        assert!(render_page_like_frame(&mut app, 0));

        // In place: same inode, truncated, then rewritten with a different PDF.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(&path)?;
            f.write_all(&build_labeled_padded_pdf(new_pages, "Bravo")?)?;
            f.sync_all()?;
        }

        let mut failed = 0;
        for idx in 10..20u16 {
            let rendered = render_page_like_frame(&mut app, idx);
            if !rendered {
                failed += 1;
                let notice = app.status_notice.clone().unwrap_or_default();
                assert!(
                    notice.contains(&format!("Cannot render page {}", idx + 1)),
                    "a failed render must say so in the status bar, got: {notice:?}"
                );
            }
            match active_page_text(&app, idx) {
                Ok(text) => assert_eq!(
                    text,
                    format!("Alpha page {}", idx + 1),
                    "[{new_pages}-page replacement] page {} shows bytes of the new file",
                    idx + 1
                ),
                Err(_) => assert!(!rendered, "page {} rendered but its text is unreadable", idx + 1),
            }
        }
        assert!(
            failed > 0,
            "[{new_pages}-page replacement] reads after the overwrite must be refused"
        );

        // Auto-reload picks up the new file.
        app.last_file_mtime = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(2));
        app.window_focused = true;
        app.last_mtime_poll = std::time::Instant::now() - std::time::Duration::from_secs(2);
        app.poll_auto_reload_if_due();
        assert_eq!(app.total_pages(), new_pages as u16, "auto-reload must load the new file");
        for idx in [0u16, new_pages as u16 - 1] {
            assert!(render_page_like_frame(&mut app, idx), "{:?}", app.status_notice);
            assert_eq!(active_page_text(&app, idx)?, format!("Bravo page {}", idx + 1));
        }

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// (c) The open file is deleted. Nothing may crash. An open handle keeps a deleted file's
    /// data readable (POSIX unlink; Windows FILE_SHARE_DELETE), so unviewed pages render the
    /// original content; a page that cannot be read reports an error in the status bar.
    #[test]
    fn test_streaming_open_file_deleted() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_stream_deleted");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        std::fs::write(&path, build_labeled_padded_pdf(20, "Alpha")?)?;

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        assert!(render_page_like_frame(&mut app, 0));
        std::fs::remove_file(&path)?;
        assert!(!path.exists());

        for idx in 10..20u16 {
            let rendered = render_page_like_frame(&mut app, idx);
            if rendered {
                assert_eq!(active_page_text(&app, idx)?, format!("Alpha page {}", idx + 1));
            } else {
                let notice = app.status_notice.clone().unwrap_or_default();
                assert!(notice.contains(&format!("Cannot render page {}", idx + 1)), "{notice:?}");
            }
            #[cfg(unix)]
            assert!(rendered, "an unlinked file stays readable through the open handle");
        }

        // Auto-reload must leave the open document alone while the file is missing.
        app.last_file_mtime = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(2));
        app.window_focused = true;
        app.last_mtime_poll = std::time::Instant::now() - std::time::Duration::from_secs(2);
        app.poll_auto_reload_if_due();
        assert_eq!(app.total_pages(), 20);

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Simulates FAT32/exFAT (and Windows without POSIX rename): replacing a file fails while
    /// this process still has it open.
    fn rename_refusing_open_target(from: &Path, to: &Path) -> std::io::Result<()> {
        if crate::engine::open_file_registry::is_open(to) {
            return Err(std::io::Error::other("simulated: the file is in use"));
        }
        std::fs::rename(from, to)
    }

    fn rename_always_failing(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("simulated: access denied"))
    }

    fn pdf_page_count(path: &Path, engine: &PdfEngine) -> Result<u16> {
        Ok(engine.load_pdf_from_bytes_safe(std::fs::read(path)?)?.pages().len())
    }

    /// Rename refused while the file is open: the save closes the app's handles on that file
    /// (the active tab and a clean second tab on the same file), renames, and reopens them.
    #[test]
    fn test_save_over_open_file_when_fs_refuses_replacing_open_files() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_save_refuse_open");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        std::fs::write(&path, build_labeled_padded_pdf(20, "Alpha")?)?;

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        app.open_document_in_new_tab(path.clone())?;
        assert_eq!((app.tabs.len(), app.active_tab), (2, 1));
        assert!(crate::engine::open_file_registry::is_open(&path));
        app.rename_for_save = rename_refusing_open_target;

        assert!(render_page_like_frame(&mut app, 0));
        app.insert_blank_page_after_current();
        app.save_active_doc_to(path.clone());

        assert!(!app.has_unsaved_edits, "save failed: {:?}", app.status_notice);
        assert!(app.status_notice.as_deref().unwrap_or("").starts_with("Saved PDF"));
        assert!(!PdfEngine::save_temp_path(&path).exists(), "temp file must be renamed away");
        assert_eq!(pdf_page_count(&path, &engine)?, 21, "the file on disk is the saved version");

        // Active tab reopened from the saved file: unviewed pages render the saved content.
        assert_eq!(app.total_pages(), 21);
        for idx in [10u16, 20] {
            assert!(render_page_like_frame(&mut app, idx), "{:?}", app.status_notice);
            assert_eq!(active_page_text(&app, idx)?, format!("Alpha page {idx}"));
        }
        // The other tab on the same file was reopened too.
        app.switch_to_tab(0);
        assert_eq!(app.total_pages(), 21);
        assert!(render_page_like_frame(&mut app, 15), "{:?}", app.status_notice);

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Rename keeps failing: the original is untouched, the saved copy stays next to it, the
    /// status bar names it, and the tab keeps working (from the copy) with its edits unsaved.
    /// A later save that can rename then succeeds normally.
    #[test]
    fn test_save_when_replace_keeps_failing_keeps_original_and_copy() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_save_rename_fails");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        let original = build_labeled_padded_pdf(20, "Alpha")?;
        std::fs::write(&path, &original)?;
        let temp = PdfEngine::save_temp_path(&path);

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        app.rename_for_save = rename_always_failing;
        assert!(render_page_like_frame(&mut app, 0));
        app.insert_blank_page_after_current();
        app.save_active_doc_to(path.clone());

        assert_eq!(std::fs::read(&path)?, original, "original must be byte-for-byte unchanged");
        assert!(temp.exists(), "the saved copy must stay next to the original");
        assert_eq!(pdf_page_count(&temp, &engine)?, 21);
        let notice = app.status_notice.clone().unwrap_or_default();
        assert!(notice.contains("original is unchanged"), "{notice}");
        assert!(notice.contains(&temp.display().to_string()), "must name the copy: {notice}");
        assert!(notice.contains("simulated: access denied"), "{notice}");
        assert!(app.has_unsaved_edits, "the edits did not reach the file");
        assert_eq!(app.file_path, path, "the tab still belongs to the original file");

        // The tab keeps the edits and can render pages it never showed.
        assert_eq!(app.total_pages(), 21);
        assert!(render_page_like_frame(&mut app, 18), "{:?}", app.status_notice);
        assert_eq!(active_page_text(&app, 18)?, "Alpha page 18");

        // Next save on a file system that allows the rename.
        app.rename_for_save = |from, to| std::fs::rename(from, to);
        app.save_active_doc_to(path.clone());
        assert!(!app.has_unsaved_edits, "{:?}", app.status_notice);
        assert!(!temp.exists());
        assert_eq!(pdf_page_count(&path, &engine)?, 21);
        assert!(render_page_like_frame(&mut app, 20), "{:?}", app.status_notice);

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// A second tab on the same file with unsaved edits of its own is never closed to make the
    /// rename work; the save then fails safely and that tab keeps its edits.
    #[test]
    fn test_save_never_drops_another_tabs_unsaved_edits() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_save_dirty_tab");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        let original = build_labeled_padded_pdf(20, "Alpha")?;
        std::fs::write(&path, &original)?;

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        app.insert_blank_page_after_current(); // tab 0: 21 pages, unsaved
        app.open_document_in_new_tab(path.clone())?;
        app.delete_current_page(); // tab 1: 19 pages, unsaved
        assert_eq!(app.total_pages(), 19);
        app.rename_for_save = rename_refusing_open_target;
        app.save_active_doc_to(path.clone());

        assert_eq!(std::fs::read(&path)?, original);
        assert!(PdfEngine::save_temp_path(&path).exists());
        assert!(app.has_unsaved_edits);
        assert_eq!(app.total_pages(), 19);
        app.switch_to_tab(0);
        assert!(app.has_unsaved_edits, "the other tab's edits must survive");
        assert_eq!(app.total_pages(), 21);

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// `write_atomic` never falls back to rewriting the target in place.
    #[test]
    fn test_write_atomic_failed_rename_leaves_target_and_no_temp() -> Result<()> {
        let dir = std::env::temp_dir().join("micropdf_write_atomic_fail");
        let _ = std::fs::remove_dir_all(&dir);
        // A non-empty directory cannot be replaced by a file on any platform.
        let target = dir.join("busy.pdf");
        std::fs::create_dir_all(&target)?;
        std::fs::write(target.join("keep.txt"), b"keep")?;
        assert!(PdfEngine::write_atomic(&target, b"%PDF-1.4 new").is_err());
        assert_eq!(std::fs::read(target.join("keep.txt"))?, b"keep");
        assert!(!PdfEngine::save_temp_path(&target).exists(), "temp must be removed");
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    fn lock_port_and_token(lock: &Path) -> (u16, String) {
        let s = std::fs::read_to_string(lock).unwrap();
        let (port, token) = s.trim().split_once('|').unwrap();
        (port.parse().unwrap(), token.to_string())
    }

    fn send_ipc_line(port: u16, line: &str) {
        use std::io::Write;
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(line.as_bytes()).unwrap();
        let _ = s.shutdown(std::net::Shutdown::Both);
    }

    #[test]
    fn test_single_instance_token_is_random() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_si_token");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let (lock_a, lock_b) = (dir.join("a.lock"), dir.join("b.lock"));
        let mut a = PdfViewerApp::new(&engine, None)?;
        let mut b = PdfViewerApp::new(&engine, None)?;
        a.start_single_instance_listener(Some(lock_a.clone()))?;
        b.start_single_instance_listener(Some(lock_b.clone()))?;
        let (_, ta) = lock_port_and_token(&lock_a);
        let (_, tb) = lock_port_and_token(&lock_b);
        for t in [&ta, &tb] {
            assert_eq!(t.len(), 32, "16 random bytes as hex: {t}");
            assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_ne!(ta, tb);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&lock_a)?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "lock file holds the token: owner-only");
        }
        a.stop_single_instance_listener();
        b.stop_single_instance_listener();
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn test_single_instance_opens_only_existing_pdfs_with_the_token() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_si_accept");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let lock = dir.join("i.lock");
        let pdf = dir.join("ok.PDF");
        engine.create_sample_pdf(&pdf, 2)?;
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, b"hello")?;
        let fake_pdf_dir = dir.join("folder.pdf");
        std::fs::create_dir_all(&fake_pdf_dir)?;

        let mut app = PdfViewerApp::new(&engine, None)?;
        app.start_single_instance_listener(Some(lock.clone()))?;
        let (port, token) = lock_port_and_token(&lock);
        let send = |line: String| send_ipc_line(port, &line);

        send(format!("wrong{}|{}\n", &token[5..], pdf.display()));
        send(format!("{token}|{}\n", txt.display()));
        send(format!("{token}|{}\n", dir.join("missing.pdf").display()));
        send(format!("{token}|{}\n", fake_pdf_dir.display()));
        send(format!("{token}|\n"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(app.poll_single_instance_listener(), 0);
        assert!(app.doc.is_none());

        send(format!("{token}|{}\n", pdf.display()));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(app.poll_single_instance_listener(), 1, "{:?}", app.status_notice);
        assert_eq!(app.total_pages(), 2);

        app.stop_single_instance_listener();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Phase 4: one local client that connects and sends nothing must not make any single poll
    /// take more than a few milliseconds (the old blocking read waited 50 ms per connection),
    /// and it is dropped after one frame.
    #[test]
    fn test_single_instance_silent_client_does_not_stall_a_poll() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_si_silent");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let lock = dir.join("i.lock");
        let mut app = PdfViewerApp::new(&engine, None)?;
        app.start_single_instance_listener(Some(lock.clone()))?;
        let (port, _) = lock_port_and_token(&lock);

        let silent = std::net::TcpStream::connect(("127.0.0.1", port))?;
        std::thread::sleep(std::time::Duration::from_millis(10)); // surely in the accept queue

        let t0 = std::time::Instant::now();
        assert_eq!(app.poll_single_instance_listener(), 0);
        let first = t0.elapsed();
        assert_eq!(app.pending_ipc.len(), 1, "accepted and waiting, not read with a block");
        assert!(first < std::time::Duration::from_millis(5), "poll took {first:?}");

        std::thread::sleep(state::IPC_FRAME + std::time::Duration::from_millis(2));
        let t1 = std::time::Instant::now();
        assert_eq!(app.poll_single_instance_listener(), 0);
        let second = t1.elapsed();
        assert!(second < std::time::Duration::from_millis(5), "poll took {second:?}");
        assert!(app.pending_ipc.is_empty(), "silent for a frame: dropped");

        drop(silent);
        app.stop_single_instance_listener();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Silent, slow and flooding local connections never block a poll, are dropped after one
    /// frame (or when too long), and a normal forward still works afterwards.
    #[test]
    fn test_single_instance_slow_or_silent_clients_never_block() -> Result<()> {
        use std::io::Write;
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_si_slow");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let lock = dir.join("i.lock");
        let pdf = dir.join("doc.pdf");
        engine.create_sample_pdf(&pdf, 3)?;

        let mut app = PdfViewerApp::new(&engine, None)?;
        app.start_single_instance_listener(Some(lock.clone()))?;
        let (port, token) = lock_port_and_token(&lock);
        let connect = || std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();

        // Silent connections, more than the pending cap.
        let silent: Vec<_> = (0..20).map(|_| connect()).collect();
        // Half a request line, never finished.
        let mut slow = connect();
        slow.write_all(token.as_bytes())?;
        // 1 MB without a newline, written from another thread (it may block on a full socket).
        let flood = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = s.write_all(&vec![b'x'; 1 << 20]);
        });
        std::thread::sleep(std::time::Duration::from_millis(30));

        let mut slowest = std::time::Duration::ZERO;
        for _ in 0..3 {
            let t0 = std::time::Instant::now();
            assert_eq!(app.poll_single_instance_listener(), 0);
            slowest = slowest.max(t0.elapsed());
            assert!(app.pending_ipc.len() <= state::IPC_MAX_PENDING);
            assert!(app.pending_ipc.iter().all(|c| c.buf.len() <= state::IPC_MAX_LINE_BYTES + 1024));
            std::thread::sleep(state::IPC_FRAME + std::time::Duration::from_millis(5));
        }
        assert!(slowest < std::time::Duration::from_millis(15), "a poll took {slowest:?}");
        assert!(app.pending_ipc.is_empty(), "stalled connections must be dropped after one frame");
        let _ = flood.join();
        drop(silent);
        drop(slow);

        // A normal forward still works.
        assert!(try_forward_to_running_instance(&pdf, Some(&lock)));
        let mut opened = 0;
        for _ in 0..3 {
            opened += app.poll_single_instance_listener();
            if opened > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(opened, 1);
        assert_eq!(app.total_pages(), 3);

        app.stop_single_instance_listener();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// One A4 page with a filled-in text field: the widget's appearance stream is a solid black
    /// box at (100,600)-(400,650) in PDF points, and the page content has a red box at
    /// (100,300)-(400,350). The catalog has an /AcroForm, as real forms do, so PDFium sets up
    /// its form environment for it.
    fn build_filled_form_pdf() -> Result<Vec<u8>> {
        use lopdf::dictionary;
        let mut ldoc = lopdf::Document::with_version("1.5");
        let pages_id = ldoc.new_object_id();
        let page_id = ldoc.new_object_id();
        let ap = ldoc.add_object(lopdf::Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 300.into(), 50.into()],
            },
            b"0 g 0 0 300 50 re f".to_vec(),
        ));
        let widget = ldoc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
            "T" => lopdf::Object::string_literal("name"),
            "V" => lopdf::Object::string_literal("filled"),
            "F" => 4, "P" => page_id,
            "Rect" => vec![100.into(), 600.into(), 400.into(), 650.into()],
            "AP" => dictionary! { "N" => ap },
        });
        let content = ldoc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"1 0 0 rg 100 300 300 50 re f".to_vec(),
        ));
        ldoc.objects.insert(
            page_id,
            lopdf::Object::Dictionary(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Contents" => content, "Annots" => vec![widget.into()],
            }),
        );
        ldoc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
            }),
        );
        let acroform = ldoc.add_object(dictionary! { "Fields" => vec![widget.into()] });
        let catalog = ldoc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id, "AcroForm" => acroform,
        });
        ldoc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        ldoc.save_to(&mut out)?;
        Ok(out)
    }

    /// Pixel at PDF point (`x_pt`, `y_pt`) of an A4 page rendered `w` x `h` into `pixels`.
    fn pixel_at_pt(pixels: &[u32], w: u32, h: u32, x_pt: f32, y_pt: f32) -> (u8, u8, u8) {
        let x = ((x_pt / 595.0) * w as f32) as usize;
        let y = (((842.0 - y_pt) / 842.0) * h as f32) as usize;
        let px = pixels[y * w as usize + x];
        ((px >> 16) as u8, (px >> 8) as u8, px as u8)
    }

    fn is_black(c: (u8, u8, u8)) -> bool {
        c.0 < 40 && c.1 < 40 && c.2 < 40
    }

    fn is_red(c: (u8, u8, u8)) -> bool {
        c.0 > 200 && c.1 < 60 && c.2 < 60
    }

    /// A filled-in form field must show in every view: page view, continuous scroll, thumbnail
    /// grid, and zoomed in (400%), where pages are drawn as 512x512 tiles. The open document is
    /// never flattened: its widget stays a live form field.
    #[test]
    fn test_filled_form_field_visible_in_all_views_including_400_percent_tiles() -> Result<()> {
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_form_views");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("form.pdf");
        std::fs::write(&path, build_filled_form_pdf()?)?;
        let (field, content) = ((250.0, 625.0), (250.0, 325.0));

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        let (vw, vh) = (1024, 1080);

        // 1. Page view (fit page).
        app.ensure_current_page_rendered(vw, vh, 0, 52, 22)?;
        let c = app.cached_page.as_ref().unwrap();
        assert!(!c.pixels.is_empty());
        assert!(is_red(pixel_at_pt(&c.pixels, c.bitmap_width, c.bitmap_height, content.0, content.1)));
        assert!(
            is_black(pixel_at_pt(&c.pixels, c.bitmap_width, c.bitmap_height, field.0, field.1)),
            "form field missing in page view"
        );

        // 2. Thumbnail.
        app.ensure_thumbnail_rendered(0)?;
        let t = &app.thumbnail_cache[&0];
        assert!(
            is_black(pixel_at_pt(&t.pixels, t.width, t.height, field.0, field.1)),
            "form field missing in thumbnail"
        );

        // 3. Continuous scroll.
        app.view_mode = ViewMode::ContinuousScroll;
        app.ensure_current_page_rendered(vw, vh, 0, 52, 22)?;
        let cc = &app.continuous_page_cache[&0];
        assert!(
            is_black(pixel_at_pt(&cc.pixels, cc.bitmap_width, cc.bitmap_height, field.0, field.1)),
            "form field missing in continuous scroll"
        );

        // 4. 400% in page view: tiles only, with the field scrolled into view.
        app.view_mode = ViewMode::FitPage;
        app.invalidate_page_caches();
        app.set_zoom_around_cursor(4.0, 500.0, 500.0);
        app.ensure_current_page_rendered(vw, vh, 0, 52, 22)?;
        let (bw, bh) = {
            let c = app.cached_page.as_ref().unwrap();
            assert!(c.pixels.is_empty(), "400% must render tiles, not a full page");
            (c.bitmap_width, c.bitmap_height)
        };
        let to_virtual = |(x, y): (f32, f32)| {
            ((x / 595.0 * bw as f32) as u32, ((842.0 - y) / 842.0 * bh as f32) as u32)
        };
        let tile_pixel = |app: &PdfViewerApp, (vx, vy): (u32, u32)| {
            let key = state::TileKey {
                page_index: 0,
                virtual_w: bw,
                virtual_h: bh,
                rotation_deg: 0,
                tile_col: (vx / state::TILE_SIZE_PX) as u16,
                tile_row: (vy / state::TILE_SIZE_PX) as u16,
            };
            let tile = app.tile_cache.get(&key).expect("tile under the point must be rendered");
            let (tx, ty) = (vx % state::TILE_SIZE_PX, vy % state::TILE_SIZE_PX);
            let px = tile.pixels[(ty * tile.width + tx) as usize];
            ((px >> 16) as u8, (px >> 8) as u8, px as u8)
        };
        for (point, want_black) in [(field, true), (content, false)] {
            let (vx, vy) = to_virtual(point);
            app.pan_x = vx.saturating_sub(300) as f32;
            app.scroll_y = vy.saturating_sub(300) as f32;
            app.ensure_current_page_rendered(vw, vh, 0, 52, 22)?;
            let got = tile_pixel(&app, (vx, vy));
            if want_black {
                assert!(is_black(got), "form field missing at 400% zoom (tile pixel {got:?})");
            } else {
                assert!(is_red(got), "page content wrong at 400% zoom (tile pixel {got:?})");
            }
        }

        // 5. The document itself is not flattened: the widget is still a live form field.
        let doc = app.doc.as_ref().unwrap();
        let page = doc.pages().get(0)?;
        assert_eq!(page.annotations().len(), 1);
        assert_eq!(
            page.annotations().iter().next().map(|a| a.annotation_type()),
            Some(PdfPageAnnotationType::Widget)
        );
        drop(page);

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Windows printing state machine through the app, with a fake printer: one page per
    /// event-loop step, "Printing page N / M" in the status bar, Esc cancels with AbortDoc,
    /// and a second Ctrl/Cmd+P while printing is refused.
    #[test]
    fn test_print_job_steps_one_page_per_frame_and_esc_cancels() -> Result<()> {
        use crate::print::test_support::{FakeSink, PrintLog};
        use winit::keyboard::{Key, NamedKey};
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_app_print_job");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        std::fs::write(&path, build_labeled_padded_pdf(3, "Alpha")?)?;
        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        let notice = |app: &PdfViewerApp| app.status_notice.clone().unwrap_or_default();

        // Run 1: cancelled with Esc after the first page.
        let log: PrintLog = Default::default();
        let job = crate::print::PagedPrintJob::new(
            &engine, app.doc.as_ref().unwrap(), &[0, 1, 2], 1, 1, false, &dir, FakeSink::new(&log),
        )?;
        app.start_print_job(job, "doc.pdf".into());
        assert!(notice(&app).starts_with("Printing page 1 / 3"), "{}", notice(&app));
        assert!(app.step_print_job());
        assert!(notice(&app).starts_with("Printing page 2 / 3"), "{}", notice(&app));
        assert_eq!(log.borrow().len(), 2, "one page sent per step");

        app.trigger_print();
        assert!(notice(&app).contains("still running"), "{}", notice(&app));
        assert!(app.print_job.is_some(), "the running job is untouched");

        app.handle_key_input(Key::Named(NamedKey::Escape), None);
        assert!(app.print_job.is_none());
        assert_eq!(notice(&app), "Printing cancelled after 1 of 3 page(s)");
        assert_eq!(*log.borrow(), ["start portrait", "end ink", "abort"]);

        // Run 2: to completion, one step per frame.
        let log: PrintLog = Default::default();
        let job = crate::print::PagedPrintJob::new(
            &engine, app.doc.as_ref().unwrap(), &[0, 1, 2], 2, 2, false, &dir, FakeSink::new(&log),
        )?;
        app.start_print_job(job, "doc.pdf".into());
        let mut frames = 1;
        while app.step_print_job() {
            frames += 1;
            let (sent, total) = app.print_job.as_ref().unwrap().job.progress();
            assert_eq!(notice(&app), format!("Printing page {} / {total} (Esc to cancel)", sent + 1));
        }
        assert_eq!(frames, 6, "3 pages x 2 copies, one per frame");
        assert!(app.print_job.is_none());
        assert_eq!(notice(&app), "Printed 3 page(s) x 2 copy/copies of 'doc.pdf'");
        assert_eq!(log.borrow().last().map(String::as_str), Some("finish"));

        // Run 3: a printer error ends the job with the error in the status bar.
        let log: PrintLog = Default::default();
        let job = crate::print::PagedPrintJob::new(
            &engine, app.doc.as_ref().unwrap(), &[0, 1], 1, 1, false, &dir,
            FakeSink::failing_on_page(&log, 1),
        )?;
        app.start_print_job(job, "doc.pdf".into());
        assert!(!app.step_print_job());
        assert!(app.print_job.is_none());
        assert!(notice(&app).contains("paper jam") && notice(&app).contains("aborted"), "{}", notice(&app));
        assert_eq!(log.borrow().last().map(String::as_str), Some("abort"));

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// The Windows print snapshot is deleted however the job ends: finished, Esc (AbortDoc),
    /// printer error, or the app quitting mid-print (the printer document is aborted too).
    #[test]
    fn test_print_snapshot_deleted_however_the_job_ends() -> Result<()> {
        use crate::print::test_support::{FakeSink, PrintLog};
        use winit::keyboard::{Key, NamedKey};
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_app_print_snapshot");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        std::fs::write(&path, build_labeled_padded_pdf(3, "Alpha")?)?;
        let snap_dir = dir.join("tmp");
        std::fs::create_dir_all(&snap_dir)?;
        let snapshots = || std::fs::read_dir(&snap_dir).unwrap().count();

        fn start<'e>(
            engine: &'e PdfEngine,
            app: &mut PdfViewerApp<'e>,
            snap_dir: &Path,
            sink: Box<dyn crate::print::PrintSink>,
        ) -> Result<std::path::PathBuf> {
            let job = crate::print::PagedPrintJob::new(
                engine, app.doc.as_ref().unwrap(), &[0, 1, 2], 1, 1, false, snap_dir, sink,
            )?;
            let temp = job.temp_path().to_path_buf();
            app.start_print_job(job, "doc.pdf".into());
            assert!(temp.exists());
            Ok(temp)
        }

        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;

        // Finished.
        let log: PrintLog = Default::default();
        let temp = start(&engine, &mut app, &snap_dir, FakeSink::new(&log))?;
        while app.step_print_job() {}
        assert!(!temp.exists(), "finished job");
        // Esc.
        let log: PrintLog = Default::default();
        let temp = start(&engine, &mut app, &snap_dir, FakeSink::new(&log))?;
        app.step_print_job();
        app.handle_key_input(Key::Named(NamedKey::Escape), None);
        assert!(!temp.exists(), "cancelled job");
        // Printer error.
        let log: PrintLog = Default::default();
        let temp = start(&engine, &mut app, &snap_dir, FakeSink::failing_on_page(&log, 2))?;
        while app.step_print_job() {}
        assert!(!temp.exists(), "failed job");
        // App quits mid-print.
        let log: PrintLog = Default::default();
        let temp = start(&engine, &mut app, &snap_dir, FakeSink::new(&log))?;
        app.step_print_job();
        drop(app);
        assert!(!temp.exists(), "app quit mid-print");
        assert_eq!(log.borrow().last().map(String::as_str), Some("abort"));
        assert_eq!(snapshots(), 0);

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Print temp files older than an hour (left by a run killed mid-print) are removed when
    /// the app starts; recent ones are left alone.
    #[test]
    fn test_stale_print_files_swept_on_start() -> Result<()> {
        let engine = PdfEngine::new()?;
        let tmp = std::env::temp_dir();
        let tag = format!("{}_sweeptest", std::process::id());
        let stale = tmp.join(format!("micropdf_print_{tag}_old.pdf"));
        let fresh = tmp.join(format!("micropdf_print_{tag}_new.pdf"));
        std::fs::write(&stale, b"%PDF-1.4")?;
        std::fs::write(&fresh, b"%PDF-1.4")?;
        std::fs::File::options()
            .write(true)
            .open(&stale)?
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(7200))?;

        let app = PdfViewerApp::new(&engine, None)?;
        assert!(!stale.exists(), "stale print file must be swept on start");
        assert!(fresh.exists(), "a recent print file may belong to a running print");
        drop(app);
        let _ = std::fs::remove_file(&fresh);
        Ok(())
    }

    /// macOS: Enter in the print bar starts `lp` without waiting; each frame polls it; when it
    /// exits the temp file is gone and the result is in the status bar.
    #[cfg(unix)]
    #[test]
    fn test_lp_print_from_prompt_runs_in_background() -> Result<()> {
        use winit::keyboard::{Key, NamedKey};
        let engine = PdfEngine::new()?;
        let dir = std::env::temp_dir().join("micropdf_app_lp");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("doc.pdf");
        std::fs::write(&path, build_labeled_padded_pdf(3, "Alpha")?)?;
        let mut app = PdfViewerApp::new(&engine, Some(path.clone()))?;
        let notice = |app: &PdfViewerApp| app.status_notice.clone().unwrap_or_default();

        for (exit_code, expect) in [
            (0, "Sent 2 page(s) x 2 copy/copies to printer 'Fake' (request id is Fake-42 (1 file(s)))"),
            (1, "Print error: lp failed: lp: The printer or class does not exist."),
        ] {
            app.lp_program = crate::print::test_support::fake_lp(&dir, &dir, exit_code, 0.3).into();
            app.input_mode = state::InputMode::PrintPrompt {
                printers: vec!["Fake".into()],
                selected_printer: 0,
                copies: 2,
                range_input: "1-2".into(),
                fit_to_page: true,
            };
            let t0 = std::time::Instant::now();
            app.handle_key_input(Key::Named(NamedKey::Enter), None);
            assert!(t0.elapsed() < std::time::Duration::from_millis(250), "Enter must not wait for lp");
            assert_eq!(notice(&app), "Sending to printer 'Fake'...");
            let temp = app.lp_job.as_ref().unwrap().job.temp_path().unwrap().to_path_buf();
            assert!(temp.exists());

            app.trigger_print();
            assert!(notice(&app).contains("still running"));

            let mut frames = 0;
            while app.poll_lp_job() {
                frames += 1;
                assert!(frames < 1000, "lp never finished");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(frames > 0, "lp was polled across frames, not waited for");
            assert!(app.lp_job.is_none());
            assert!(!temp.exists(), "temp PDF deleted when lp exited");
            assert_eq!(notice(&app), expect);
        }

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// `n`-page text PDF built with lopdf.
    fn build_text_pdf(n: u32) -> Result<Vec<u8>> {
        use lopdf::dictionary;
        let mut ldoc = lopdf::Document::with_version("1.5");
        let pages_id = ldoc.new_object_id();
        let font_id = ldoc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let resources_id = ldoc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let mut kids = Vec::new();
        for i in 1..=n {
            let content = format!("BT /F1 14 Tf 72 720 Td (Text page {i}) Tj ET");
            let content_id = ldoc.add_object(lopdf::Stream::new(dictionary! {}, content.into_bytes()));
            kids.push(
                ldoc.add_object(dictionary! {
                    "Type" => "Page", "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                    "Resources" => resources_id, "Contents" => content_id,
                })
                .into(),
            );
        }
        ldoc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }),
        );
        let catalog_id = ldoc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        ldoc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        ldoc.save_to(&mut out)?;
        Ok(out)
    }

    /// `n`-page scan PDF: each page a distinct 300-dpi A4 JPEG (about 1.1 MB), like a real
    /// office scanner produces. Built with lopdf so PDFium allocations don't pollute the
    /// measurement that follows.
    fn build_scan_pdf(n: u32) -> Result<Vec<u8>> {
        use lopdf::dictionary;
        let (w, h) = (2480u32, 3508u32);
        let mut ldoc = lopdf::Document::with_version("1.5");
        let pages_id = ldoc.new_object_id();
        let mut kids = Vec::new();
        for p in 0..n {
            let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([245, 243, 238]));
            let mut seed: u32 = 12345 + p;
            for row in (300..h - 300).step_by(60) {
                let mut x = 250;
                while x < w - 250 {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    let word = 40 + (seed % 160);
                    for yy in row..row + 34 {
                        for xx in x..(x + word).min(w - 250) {
                            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                            if seed % 2 == 0 {
                                img.put_pixel(xx, yy, image::Rgb([25 + (seed % 40) as u8, 25, 30]));
                            }
                        }
                    }
                    x += word + 30;
                }
            }
            let mut jpg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 60).encode_image(&img)?;
            let img_id = ldoc.add_object(
                lopdf::Stream::new(
                    dictionary! {
                        "Type" => "XObject", "Subtype" => "Image",
                        "Width" => w as i64, "Height" => h as i64,
                        "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode",
                    },
                    jpg,
                )
                .with_compression(false),
            );
            let content_id = ldoc.add_object(lopdf::Stream::new(
                dictionary! {},
                b"q 595 0 0 842 0 0 cm /Im0 Do Q".to_vec(),
            ));
            kids.push(
                ldoc.add_object(dictionary! {
                    "Type" => "Page", "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                    "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => img_id } },
                    "Contents" => content_id,
                })
                .into(),
            );
        }
        ldoc.objects.insert(
            pages_id,
            lopdf::Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }),
        );
        let catalog_id = ldoc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        ldoc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        ldoc.save_to(&mut out)?;
        Ok(out)
    }

    /// Raw process RSS while printing a 20-page, 300-dpi scan at 600 dpi:
    /// - Windows path: every page rendered in 256-row strips (the GDI calls are replaced by a
    ///   no-op sink, so this measures MicroPDF + PDFium, not the printer driver);
    /// - macOS path: the in-memory document written to the temporary PDF handed to `lp`.
    /// Run: cargo test --release ram_print -- --ignored --nocapture --test-threads=1
    #[test]
    #[ignore]
    fn ram_print_600dpi_scan_measurement() -> Result<()> {
        let rss = || get_current_rss_mb().unwrap_or(0.0);
        let dir = std::env::temp_dir().join("micropdf_ram_print");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("scan20.pdf");
        std::fs::write(&path, build_scan_pdf(20)?)?;
        let file_mb = std::fs::metadata(&path)?.len() as f64 / 1_048_576.0;

        let engine = PdfEngine::new()?;
        let doc = engine.load_pdf_safe(&path)?;
        let opened = rss();

        let mut strip_buf = Vec::new();
        let mut peak_strips = opened;
        let start = std::time::Instant::now();
        for page_idx in 0..doc.pages().len() {
            // A4 at 600 dpi with ~4 mm margins.
            crate::print::render_page_in_horizontal_strips(
                &engine,
                &doc,
                page_idx,
                600,
                600,
                4800,
                6814,
                true,
                crate::print::PRINT_STRIP_ROWS,
                &mut strip_buf,
                |_, _, _, _, px| {
                    std::hint::black_box(px);
                    peak_strips = peak_strips.max(rss());
                    Ok(())
                },
            )?;
        }
        let strips_secs = start.elapsed().as_secs_f64();
        let after_strips = rss();

        let all: Vec<u16> = (0..doc.pages().len()).collect();
        let before_temp = rss();
        let temp = crate::print::write_pages_to_temp_pdf(&engine, &doc, &all, false, &dir)?;
        let after_temp = rss();
        let temp_mb = std::fs::metadata(temp.path())?.len() as f64 / 1_048_576.0;
        drop(temp);

        eprintln!();
        eprintln!("| Raw RSS, printing a 20-page 300-dpi scan ({file_mb:.1} MB) | Value |");
        eprintln!("|---|---|");
        eprintln!("| After open | {opened:.1} MB |");
        eprintln!("| Windows path: peak while rendering 20 pages at 600 dpi in strips | {peak_strips:.1} MB |");
        eprintln!("| Windows path: after the last page | {after_strips:.1} MB |");
        eprintln!("| Windows path: render time (no printer) | {strips_secs:.1} s |");
        eprintln!("| macOS path: before writing the temp PDF | {before_temp:.1} MB |");
        eprintln!("| macOS path: after writing the temp PDF ({temp_mb:.1} MB) | {after_temp:.1} MB |");

        drop(doc);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// Helper for `ram_realistic_measurement` (runs in a child process).
    #[test]
    #[ignore]
    fn ram_build_scan_fixture() -> Result<()> {
        let scan = std::env::temp_dir().join("micropdf_ram_scan.pdf");
        PdfEngine::write_atomic(&scan, &build_scan_pdf(100)?)
    }

    /// Real RAM measurement: raw, unadjusted process RSS at 1920x1080 (headless, so the window
    /// frame buffer, about 8 MB at 1080p, is not included). Run it on its own:
    ///   cargo test --release ram_realistic -- --ignored --nocapture --test-threads=1
    /// Targets: small text PDF idle <= 25 MB; 100-page 300-dpi scan <= file size + 120 MB in
    /// every scenario. Known in v1.2.1: zoom and grid on the scan exceed the target.
    #[test]
    #[ignore]
    fn ram_realistic_measurement() -> Result<()> {
        let engine = PdfEngine::new()?;
        let (vw, vh, sw, th, sb) = (1920u32, 1080u32, 0u32, 52u32, 22u32);
        let rss = || get_current_rss_mb().unwrap_or(0.0);
        let dir = std::env::temp_dir();

        let small = dir.join("micropdf_ram_small.pdf");
        PdfEngine::write_atomic(&small, &build_text_pdf(15)?)?;
        let before_small = rss();
        let small_idle = {
            let mut app = PdfViewerApp::new(&engine, Some(small.clone()))?;
            app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            rss()
        };

        // Build the 116 MB fixture in a child process; building it here would leave about
        // 100 MB of freed-but-resident heap in this process and distort every scan figure.
        let scan = dir.join("micropdf_ram_scan.pdf");
        let status = std::process::Command::new(std::env::current_exe()?)
            .args(["--ignored", "--exact", "viewer::tests::ram_build_scan_fixture", "--test-threads=1"])
            .status()?;
        assert!(status.success() && scan.exists(), "fixture build failed");
        let file_mb = std::fs::metadata(&scan)?.len() as f64 / 1048576.0;
        let before_scan = rss();
        let mut app = PdfViewerApp::new(&engine, Some(scan.clone()))?;
        app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
        let idle = rss();
        app.set_view_mode(ViewMode::ContinuousScroll);
        let mut scroll = 0.0f64;
        for p in 0..50u16 {
            app.current_page_index = p;
            app.scroll_y = 120.0;
            app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            scroll = scroll.max(rss());
        }
        app.set_view_mode(ViewMode::FitPage);
        app.set_zoom_around_cursor(4.0, 960.0, 540.0);
        let mut zoom = 0.0f64;
        for k in 0..12 {
            app.pan_x = k as f32 * 400.0;
            app.scroll_y = k as f32 * 350.0;
            app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            zoom = zoom.max(rss());
        }
        app.reset_zoom();
        app.set_view_mode(ViewMode::ThumbnailGrid);
        let mut grid = 0.0f64;
        let mut gs = 0.0f32;
        while gs < 30_000.0 {
            app.grid_scroll_y = gs;
            app.update_visible_thumbnails(vw, vh, sw, th, sb);
            grid = grid.max(rss());
            gs += 600.0;
        }
        drop(app);
        let _ = std::fs::remove_file(&small);
        let _ = std::fs::remove_file(&scan);

        eprintln!("\n| Raw RSS, 1920x1080, headless | Value |");
        eprintln!("|---|---|");
        eprintln!("| Small text PDF: process before open | {before_small:.1} MB |");
        eprintln!("| Small text PDF: idle after open | {small_idle:.1} MB |");
        eprintln!("| Scan ({file_mb:.1} MB, 100 pages): process before open | {before_scan:.1} MB |");
        eprintln!("| Scan: idle after open | {idle:.1} MB |");
        eprintln!("| Scan: peak while scrolling 50 pages | {scroll:.1} MB |");
        eprintln!("| Scan: peak at 400% zoom, panning | {zoom:.1} MB |");
        eprintln!("| Scan: peak in thumbnail grid | {grid:.1} MB |");

        let cap = file_mb + 120.0;
        assert!(small_idle <= 25.0, "small text idle {small_idle:.1} MB > 25 MB");
        assert!(idle <= cap, "scan idle {idle:.1} > {cap:.1}");
        assert!(scroll <= cap, "scan scroll {scroll:.1} > {cap:.1}");
        assert!(zoom <= cap, "scan zoom {zoom:.1} > {cap:.1}");
        assert!(grid <= cap, "scan grid {grid:.1} > {cap:.1}");
        Ok(())
    }

    #[test]
    fn test_ui_theme_default_is_light_and_ini_roundtrip() {
        use super::theme::UiThemeMode;
        assert_eq!(UiThemeMode::default(), UiThemeMode::Light);
        assert_eq!(UiThemeMode::Light.toggle(), UiThemeMode::Dark);
        assert_eq!(UiThemeMode::Dark.toggle(), UiThemeMode::Light);
        assert_eq!(UiThemeMode::from_ini_str("light"), UiThemeMode::Light);
        assert_eq!(UiThemeMode::from_ini_str("dark"), UiThemeMode::Dark);
        assert_eq!(UiThemeMode::from_ini_str("unknown"), UiThemeMode::Light);
        assert_eq!(UiThemeMode::Light.to_ini_str(), "light");
        assert_eq!(UiThemeMode::Dark.to_ini_str(), "dark");
        let light = UiThemeMode::Light.theme();
        let dark = UiThemeMode::Dark.theme();
        assert_ne!(light.canvas_bg, dark.canvas_bg);
        assert_ne!(light.topbar_bg, dark.topbar_bg);
    }

    #[test]
    fn test_toolbar_config_defaults_toggle_and_ini_roundtrip() {
        use super::toolbar::{ToolbarConfig, ToolbarItem};
        let mut cfg = ToolbarConfig::default();
        // Sidebar & Grid moved to Bottom Bar by default (still toggleable on top toolbar if desired)
        assert!(!cfg.is_enabled(ToolbarItem::Sidebar));
        assert!(!cfg.is_enabled(ToolbarItem::Grid));
        assert!(cfg.is_enabled(ToolbarItem::Open));
        assert!(cfg.is_enabled(ToolbarItem::SaveAs));
        assert!(cfg.is_enabled(ToolbarItem::Print));
        assert!(cfg.is_enabled(ToolbarItem::Annotations));
        assert!(cfg.is_enabled(ToolbarItem::UiScale));
        assert!(cfg.is_enabled(ToolbarItem::ThemeToggle));

        cfg.toggle(ToolbarItem::Annotations);
        cfg.toggle(ToolbarItem::Print);
        assert!(!cfg.is_enabled(ToolbarItem::Annotations));
        assert!(!cfg.is_enabled(ToolbarItem::Print));

        let serialized = cfg.to_ini_str();
        let parsed = ToolbarConfig::from_ini_str(&serialized);
        assert_eq!(cfg, parsed);

        let none_cfg = ToolbarConfig::from_ini_str("none");
        for item in ToolbarItem::ALL {
            assert!(!none_cfg.is_enabled(item));
        }
        assert_eq!(none_cfg.to_ini_str(), "none");
    }

    #[test]
    fn test_session_config_persists_ui_preferences_and_loads_legacy_ini() {
        use super::state::{PdfViewerApp, SessionConfig};
        use super::theme::UiThemeMode;
        use super::toolbar::{ToolbarConfig, ToolbarItem};

        // Legacy v1.2 ini without ui_theme / show_toolbar / toolbar_items / ui_scale should default cleanly
        let legacy = "recent=foo.pdf\ntab=bar.pdf|2\nactive_tab=0\n";
        let loaded_legacy = PdfViewerApp::parse_session_ini(legacy);
        assert_eq!(loaded_legacy.ui_theme, UiThemeMode::Light);
        assert!(loaded_legacy.show_toolbar);
        assert!((loaded_legacy.ui_scale - 1.0).abs() < 0.001);
        assert_eq!(loaded_legacy.toolbar_config, ToolbarConfig::default());

        // Roundtrip with custom UI preferences including UI scale
        let mut cfg = SessionConfig::default();
        cfg.ui_theme = UiThemeMode::Dark;
        cfg.show_toolbar = false;
        cfg.ui_scale = 1.25;
        cfg.toolbar_config.toggle(ToolbarItem::Open);
        cfg.toolbar_config.toggle(ToolbarItem::EditTools);
        let serialized = PdfViewerApp::serialize_session_ini(&cfg);
        let roundtripped = PdfViewerApp::parse_session_ini(&serialized);
        assert_eq!(roundtripped.ui_theme, UiThemeMode::Dark);
        assert!(!roundtripped.show_toolbar);
        assert!((roundtripped.ui_scale - 1.25).abs() < 0.001);
        assert_eq!(roundtripped.toolbar_config, cfg.toolbar_config);
    }

    #[test]
    fn test_ui_draw_fonts_metrics_truncation_and_cache_cap() {
        use super::ui_draw::*;
        let w = measure_text("MicroPDF Print Preview", 13.0, false);
        assert!(w > 40.0, "expected positive measured text width, got {w}");
        let wb = measure_text("MicroPDF Print Preview", 13.0, true);
        assert!(wb > 40.0, "expected positive bold text width, got {wb}");

        let trunc = truncate_ellipsis("VeryLongDocumentFileNameThatShouldTruncate.pdf", 80.0, 12.0, false);
        assert!(trunc.ends_with('…') || trunc.ends_with("..."), "expected ellipsis truncation, got '{trunc}'");
        assert!(measure_text(&trunc, 12.0, false) <= 82.0);

        // Render text and icons into a scratch buffer and verify glyph cache stays bounded
        let mut buf = vec![0x00FF_FFFFu32; 320 * 120];
        draw_text(&mut buf, 320, 120, 10, 10, "Bright Professional UI 1234567890", 14.0, 0x001E_293B, false);
        draw_text(&mut buf, 320, 120, 10, 34, "Bold Heading — Print & Zoom", 15.0, 0x0025_63EB, true);
        draw_icon(&mut buf, 320, 120, UiIcon::Print, 10, 64, 18, 0x001E_293B);
        draw_icon(&mut buf, 320, 120, UiIcon::Sun, 36, 64, 18, 0x001E_293B);
        draw_icon(&mut buf, 320, 120, UiIcon::Moon, 62, 64, 18, 0x001E_293B);
        assert!(buf.iter().any(|&p| p != 0x00FF_FFFF), "expected pixels to be drawn");

        // Stress-test many sizes to verify the glyph cache cap holds
        for sz in [10.0f32, 12.0, 14.0, 16.0, 18.0, 22.0, 28.0] {
            let _ = measure_text("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", sz, false);
        }
        assert!(
            glyph_cache_bytes() <= MAX_GLYPH_CACHE_BYTES,
            "glyph cache exceeded {} byte cap: {} bytes",
            MAX_GLYPH_CACHE_BYTES,
            glyph_cache_bytes()
        );
    }

    #[test]
    fn test_phase1_ui_skin_doc_kind_and_help_menu() {
        use super::state::{MenuCategory, PdfViewerApp, SessionConfig};
        use super::theme::{DocKind, UiSkin};
        use std::path::Path;

        // UiSkin defaults, toggle, labels, and INI round-trip
        assert_eq!(UiSkin::default(), UiSkin::Classic);
        assert_eq!(UiSkin::Classic.label(), "Classic");
        assert_eq!(UiSkin::Distinct.label(), "Distinct");
        assert_eq!(UiSkin::Classic.toggle(), UiSkin::Distinct);
        assert_eq!(UiSkin::Distinct.toggle(), UiSkin::Classic);
        assert_eq!(UiSkin::from_ini_str("distinct"), UiSkin::Distinct);
        assert_eq!(UiSkin::from_ini_str("DISTINCT"), UiSkin::Distinct);
        assert_eq!(UiSkin::from_ini_str("classic"), UiSkin::Classic);
        assert_eq!(UiSkin::from_ini_str("unknown"), UiSkin::Classic);

        let legacy_ini = "ui_theme=dark\nactive_tab=0\n";
        let parsed_legacy = PdfViewerApp::parse_session_ini(legacy_ini);
        assert_eq!(parsed_legacy.ui_skin, UiSkin::Classic);

        let mut cfg = SessionConfig::default();
        cfg.ui_skin = UiSkin::Distinct;
        let ini = PdfViewerApp::serialize_session_ini(&cfg);
        assert!(ini.contains("ui_skin=distinct"));
        let roundtripped = PdfViewerApp::parse_session_ini(&ini);
        assert_eq!(roundtripped.ui_skin, UiSkin::Distinct);

        // DocKind detection
        assert_eq!(DocKind::from_path(Path::new("report.pdf")), DocKind::Pdf);
        assert_eq!(DocKind::from_path(Path::new("REPORT.PDF")), DocKind::Pdf);
        assert_eq!(DocKind::from_path(Path::new("")), DocKind::Pdf);
        assert_eq!(DocKind::from_path(Path::new("notes.md")), DocKind::Markdown);
        assert_eq!(DocKind::from_path(Path::new("README.MARKDOWN")), DocKind::Markdown);
        assert_eq!(DocKind::from_path(Path::new("paper.tex")), DocKind::LaTeX);
        assert_eq!(DocKind::from_path(Path::new("novel.EPUB")), DocKind::Epub);
        assert_eq!(DocKind::from_path(Path::new("memo.docx")), DocKind::Word);
        assert_eq!(DocKind::from_path(Path::new("REPORT.DOCX")), DocKind::Word);
        assert_eq!(DocKind::from_path(Path::new("issue01.cbz")), DocKind::Comic);
        assert_eq!(DocKind::from_path(Path::new("MANGA.CBZ")), DocKind::Comic);
        assert!(!DocKind::Pdf.is_reflowable());
        assert!(DocKind::Markdown.is_reflowable());
        assert!(DocKind::LaTeX.is_reflowable());
        assert!(DocKind::Epub.is_reflowable());
        assert!(DocKind::Word.is_reflowable());
        assert!(!DocKind::Comic.is_reflowable());

        // Help menu contains UI Skin entry without breaking existing items
        let help_items = MenuCategory::Help.items();
        assert!(help_items.iter().any(|s| s.contains("User Guide")));
        assert!(help_items.iter().any(|s| s.contains("Keyboard & Mouse Shortcuts")));
        assert!(help_items.iter().any(|s| s.contains("Bookmark Import Format")));
        assert!(help_items.iter().any(|s| s.contains("UI Skin: Classic / Distinct")));
        assert!(help_items.iter().any(|s| s.contains("About MicroPDF")));
    }

    #[test]
    fn test_phase1_multifamily_typography_kerning_custom_os_and_doc_icons() {
        use super::theme::DocKind;
        use super::ui_draw::*;

        let serif_reg = TextStyle::book(24.0, false, false);
        let serif_bold = TextStyle::book(24.0, true, false);
        let serif_ital = TextStyle::book(24.0, false, true);
        let mono = TextStyle::mono(16.0);

        // Positive measurements across BookSerif & CodeMono
        let w_reg = measure_text_styled("Hello Typography", serif_reg);
        let w_bold = measure_text_styled("Hello Typography", serif_bold);
        let w_ital = measure_text_styled("Hello Typography", serif_ital);
        assert!(w_reg > 40.0 && w_bold > 40.0 && w_ital > 40.0);

        // CodeMono is strictly monospaced: 'i' and 'W' have equal advance width
        let w_mono_i = measure_text_styled("iiii", mono);
        let w_mono_w = measure_text_styled("WWWW", mono);
        assert!(
            (w_mono_i - w_mono_w).abs() < 0.05,
            "CodeMono must have uniform advance: iiii={w_mono_i}, WWWW={w_mono_w}"
        );

        // Real kerning pairs in BookSerif: "AV" and "To" tighten advance width vs unkerned sum
        let av_kerned = measure_text_styled("AV", serif_reg);
        let av_sum = measure_text_styled("A", serif_reg) + measure_text_styled("V", serif_reg);
        assert!(
            av_kerned < av_sum - 0.5,
            "Expected negative kerning on BookSerif 'AV': kerned={av_kerned}, sum={av_sum}"
        );

        let to_kerned = measure_text_styled("To", serif_reg);
        let to_sum = measure_text_styled("T", serif_reg) + measure_text_styled("o", serif_reg);
        assert!(
            to_kerned < to_sum - 0.3,
            "Expected negative kerning on BookSerif 'To': kerned={to_kerned}, sum={to_sum}"
        );

        // Render BookSerif (regular/bold/italic), CodeMono, and all 6 DocKind icons
        let mut buf = vec![0x00FF_FFFFu32; 420 * 180];
        draw_text_styled_clipped(&mut buf, 420, 180, 12, 10, 408, "AV Book Serif", serif_reg, 0x000F_172A);
        draw_text_styled_clipped(&mut buf, 420, 180, 12, 42, 408, "Bold Serif", serif_bold, 0x000F_172A);
        draw_text_styled_clipped(&mut buf, 420, 180, 12, 74, 408, "Italic Serif", serif_ital, 0x000F_172A);
        draw_text_styled_clipped(&mut buf, 420, 180, 12, 108, 408, "fn main() {}", mono, 0x001E_293B);

        for (i, (kind, icon)) in [
            (DocKind::Pdf, UiIcon::DocPdf),
            (DocKind::Epub, UiIcon::DocEpub),
            (DocKind::Markdown, UiIcon::DocMarkdown),
            (DocKind::LaTeX, UiIcon::DocLatex),
            (DocKind::Word, UiIcon::DocWord),
            (DocKind::Comic, UiIcon::DocComic),
        ]
        .into_iter()
        .enumerate()
        {
            draw_doc_type_icon(&mut buf, 420, 180, kind, 12 + (i as i32) * 20, 140, 16, false);
            draw_doc_type_icon(&mut buf, 420, 180, kind, 144 + (i as i32) * 20, 140, 16, true);
            draw_icon(&mut buf, 420, 180, icon, 276 + (i as i32) * 20, 140, 16, 0x001E_293B);
        }
        assert!(buf.iter().any(|&p| p != 0x00FF_FFFF), "expected pixels to be drawn");

        // Lazy CustomOs slot: clears cleanly and falls back safely when unloaded
        assert!(set_custom_os_font(None));
        assert_eq!(active_custom_os_font_name(), None);
        let fallback_w = measure_text_styled(
            "Fallback Serif",
            TextStyle {
                family: FontFamily::CustomOs,
                size_px: 18.0,
                bold: false,
                italic: false,
            },
        );
        assert!(fallback_w > 30.0);

        // If any OS font exists in catalog, verify CustomOs can load and unload it cleanly
        if let Some(entry) = crate::fonts::global_system_fonts()
            .iter()
            .find(|e| e.path.is_some())
        {
            assert!(set_custom_os_font(Some(&entry.display_name)));
            assert!(active_custom_os_font_name().is_some());
            let os_w = measure_text_styled(
                "Custom OS Font",
                TextStyle {
                    family: FontFamily::CustomOs,
                    size_px: 18.0,
                    bold: false,
                    italic: false,
                },
            );
            assert!(os_w > 20.0);
            assert!(set_custom_os_font(None));
            assert_eq!(active_custom_os_font_name(), None);
        }

        // Stress-test across families & sizes to ensure bounded 512 KB cache holds
        for fam in [FontFamily::UiSans, FontFamily::BookSerif, FontFamily::CodeMono] {
            for sz in [11.0f32, 14.0, 18.0, 24.0, 32.0] {
                let _ = measure_text_styled(
                    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                    TextStyle {
                        family: fam,
                        size_px: sz,
                        bold: false,
                        italic: fam == FontFamily::BookSerif,
                    },
                );
            }
        }
        assert!(
            glyph_cache_bytes() <= MAX_GLYPH_CACHE_BYTES,
            "glyph cache exceeded {} byte cap: {} bytes",
            MAX_GLYPH_CACHE_BYTES,
            glyph_cache_bytes()
        );
    }

    #[test]
    fn test_print_preview_state_page_options_and_stepping() {
        use super::print_preview::{PrintPageScope, PrintPreviewState};
        use crate::print::PageParity;

        let mut pp = PrintPreviewState::new(
            vec!["HP_LaserJet".to_string(), "Save_as_PDF".to_string()],
            0,
            10,
            3, // 0-based page 4
        );
        assert_eq!(pp.resolved_pages, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(pp.active_page_index(), Some(0));

        // Current page scope
        pp.page_scope = PrintPageScope::Current;
        pp.recompute_pages(10, 3);
        assert_eq!(pp.resolved_pages, vec![3]);
        assert_eq!(pp.active_page_index(), Some(3));

        // Custom range + Even + Reverse
        pp.page_scope = PrintPageScope::Custom;
        pp.custom_range = "1-6, 9-10".to_string();
        pp.parity = PageParity::Even;
        pp.reverse = true;
        pp.recompute_pages(10, 3);
        assert_eq!(pp.resolved_pages, vec![9, 5, 3, 1]);
        assert_eq!(pp.sheet_pos, 0);
        assert_eq!(pp.active_page_index(), Some(9));
        pp.sheet_pos = 2;
        assert_eq!(pp.active_page_index(), Some(3));

        // Invalid range sets error and clears resolved_pages
        pp.custom_range = "99".to_string();
        pp.recompute_pages(10, 3);
        assert!(pp.resolved_pages.is_empty());
        assert!(pp.range_error.is_some());
    }

    #[test]
    fn test_print_preview_collate_booklet_and_annotation_colors() {
        use super::print_preview::PrintPreviewState;
        use crate::engine::AnnotationColor;

        let mut pp = PrintPreviewState::new(vec!["Simplex_Printer".to_string()], 0, 5, 0);
        assert!(pp.render_opts.collate, "Collate should be checked by default");
        assert!(pp.render_opts.include_annotations, "Print with annotations should be default");
        assert!(!pp.render_opts.booklet);
        assert!(!pp.manual_duplex);

        pp.render_opts.booklet = true;
        pp.recompute_pages(5, 0);
        // 5 pages pad to 8 -> 2 physical sheets -> 4 printed sides (front/back pairs)
        assert_eq!(pp.sheet_count(), 4);
        let sheets = pp.booklet_sheets();
        assert_eq!(sheets.len(), 4);
        assert!(sheets[0].is_front);
        assert_eq!(sheets[0].left, None);
        assert_eq!(sheets[0].right, Some(0));
        assert!(!sheets[1].is_front);
        assert_eq!(sheets[1].left, Some(1));
        assert_eq!(sheets[1].right, None);

        // AnnotationColor cycles through all 7 colors cleanly
        let mut c = AnnotationColor::Yellow;
        for _ in 0..7 {
            assert!(!c.label().is_empty());
            c = c.next();
        }
        assert_eq!(c, AnnotationColor::Yellow);
    }

    #[test]
    fn test_file_menu_no_duplicate_optimize_and_tools_keeps_optimize() {
        use super::state::MenuCategory;
        assert!(
            !MenuCategory::File.items().iter().any(|s| s.contains("Optimize")),
            "File menu should not contain duplicate Optimize item"
        );
        assert!(
            MenuCategory::Tools.items().iter().any(|s| s.contains("Optimize")),
            "Tools menu should keep Optimize item"
        );
        assert!(
            !MenuCategory::Tools.items().iter().any(|s| s.contains("Clear Existing Margins")),
            "Clear Existing Header/Footer should live inside the Header & Footer dialog, not as a separate Tools menu item"
        );
    }

    #[test]
    fn test_header_footer_profile_and_scroll_mode_session_ini_roundtrip() {
        use super::state::{PdfViewerApp, ScrollMode, SessionConfig};
        use crate::engine::{HeaderFooterColor, HeaderFooterProfile};

        let mut p = HeaderFooterProfile::default();
        p.name = "Board Report | Q4".to_string();
        p.header_left = "Confidential".to_string();
        p.header_center = "Annual Review".to_string();
        p.header_right = "2026".to_string();
        p.footer_left = "MicroPDF".to_string();
        p.footer_center = "Page {page} of {total}".to_string();
        p.footer_right = "Draft".to_string();
        p.font_size = 11.0;
        p.margin_pt = 28.0;
        p.color = HeaderFooterColor::Navy;

        let encoded = p.to_ini_str();
        let decoded = HeaderFooterProfile::from_ini_str(&encoded).expect("decode profile");
        assert_eq!(decoded.header_left, p.header_left);
        assert_eq!(decoded.header_center, p.header_center);
        assert_eq!(decoded.footer_center, p.footer_center);
        assert_eq!(decoded.font_size, 11.0);
        assert_eq!(decoded.margin_pt, 28.0);
        assert_eq!(decoded.color, HeaderFooterColor::Navy);

        let mut cfg = SessionConfig::default();
        cfg.scroll_mode = ScrollMode::ContinuousHorizontal;
        cfg.hf_profiles.push(decoded.clone());
        let ini = PdfViewerApp::serialize_session_ini(&cfg);
        let parsed = PdfViewerApp::parse_session_ini(&ini);
        assert_eq!(parsed.scroll_mode, ScrollMode::ContinuousHorizontal);
        assert_eq!(parsed.hf_profiles.len(), 1);
        assert_eq!(parsed.hf_profiles[0].footer_center, "Page {page} of {total}");
    }

    #[test]
    fn test_page_number_config_formatting_and_roman_cover_skip() {
        use crate::engine::{MarginPosition, PageNumberConfig, PageNumberFormat, PdfEngine};

        // Full non-abbreviated "Bottom Left / Center / Right" labels (#2c)
        assert_eq!(MarginPosition::FooterLeft.label(), "Bottom Left");
        assert_eq!(MarginPosition::FooterCenter.label(), "Bottom Center");
        assert_eq!(MarginPosition::FooterRight.label(), "Bottom Right");

        // Multi-page front skip (2 unnumbered cover/picture pages) + 3 Roman pages + Arabic body (#2d)
        let mut cfg = PageNumberConfig::default();
        cfg.skip_pages = 2;
        cfg.roman_end = 3;
        cfg.format = PageNumberFormat::PageNOfTotal;

        assert_eq!(PdfEngine::format_page_number_label(&cfg, 1, 10), None);
        assert_eq!(PdfEngine::format_page_number_label(&cfg, 2, 10), None);
        assert_eq!(
            PdfEngine::format_page_number_label(&cfg, 3, 10),
            Some("Page i".to_string())
        );
        assert_eq!(
            PdfEngine::format_page_number_label(&cfg, 5, 10),
            Some("Page iii".to_string())
        );
        assert_eq!(
            PdfEngine::format_page_number_label(&cfg, 6, 10),
            Some("Page 1 of 5".to_string())
        );
        assert_eq!(
            PdfEngine::format_page_number_label(&cfg, 10, 10),
            Some("Page 5 of 5".to_string())
        );

        // Logical page resolution & display labels match skip_pages + roman_end (#2b)
        assert_eq!(PdfEngine::resolve_logical_page_str_with_skip("i", 2, 3, 10), Some(2));
        assert_eq!(PdfEngine::resolve_logical_page_str_with_skip("iii", 2, 3, 10), Some(4));
        assert_eq!(PdfEngine::resolve_logical_page_str_with_skip("1", 2, 3, 10), Some(5));
        assert_eq!(PdfEngine::resolve_logical_page_str_with_skip("#2", 2, 3, 10), Some(1));
        assert_eq!(PdfEngine::format_logical_page_label_with_skip(0, 2, 3), "—");
        assert_eq!(PdfEngine::format_logical_page_label_with_skip(2, 2, 3), "i");
        assert_eq!(PdfEngine::format_logical_page_label_with_skip(4, 2, 3), "iii");
        assert_eq!(PdfEngine::format_logical_page_label_with_skip(5, 2, 3), "1");

        cfg.skip_pages = 0;
        cfg.roman_end = 0;
        cfg.format = PageNumberFormat::Dashed;
        assert_eq!(
            PdfEngine::format_page_number_label(&cfg, 1, 5),
            Some("- 1 -".to_string())
        );
        cfg.format = PageNumberFormat::NSlashTotal;
        assert_eq!(
            PdfEngine::format_page_number_label(&cfg, 4, 5),
            Some("4 / 5".to_string())
        );
    }

    #[test]
    fn test_header_footer_modal_opens_blank_and_export_help_menus_complete() {
        use super::doc_tools_modal::HeaderFooterModalState;
        use super::overlays::BOOKMARK_SAMPLE_TXT;
        use super::state::MenuCategory;
        use crate::engine::PdfEngine;

        // #1c: Header & Footer modal must always start completely blank (never stale footer text)
        let hf = HeaderFooterModalState::new(0);
        assert!(hf.profile.name.is_empty());
        assert!(hf.profile.header_left.is_empty());
        assert!(hf.profile.header_center.is_empty());
        assert!(hf.profile.header_right.is_empty());
        assert!(hf.profile.footer_left.is_empty());
        assert!(hf.profile.footer_center.is_empty());
        assert!(hf.profile.footer_right.is_empty());
        assert!(!hf.select_all);

        // #9: Dedicated Export menu with .docx, .xlsx, .md, .tex (and removed from File)
        assert!(MenuCategory::ALL.contains(&MenuCategory::Export));
        let exp = MenuCategory::Export.items();
        assert!(exp.iter().any(|s| s.contains(".docx")));
        assert!(exp.iter().any(|s| s.contains(".xlsx")));
        assert!(exp.iter().any(|s| s.contains(".md")));
        assert!(exp.iter().any(|s| s.contains(".tex")));
        assert!(!MenuCategory::File.items().iter().any(|s| s.contains("Markdown")));
        assert!(MenuCategory::File.items().iter().any(|s| s.contains("Close Other Tabs")));
        assert!(MenuCategory::File.items().iter().any(|s| s.contains("Close All Tabs")));

        // #3: Page menu has Insert Blank Page Before and Insert PDF(s) Before / After Page...
        assert!(MenuCategory::Page.items().iter().any(|s| s.contains("Insert Blank Page Before")));
        assert!(MenuCategory::Page.items().iter().any(|s| s.contains("Insert PDF(s)")));

        // #8: Help menu has Bookmark Import Format (.txt) & bundled sample parses cleanly
        assert!(MenuCategory::Help.items().iter().any(|s| s.contains("Bookmark Import Format")));
        let parsed = PdfEngine::parse_bookmarks_txt(BOOKMARK_SAMPLE_TXT, 4, 30);
        assert!(parsed.len() >= 7, "sample bookmark template must parse cleanly");
        assert_eq!(parsed[0].title, "Preface");
        assert_eq!(parsed[0].page_index, 0);
        assert_eq!(parsed[0].level, 0);
        assert!(parsed.iter().any(|b| b.level == 1 && b.title.contains("Overview")));
        assert!(parsed.iter().any(|b| b.page_index == 24 && b.title.contains("Appendix")));
    }

    #[test]
    fn test_reading_filters_have_six_eye_care_modes_with_day_and_night_recommendations() {
        use super::state::ReadingFilter;

        assert_eq!(ReadingFilter::ALL.len(), 6);
        assert!(
            ReadingFilter::PaperEInk.picker_title().contains("Best for day"),
            "PaperEInk must advertise Best for day: {}",
            ReadingFilter::PaperEInk.picker_title()
        );
        assert!(
            ReadingFilter::NightAmber.picker_title().contains("Best for night"),
            "NightAmber must advertise Best for night: {}",
            ReadingFilter::NightAmber.picker_title()
        );

        let mut f = ReadingFilter::Normal;
        for _ in 0..ReadingFilter::ALL.len() {
            assert!(!f.label().is_empty());
            assert!(!f.picker_title().is_empty());
            assert!(!f.picker_hint().is_empty());
            f = f.next();
        }
        assert_eq!(f, ReadingFilter::Normal);
    }

    #[test]
    fn test_scroll_mode_cycling_and_ini_strings() {
        use super::state::ScrollMode;

        let mut sm = ScrollMode::SinglePage;
        let seen = [
            ScrollMode::SinglePage,
            ScrollMode::ContinuousVertical,
            ScrollMode::ContinuousHorizontal,
            ScrollMode::MagazineBook,
        ];
        for expected in seen {
            assert_eq!(sm, expected);
            assert_eq!(ScrollMode::from_ini_str(sm.to_ini_str()), sm);
            sm = sm.next();
        }
        assert_eq!(sm, ScrollMode::SinglePage);
    }

    #[test]
    fn test_tier2_search_case_sensitive_and_whole_word() {
        let text = "Cat catalog CAT cat_dog cat.";
        let lower_hay: Vec<char> = text.chars().flat_map(|c| c.to_lowercase()).collect();
        let exact_hay: Vec<char> = text.chars().collect();
        let q_lower: Vec<char> = "cat".chars().collect();
        let q_cap: Vec<char> = "Cat".chars().collect();

        // Case-insensitive substring: "Cat", "cat" (in catalog), "CAT", "cat" (in cat_dog), "cat"
        let all_sub = PdfEngine::find_text_match_ranges(&lower_hay, &q_lower, false);
        assert_eq!(all_sub.len(), 5, "Expected 5 substring matches, got {all_sub:?}");

        // Case-insensitive whole word: "Cat", "CAT", "cat" (excludes catalog & cat_dog)
        let all_word = PdfEngine::find_text_match_ranges(&lower_hay, &q_lower, true);
        assert_eq!(all_word.len(), 3, "Expected 3 whole-word matches, got {all_word:?}");

        // Case-sensitive exact "Cat" whole word: only first token at 0..3
        let exact_word = PdfEngine::find_text_match_ranges(&exact_hay, &q_cap, true);
        assert_eq!(exact_word, vec![(0, 3)]);
    }

    #[test]
    fn test_tier2_measure_stats_and_units() {
        use super::measure::{compute_measurement_stats, MeasureUnit};

        // 72 pt horizontal segment = 1.00 in = 2.54 cm = 25.40 mm
        let s_in = compute_measurement_stats(10.0, 20.0, 82.0, 20.0, MeasureUnit::In);
        assert!((s_in.length - 1.0).abs() < 1e-4);
        assert!((s_in.dx - 1.0).abs() < 1e-4);
        assert!(s_in.dy.abs() < 1e-4);
        assert!(s_in.angle_deg.abs() < 1e-3);

        let s_mm = compute_measurement_stats(10.0, 20.0, 82.0, 20.0, MeasureUnit::Mm);
        assert!((s_mm.length - 25.4).abs() < 1e-3);

        let s_cm = compute_measurement_stats(10.0, 20.0, 82.0, 20.0, MeasureUnit::Cm);
        assert!((s_cm.length - 2.54).abs() < 1e-3);

        // 3-4-5 right triangle in PDF points: dx=216pt (3in), dy=288pt (4in) -> len=5in, area=12in²
        let tri = compute_measurement_stats(0.0, 0.0, 216.0, 288.0, MeasureUnit::In);
        assert!((tri.length - 5.0).abs() < 1e-3);
        assert!((tri.area - 12.0).abs() < 1e-3);
        assert!((tri.angle_deg - 53.1301).abs() < 0.05);
    }

    #[test]
    fn test_tier2_visual_diff_buffer() {
        use super::compare_mode::compute_visual_diff_buffer;

        // Identical white/black pixels -> 0.0% diff
        let a = vec![0x00FF_FFFF, 0x0010_1010, 0x00FF_FFFF, 0x00FF_FFFF];
        let (diff_same, pct_same) = compute_visual_diff_buffer(&a, &a);
        assert_eq!(diff_same.len(), 4);
        assert!(pct_same < 1e-5);

        // Pixel 0 darker on left -> Red (0x00DC_2626)
        // Pixel 1 darker on right -> Blue (0x0025_63EB)
        // Pixels 2 & 3 identical -> 50% diff
        let left = vec![0x0000_0000, 0x00FF_FFFF, 0x00FF_FFFF, 0x00FF_FFFF];
        let right = vec![0x00FF_FFFF, 0x0000_0000, 0x00FF_FFFF, 0x00FF_FFFF];
        let (diff, pct) = compute_visual_diff_buffer(&left, &right);
        assert_eq!(diff[0], 0x00DC_2626);
        assert_eq!(diff[1], 0x0025_63EB);
        assert!((pct - 50.0).abs() < 1e-3);
    }

    #[test]
    fn test_tier2_reading_continuity_and_lru_positions_ini() {
        use super::state::{LastPositionEntry, ReadingFilter, SessionConfig, MAX_LAST_POSITIONS};
        use std::path::PathBuf;

        let mut cfg = SessionConfig::default();
        cfg.reading_filter = ReadingFilter::NightAmber;
        for i in 0..(MAX_LAST_POSITIONS + 10) {
            cfg.last_positions.push(LastPositionEntry {
                path: PathBuf::from(format!("/tmp/doc_{i}.pdf")),
                page_index: i as u16,
                zoom_level: 1.25,
            });
        }
        let ini = PdfViewerApp::serialize_session_ini(&cfg);
        assert!(ini.contains("reading_filter=NightAmber"));
        assert!(ini.contains("last_pos=0|1.25|/tmp/doc_0.pdf"));

        let parsed = PdfViewerApp::parse_session_ini(&ini);
        assert_eq!(parsed.reading_filter, ReadingFilter::NightAmber);
        assert_eq!(parsed.last_positions.len(), MAX_LAST_POSITIONS);
        assert_eq!(parsed.last_positions[0].path, PathBuf::from("/tmp/doc_0.pdf"));
        assert_eq!(parsed.last_positions[0].page_index, 0);
        assert!((parsed.last_positions[0].zoom_level - 1.25).abs() < 1e-3);
        assert_eq!(parsed.last_positions[5].page_index, 5);
    }

    #[test]
    fn test_tier3_user_guide_chapters_and_export() {
        use super::user_guide::{format_full_user_guide_txt, GuideBlock, USER_GUIDE_CHAPTERS};

        assert_eq!(USER_GUIDE_CHAPTERS.len(), 11, "User Guide must have 11 chapters");
        for (idx, ch) in USER_GUIDE_CHAPTERS.iter().enumerate() {
            assert_eq!(ch.number as usize, idx + 1, "Chapter numbering must be sequential 1..=11");
            assert!(!ch.short_title.trim().is_empty());
            assert!(!ch.title.trim().is_empty());
            assert!(!ch.summary.trim().is_empty());
            let step_count = ch
                .blocks
                .iter()
                .filter(|b| matches!(b, GuideBlock::Step { .. }))
                .count();
            assert!(
                step_count >= 3,
                "Chapter {} ({}) should have at least 3 numbered steps, found {step_count}",
                ch.number,
                ch.title
            );
            let has_tip = ch.blocks.iter().any(|b| matches!(b, GuideBlock::Tip(_)));
            assert!(has_tip, "Chapter {} ({}) should include a beginner Tip callout", ch.number, ch.title);
        }

        let txt = format_full_user_guide_txt();
        assert!(txt.contains("MicroPDF v1.4 — Step-by-Step User Guide"));
        assert!(txt.contains("CHAPTER 1:"));
        assert!(txt.contains("CHAPTER 10:"));
        assert!(txt.contains("CHAPTER 11:"));
        assert!(txt.contains("Step 1:"));
        assert!(txt.contains("[TIP]"));
        assert!(txt.contains("Create PDF from Images"));
        assert!(txt.contains("Compare with Another PDF"));
        assert!(txt.contains("Measure Distance & Area"));
        assert!(txt.contains("Strip Personal Metadata & XMP"));
        assert!(txt.contains("Deskew Page(s)"));
        assert!(txt.contains("Auto (Hybrid)"));
        assert!(txt.contains(".docx"));
        assert!(txt.contains(".cbz"));
        assert!(txt.contains("Manga"));
    }

    #[test]
    fn test_milestone1_compare_viewing_modes_dimensions() {
        use super::compare_mode::{compute_compare_render_dimensions, CompareFitMode};
        // Letter page (612 x 792 pt) inside a 600x600 half-pane at 1.0x scale
        let (fw, fh) = compute_compare_render_dimensions(612.0, 792.0, 600, 600, CompareFitMode::FitPage, 1.0, 1.0);
        assert!(fw < 600 && fh <= 564, "FitPage must fit height & width: {fw}x{fh}");

        let (ww, wh) = compute_compare_render_dimensions(612.0, 792.0, 600, 600, CompareFitMode::FitWidth, 1.0, 1.0);
        assert_eq!(ww, 576, "FitWidth must match available pane width (600 - 24)");
        assert!(wh > fh, "FitWidth on portrait Letter should be taller than FitPage: {ww}x{wh}");

        let (aw, ah) = compute_compare_render_dimensions(612.0, 792.0, 600, 600, CompareFitMode::ActualSize, 1.0, 1.0);
        assert_eq!(aw, 816, "ActualSize 100% of 612pt at 96dpi = 816px");
        assert_eq!(ah, 1056, "ActualSize 100% of 792pt at 96dpi = 1056px");

        let (zw, zh) = compute_compare_render_dimensions(612.0, 792.0, 600, 600, CompareFitMode::FitPage, 2.0, 1.0);
        assert!((zw as i32 - (fw * 2) as i32).abs() <= 2 && (zh as i32 - (fh * 2) as i32).abs() <= 2,
            "200% zoom must double FitPage dimensions: {zw}x{zh} vs {fw}x{fh}");
    }

    #[test]
    fn test_milestone1_single_page_pdf_filename_and_range() {
        use super::split_pages_modal::{SplitPagesModalState, SplitPdfScope};
        assert_eq!(
            crate::engine::PdfEngine::format_single_page_pdf_name("Q4 Report", 1, 25),
            "Q4 Report_page_001.pdf"
        );
        assert_eq!(
            crate::engine::PdfEngine::format_single_page_pdf_name("bad/name:here", 12, 1200),
            "bad_name_here_page_0012.pdf"
        );
        assert_eq!(
            crate::engine::PdfEngine::format_single_page_pdf_name("   ", 5, 10),
            "page_page_005.pdf"
        );

        let mut st = SplitPagesModalState::new("demo".to_string());
        assert_eq!(st.resolved_pages(2, 5).unwrap(), vec![0, 1, 2, 3, 4]);
        st.scope = SplitPdfScope::Current;
        assert_eq!(st.resolved_pages(2, 5).unwrap(), vec![2]);
        st.scope = SplitPdfScope::Custom;
        st.custom_range = "1-2, 5".to_string();
        assert_eq!(st.resolved_pages(2, 5).unwrap(), vec![0, 1, 4]);
    }

    #[test]
    fn test_milestone2_stamp_and_signature_rasterization_and_storage() {
        use super::stamp_manager_modal::{
            ensure_asset_subdirs, rasterize_signature_pad_rgba, rasterize_text_stamp_rgba,
            SignatureInkColor, StampBorderStyle,
        };
        let stamp = rasterize_text_stamp_rgba("APPROVED", (22, 138, 68), StampBorderStyle::Double);
        assert!(stamp.width >= 140 && stamp.height >= 64);
        let stamp_rgba = stamp.image.to_rgba8();
        assert_eq!(stamp_rgba.as_raw().len(), (stamp.width * stamp.height * 4) as usize);
        assert!(stamp_rgba.as_raw().chunks_exact(4).any(|px| px[3] > 200 && px[1] == 138));
        assert!(stamp_rgba.as_raw().chunks_exact(4).any(|px| px[3] == 0), "stamp background must stay transparent");

        assert!(rasterize_signature_pad_rgba(&[], SignatureInkColor::Black).is_none());
        let strokes = vec![vec![(0.1, 0.5), (0.4, 0.3), (0.8, 0.6)]];
        let sig = rasterize_signature_pad_rgba(&strokes, SignatureInkColor::Blue).unwrap();
        assert_eq!((sig.width, sig.height), (480, 180));
        let sig_rgba = sig.image.to_rgba8();
        assert!(sig_rgba.as_raw().chunks_exact(4).any(|px| px[3] > 200 && px[2] == 178));

        let tmp = std::env::temp_dir().join("micropdf_m2_assets_test");
        let (sigs, stamps) = ensure_asset_subdirs(&tmp);
        assert!(sigs.is_dir() && stamps.is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_phase2_hypher_justified_wrapping_and_line_level_spread_pagination() {
        use super::markdown_view::{
            layout_blocks, paginate_book_pages, tokenize_spans, wrap_styled_words_ex,
            BookLayoutOpts, LaidOutBlock,
        };
        use super::ui_draw::FontFamily;
        use crate::markdown::{InlineSpan, MdBlock};
        use std::collections::HashMap;

        let opts = BookLayoutOpts {
            book_mode: true,
            font_family: FontFamily::BookSerif,
            line_mult: 1.56,
            word_spacing_px: 0.0,
            para_mult: 1.0,
            justify: true,
            hyphenate: true,
            hypher_lang: hypher::Lang::English,
        };
        let mut links = Vec::new();
        let text = "Typography extraordinary international characteristics communication representation considerations.";
        let words = tokenize_spans(&[InlineSpan::Text(text.to_string())], 16.0, &mut links, 0, opts);
        let lines = wrap_styled_words_ex(words, 165.0, 24.0, 16.0, opts);
        assert!(lines.len() >= 3, "Expected multi-line wrap, got {}", lines.len());
        assert!(lines[0].first_line_indent > 0.0, "Consecutive paragraph first line indent missing");
        assert!(!lines[0].is_last_in_para && lines.last().unwrap().is_last_in_para);
        let has_hyphen_break = lines[..lines.len() - 1]
            .iter()
            .any(|ln| ln.spans.last().is_some_and(|w| w.text.ends_with('-')));
        assert!(has_hyphen_break, "Expected hypher syllable hyphenation across narrow justified book column");

        // Build two paragraphs where the second long paragraph MUST split at line level across 2 book pages
        let long_sentence = "In the beginning the universe was created and every single carefully typeset line flows cleanly across the open spread without jarring empty gaps. ".repeat(6);
        let ast = vec![
            MdBlock::Paragraph(vec![InlineSpan::Text("Opening short paragraph.".to_string())]),
            MdBlock::Paragraph(vec![InlineSpan::Text(long_sentence)]),
        ];
        let mut code_blocks = Vec::new();
        let mut dims_cache = HashMap::new();
        let embedded = HashMap::new();
        let (blocks, _) = layout_blocks(
            &ast, 280.0, 16.0, 1.0, 0.0, &mut links, &mut code_blocks, &mut dims_cache, &embedded, true, None, opts, None,
        );
        let total_p2_lines = match &blocks[1] {
            LaidOutBlock::Paragraph { lines, .. } => lines.len(),
            _ => panic!("expected paragraph"),
        };
        assert!(total_p2_lines >= 8, "Expected >= 8 lines in paragraph 2, got {total_p2_lines}");

        // Choose a small page height (~5 lines tall) so paragraph 2 splits cleanly across multiple pages
        let pages = paginate_book_pages(&blocks, 130.0, true);
        assert!(pages.len() >= 3, "Expected cover + >=2 content pages, got {}", pages.len());
        assert!(pages[0].is_cover);
        let p2_slices: Vec<(usize, usize)> = pages[1..]
            .iter()
            .flat_map(|p| p.slices.iter())
            .filter(|s| s.block_idx == 1)
            .map(|s| (s.line_start, s.line_end))
            .collect();
        assert!(p2_slices.len() >= 2, "Long paragraph must split across >=2 pages at line granularity");
        assert_eq!(p2_slices.first().unwrap().0, 0);
        assert_eq!(p2_slices.last().unwrap().1, total_p2_lines);
        for pair in p2_slices.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "Line slices across page boundary must be contiguous with zero lost lines");
        }
    }

    #[test]
    fn test_phase2_epub_prefs_and_books_ini_roundtrip() {
        use super::book_ui::{
            parse_books_ini, serialize_books_ini, EpubBookData, EpubBookmark, EpubFontChoice,
            EpubHighlight, EpubReaderPrefs, LineSpacingPreset, MarginPreset,
        };
        use super::state::{PdfViewerApp, SessionConfig};
        use std::path::PathBuf;

        let mut cfg = SessionConfig::default();
        cfg.epub_prefs = EpubReaderPrefs {
            font_choice: EpubFontChoice::OsSerif("Georgia".to_string()),
            font_size_px: 19.5,
            line_spacing: LineSpacingPreset::Relaxed,
            line_spacing_mult: 1.75,
            word_spacing_px: 1.5,
            paragraph_spacing_mult: 1.30,
            margins: MarginPreset::Wide,
            margin_ratio: 0.22,
            margin_top: 0.14,
            margin_bottom: 0.12,
            margin_outer: 0.22,
            margin_inner: 0.15,
            justify: false,
            hyphenate: false,
            two_page_spread: false,
            page_curl_anim: false,
        };
        let ini = PdfViewerApp::serialize_session_ini(&cfg);
        let parsed = PdfViewerApp::parse_session_ini(&ini);
        assert_eq!(parsed.epub_prefs, cfg.epub_prefs);

        // Portable micropdf_books.ini annotations + bookmarks roundtrip
        let mut all_books = std::collections::BTreeMap::new();
        let mut book = EpubBookData::default();
        book.next_id = 2;
        book.highlights.push(EpubHighlight {
            id: 1,
            block_idx: 12,
            start_word: 3,
            end_word: 9,
            quote: "Call me | Ishmael\nChapter 1".to_string(),
            color_idx: 2,
            note: Some("Famous opening | line\nKeep for essay".to_string()),
        });
        book.bookmarks.push(EpubBookmark {
            block_idx: 12,
            page_num: 4,
            label: "Chapter 1: Loomings".to_string(),
        });
        all_books.insert("books/moby_dick.epub".to_string(), book.clone());
        let _ = PathBuf::from("books/moby_dick.epub");
        let saved_books = serialize_books_ini(&all_books);
        let parsed_books = parse_books_ini(&saved_books);
        assert_eq!(parsed_books.get("books/moby_dick.epub"), Some(&book));
    }

    #[test]
    fn test_epub_feedback_margins_chapter_breaks_search_and_curl() {
        use super::book_ui::{EpubReaderPrefs, MarginPreset};
        use super::markdown_view::{
            layout_blocks, paginate_book_pages, search_markdown_doc, BookLayoutOpts,
        };
        use super::state::MenuCategory;
        use super::ui_draw::{draw_apple_books_page_curl, FontFamily};
        use super::user_guide::USER_GUIDE_CHAPTERS;
        use crate::markdown::{InlineSpan, MarkdownDoc, MdBlock};
        use std::collections::HashMap;

        // 1. Top/vertical padding scales cleanly with Narrow / Normal / Wide and 4-side margin controls
        let mut prefs = EpubReaderPrefs::default();
        prefs.set_margin_preset(MarginPreset::Narrow);
        let (top_narrow, bot_narrow) = prefs.vertical_pad_px(800.0, 1.0);
        prefs.set_margin_preset(MarginPreset::Normal);
        let (top_normal, bot_normal) = prefs.vertical_pad_px(800.0, 1.0);
        prefs.set_margin_preset(MarginPreset::Wide);
        let (top_wide, bot_wide) = prefs.vertical_pad_px(800.0, 1.0);
        assert!(top_narrow < top_normal && top_normal < top_wide);
        assert!(bot_narrow < bot_normal && bot_normal < bot_wide);
        assert!(top_normal >= 44.0, "Normal top margin should be generous, got {top_normal}");

        prefs.set_margin_slider_norm(0.0);
        let (top_min, _) = prefs.vertical_pad_px(800.0, 1.0);
        prefs.set_margin_slider_norm(1.0);
        let (top_max, _) = prefs.vertical_pad_px(800.0, 1.0);
        assert!(top_min < top_max);

        // 2. Chapters separated by MdBlock::PageBreak MUST start on separate book pages
        // even when both chapters are tiny and page height is huge.
        let ast = vec![
            MdBlock::Heading { level: 1, spans: vec![InlineSpan::Text("Chapter 1".into())] },
            MdBlock::Paragraph(vec![InlineSpan::Text("Short first chapter.".into())]),
            MdBlock::PageBreak,
            MdBlock::Heading { level: 1, spans: vec![InlineSpan::Text("Chapter 2".into())] },
            MdBlock::Paragraph(vec![InlineSpan::Text("Short second chapter.".into())]),
        ];
        let opts = BookLayoutOpts {
            book_mode: true,
            font_family: FontFamily::BookSerif,
            line_mult: 1.5,
            word_spacing_px: 0.0,
            para_mult: 1.0,
            justify: true,
            hyphenate: false,
            hypher_lang: hypher::Lang::English,
        };
        let mut links = Vec::new();
        let mut code_blocks = Vec::new();
        let mut dims_cache = HashMap::new();
        let embedded = HashMap::new();
        let (blocks, _) = layout_blocks(
            &ast, 400.0, 16.0, 1.0, 0.0, &mut links, &mut code_blocks, &mut dims_cache, &embedded, true, None, opts, None,
        );
        let pages = paginate_book_pages(&blocks, 900.0, false);
        assert_eq!(pages.len(), 2, "Two chapters separated by PageBreak must produce 2 separate pages");
        assert!(pages[0].slices.iter().all(|s| s.ast_block_idx < 2));
        assert!(pages[1].slices.iter().all(|s| s.ast_block_idx >= 3));

        // 3. Full-text search across MarkdownDoc (EPUB, Markdown, LaTeX)
        let md_doc = MarkdownDoc {
            title: "Search Book".into(),
            blocks: vec![
                MdBlock::Heading { level: 1, spans: vec![InlineSpan::Text("Quantum Mechanics".into())] },
                MdBlock::Paragraph(vec![InlineSpan::Text("A cat sits in a catalog with another Cat.".into())]),
                MdBlock::Blockquote(vec![MdBlock::Paragraph(vec![InlineSpan::Text("Every quantum state evolves.".into())])]),
            ],
            headings: Vec::new(),
            embedded_images: HashMap::new(),
            book_meta: None,
        };
        assert_eq!(search_markdown_doc(&md_doc, "quantum", false, false).len(), 2);
        assert_eq!(search_markdown_doc(&md_doc, "Quantum", true, false).len(), 1);
        assert_eq!(search_markdown_doc(&md_doc, "cat", false, false).len(), 3);
        assert_eq!(search_markdown_doc(&md_doc, "cat", false, true).len(), 2);
        assert_eq!(search_markdown_doc(&md_doc, "Cat", true, true).len(), 1);

        // 4. Apple Books 3D hand page-curl renderer paints cleanly into CPU softbuffer
        let (w, h) = (320u32, 200u32);
        let mut cur_buf = vec![0x00FB_F8F1u32; (w * h) as usize];
        let prev_buf = vec![0x00E8_E2D5u32; (w * h) as usize];
        let before = cur_buf.clone();
        draw_apple_books_page_curl(
            &mut cur_buf, w, h, &prev_buf, 10, 10, 300, 180, 0.45, 1, 0x00FB_F8F1, 1.0,
        );
        assert_ne!(cur_buf, before, "Forward 3D page curl must render curved peel onto frame buffer");
        draw_apple_books_page_curl(
            &mut cur_buf, w, h, &prev_buf, 10, 10, 300, 180, 0.55, -1, 0x001B_1E24, 1.0,
        );

        // 5. Global wording: Open Documents & Find in the document
        assert!(MenuCategory::File.items().iter().any(|it| it.contains("Open Documents")));
        assert!(MenuCategory::Edit.items().iter().any(|it| it.contains("Find in the document")));
        for ch in USER_GUIDE_CHAPTERS {
            for b in ch.blocks {
                let txt = match b {
                    super::user_guide::GuideBlock::Heading(s)
                    | super::user_guide::GuideBlock::Paragraph(s)
                    | super::user_guide::GuideBlock::Tip(s) => *s,
                    super::user_guide::GuideBlock::Step { text, .. } => *text,
                };
                assert!(!txt.contains("Open PDF"), "Stale 'Open PDF' wording in User Guide: {txt}");
                assert!(!txt.contains("Find Text in PDF"), "Stale 'Find Text in PDF' wording in User Guide: {txt}");
            }
        }
    }

    /// Generates authentic pixel-accurate PNG screenshots of the new Distinct UI Skin
    /// directly from MicroPDF's CPU `softbuffer` renderer into `docs/screenshots/`.
    /// Run via:
    /// `MICROPDF_PDFIUM_DIR=/path/to/pdfium cargo test --release --locked generate_distinct_ui_screenshots -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn generate_distinct_ui_screenshots() {
        use super::state::{InputMode, PdfViewerApp, ReadingFilter};
        use super::theme::{UiSkin, UiThemeMode};
        use crate::engine::PdfEngine;
        use crate::zip_mini::ZipWriter;
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        fn save_u32_rgb_png(buf: &[u32], w: u32, h: u32, path: &std::path::Path) {
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for &px in buf {
                rgb.push(((px >> 16) & 0xFF) as u8);
                rgb.push(((px >> 8) & 0xFF) as u8);
                rgb.push((px & 0xFF) as u8);
            }
            let img = image::RgbImage::from_raw(w, h, rgb).expect("valid rgb buffer");
            img.save(path).expect("save screenshot png");
        }

        let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs").join("screenshots");
        std::fs::create_dir_all(&out_dir).expect("create docs/screenshots");

        let tmp = std::env::temp_dir().join("micropdf_showcase_v17");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create tmp showcase dir");

        // 1. Sample Markdown document
        let md_path = tmp.join("Architecture_Notes.md");
        let md_text = r#"# MicroPDF v1.7 Architecture

MicroPDF renders every menu, tab, sidebar, and document page directly into a CPU frame buffer with **zero GPU overhead** and *zero GUI toolkit bloat*.

## Key Design Principles

- **Portable-First**: All settings live in `micropdf.ini` beside the executable.
- **Bounded Memory**: Inactive tabs hold zero bitmaps; image caches evict cleanly.
- **Multi-Family Typography**: Embedded `BookSerif`, `UiSans`, and `CodeMono` fonts.

## Performance Snapshot

| Subsystem | Strategy | Peak Overhead |
| :--- | :--- | ---: |
| EPUB Layout | Lock-free font metrics + lazy images | 24 MB cap |
| PDF Zoom | 512x512 LRU tile cache | 16 MB cap |
| Page Curl | Single spread snapshot (freed on end) | 0 MB idle |

## Equation Support

Inline math like $E = mc^2$ and display equations render crisply offline:

$$\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}$$

```rust
pub fn render_frame(buffer: &mut [u32], width: u32, height: u32) {
    // Pure CPU antialiased rendering into 0x00RRGGBB
    draw_distinct_workspace(buffer, width, height);
}
```
"#;
        std::fs::write(&md_path, md_text).expect("write md");

        // 2. Sample LaTeX document
        let tex_path = tmp.join("Quantum_Notes.tex");
        let tex_text = r#"\title{Variational Principles in Quantum Mechanics}
\author{Dr. Elena Vance}

\section{Introduction}
In quantum mechanics, the time evolution of a pure state vector $\psi(x, t)$ is governed by the Schrödinger equation. MicroPDF parses sections, inline math, numbered equations, and citations cleanly offline.

\section{Hamiltonian Formulation}
The stationary states satisfy the eigenvalue problem shown in Equation~\eqref{eq:schrodinger}:

\begin{equation}
\label{eq:schrodinger}
\hat{H}\psi = \left(-\frac{\hbar^2}{2m}\nabla^2 + V(x)\right)\psi = E\psi
\end{equation}

\subsection{Expectation Value Bound}
For any normalized trial wavefunction $\phi$, the variational energy satisfies:

\begin{equation}
\label{eq:variational}
E_0 \le \frac{\langle \phi | \hat{H} | \phi \rangle}{\langle \phi | \phi \rangle}
\end{equation}

\section{Summary}
Equation~\eqref{eq:variational} provides a rigorous upper bound on the ground-state energy $E_0$ across atomic and molecular systems.
"#;
        std::fs::write(&tex_path, tex_text).expect("write tex");

        let engine = PdfEngine::new().expect("init pdfium");

        // 3. Sample PDF generated from Markdown via MicroPDF's own PDF exporter
        let pdf_path = tmp.join("Quarterly_Report.pdf");
        let pdf_md = crate::markdown::parse_markdown(
            r#"# Quarterly Engineering Report

**Prepared by:** MicroPDF Core Team  
**Quarter:** Q4 — Portable Release v1.7.0

## Executive Summary

MicroPDF delivers instant startup, crisp typography, and a unified reading and editing workflow across **PDF**, **EPUB**, **Markdown**, and **LaTeX** documents — all in a single portable folder with zero installer footprint.

## Release Highlights

| Feature | Status | Target Platforms |
| :--- | :--- | :--- |
| Distinct Contextual UI Skin | Shipped | Windows, macOS, Linux |
| Apple Books-Style EPUB Reader | Shipped | Windows, macOS, Linux |
| Markdown & LaTeX Split Studio | Shipped | Windows, macOS, Linux |
| Universal Ctrl+F Document Search | Shipped | Windows, macOS, Linux |

## Memory Discipline

By probing image headers during layout and decoding visible figures into a bounded 24 MB cache, even 15 MB illustrated e-books reflow instantaneously when changing UI zoom or typography margins.
"#,
            "Quarterly Engineering Report",
        );
        crate::markdown_pdf::export_markdown_to_pdf(
            &engine.pdfium,
            &pdf_md,
            &pdf_path,
            crate::markdown_pdf::MdPdfPageSize::Letter,
        )
        .expect("export sample pdf");

        // 4. Sample EPUB with generated cover PNG, nested TOC, and multi-chapter prose
        let epub_path = tmp.join("The_Voyage_of_the_Horizon.epub");
        let mut cover_img = image::RgbImage::new(240, 340);
        for (x, y, px) in cover_img.enumerate_pixels_mut() {
            let t = y as f32 / 340.0;
            let r = (24.0 + t * 28.0) as u8;
            let g = (48.0 + t * 54.0 + ((x as f32 / 240.0) * 18.0)) as u8;
            let b = (84.0 + t * 62.0) as u8;
            *px = image::Rgb([r, g, b]);
        }
        let mut cover_png = Vec::new();
        cover_img
            .write_to(&mut std::io::Cursor::new(&mut cover_png), image::ImageFormat::Png)
            .expect("encode cover png");

        let mut zw = ZipWriter::new();
        zw.add_stored("mimetype", b"application/epub+zip");
        zw.add_deflated(
            "META-INF/container.xml",
            br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#,
        );
        zw.add_deflated(
            "OEBPS/content.opf",
            br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>The Voyage of the Horizon</dc:title>
    <dc:creator>Clara Sterling</dc:creator>
    <dc:publisher>Meridian Press</dc:publisher>
    <dc:language>en-US</dc:language>
    <dc:description>An illustrated maritime chronicle of celestial navigation, quiet harbors, and the craft of bookmaking.</dc:description>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="cover" href="images/cover.png" media-type="image/png" properties="cover-image"/>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch3" href="ch3.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
    <itemref idref="ch3"/>
  </spine>
</package>"#,
        );
        zw.add_deflated("OEBPS/images/cover.png", &cover_png);
        zw.add_deflated(
            "OEBPS/nav.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body><nav epub:type="toc"><ol>
  <li><a href="ch1.xhtml">Chapter I: Departure at Dawn</a>
    <ol>
      <li><a href="ch1.xhtml#harbor">The Quiet Harbor</a></li>
      <li><a href="ch1.xhtml#compass">Setting the Compass</a></li>
    </ol>
  </li>
  <li><a href="ch2.xhtml">Chapter II: Across the Open Meridian</a></li>
  <li><a href="ch3.xhtml">Chapter III: Lanterns of the Far Coast</a></li>
</ol></nav></body></html>"#,
        );
        zw.add_deflated(
            "OEBPS/ch1.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <h1>Chapter I: Departure at Dawn</h1>
  <p>Before the first silver light touched the breakwater, the harbor lay motionless beneath a pale amber sky. Along the timber wharf, brass lanterns swayed gently in the morning breeze while sailors coiled hemp lines and checked the chronometer for the long crossing ahead.</p>
  <h2 id="harbor">The Quiet Harbor</h2>
  <p>Every voyage begins in stillness. Inside the chartroom, leather-bound logbooks rested beside brass dividers and carefully annotated coastal charts. The navigator traced a delicate arc across the northern strait, noting prevailing currents, shoals, and the familiar rhythm of autumn tides.</p>
  <blockquote>"A well-typeset page, like a calm horizon, invites the eye to travel effortlessly without noticing the craft that holds it steady."</blockquote>
  <p>When the sails finally caught the offshore wind, the hull answered with a steady, reassuring murmur. Gulls wheeled above the stern, and the distant stone lighthouse shrank into a bright pinprick against the waking hills.</p>
  <h2 id="compass">Setting the Compass</h2>
  <p>By midmorning the open sea revealed its full expanse. Crisp whitecaps marched eastward under high cirrus clouds, and the crew settled into the familiar cadence of watches, sextant sightings, and quiet hours spent reading beneath the canvas awning.</p>
  <p>In the afternoon, the barometer held steady at thirty inches. The helmsman kept the bowsprit aligned with the distant swell while the captain recorded latitude, wind bearing, and water temperature in neat italic ink.</p>
  <p>As dusk gathered, the horizon softened from cobalt to slate violet, and the first stars emerged directly ahead of the masthead, promising a clear and unhurried night under sail.</p>
</body></html>"#,
        );
        zw.add_deflated(
            "OEBPS/ch2.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <h1>Chapter II: Across the Open Meridian</h1>
  <p>Three days out from the coast, the ocean became a library of light and motion. Each morning brought a fresh chapter of trade winds, translucent turquoise swells, and long uninterrupted passages of reflection on deck.</p>
  <p>Beneath the skylight of the aft cabin, the navigator compared antique celestial tables with fresh noon observations. The precision of the instruments mirrored the harmony of the stars overhead, turning an empty expanse of water into a readable map.</p>
  <p>Toward evening a warm southerly breeze filled the mainsail, carrying the scent of distant rain and urging the vessel smoothly across the quiet meridian line toward the western islands.</p>
</body></html>"#,
        );
        zw.add_deflated(
            "OEBPS/ch3.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <h1>Chapter III: Lanterns of the Far Coast</h1>
  <p>On the seventh night, a faint amber glow appeared along the western horizon. The lookout called from the crosstrees, and every hand gathered at the rail to watch the harbor beacons emerge through the coastal mist.</p>
  <p>By sunrise the anchor chain rattled softly into clear green water, bringing the crossing to a peaceful close.</p>
</body></html>"#,
        );
        std::fs::write(&epub_path, zw.finish()).expect("write epub");

        let mut app = PdfViewerApp::new(&engine, None).expect("init app");
        app.ui_skin = UiSkin::Distinct;
        app.ui_theme_mode = UiThemeMode::Light;
        app.reading_filter = ReadingFilter::WarmSepia;

        // Open all 4 document types so the tab bar showcases all 4 file-type icons!
        app.open_document_in_new_tab(pdf_path.clone()).expect("open pdf");
        app.open_document_in_new_tab(md_path.clone()).expect("open md");
        app.open_document_in_new_tab(tex_path.clone()).expect("open tex");
        app.open_document_in_new_tab(epub_path.clone()).expect("open epub");

        let (w, h) = (1440u32, 900u32);
        let mut buf = vec![0u32; (w * h) as usize];

        // Screenshot 1: Distinct EPUB Light (2-page open book spread + Book Drawer sidebar)
        app.show_bookmarks = true;
        app.show_aa_popover = false;
        app.epub_prefs.two_page_spread = true;
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        // Step past cover onto first 2-page spread (Chapter I)
        app.step_md_magazine_spread(1, w, h.saturating_sub(100), 1.0);
        app.page_turn_anim = None;
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        save_u32_rgb_png(&buf, w, h, &out_dir.join("01_epub_distinct_light.png"));

        // Screenshot 2: Distinct EPUB with redesigned Aa Typography & Reading Popover open
        app.show_bookmarks = true;
        app.show_aa_popover = true;
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        save_u32_rgb_png(&buf, w, h, &out_dir.join("02_epub_aa_typography_popover.png"));

        // Screenshot 3: Distinct EPUB Dark Mode + Apple Books 3D Page Curl + Universal Search
        app.show_aa_popover = false;
        app.show_bookmarks = false;
        app.ui_theme_mode = UiThemeMode::Dark;
        app.reading_filter = ReadingFilter::DarkInvert;
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        app.execute_text_search("harbor");
        app.input_mode = InputMode::SearchText {
            query_input: "harbor".to_string(),
        };
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        // Trigger a live 3D page-curl mid-frame (progress ~42%)
        let prev_y = app.scroll_y;
        app.step_md_magazine_spread(1, w, h.saturating_sub(100), 1.0);
        if let Some(ref mut anim) = app.page_turn_anim {
            anim.started_at = Instant::now() - Duration::from_millis(180);
            anim.duration_ms = 450;
            anim.from_scroll_y = Some(prev_y);
        }
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        save_u32_rgb_png(&buf, w, h, &out_dir.join("03_epub_page_curl_dark.png"));

        // Clear search & anim for subsequent screenshots
        app.page_turn_anim = None;
        app.input_mode = InputMode::Normal;
        app.md_search_matches.clear();
        app.search_query.clear();

        // Screenshot 4: Distinct Markdown Split Studio (Light)
        app.ui_theme_mode = UiThemeMode::Light;
        app.reading_filter = ReadingFilter::Normal;
        app.switch_to_tab(1);
        app.show_bookmarks = true;
        app.md_editor_mode = super::text_editor::MdEditorMode::Split;
        app.ensure_md_editor_initialized();
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        save_u32_rgb_png(&buf, w, h, &out_dir.join("04_markdown_studio_split.png"));

        // Screenshot 5: Distinct LaTeX Academic Studio (Dark)
        app.ui_theme_mode = UiThemeMode::Dark;
        app.switch_to_tab(2);
        app.show_bookmarks = true;
        app.md_editor_mode = super::text_editor::MdEditorMode::Split;
        app.ensure_md_editor_initialized();
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        save_u32_rgb_png(&buf, w, h, &out_dir.join("05_latex_studio_dark.png"));

        // Screenshot 6: Distinct PDF Studio (Light)
        app.ui_theme_mode = UiThemeMode::Light;
        app.reading_filter = ReadingFilter::Normal;
        app.switch_to_tab(0);
        app.show_bookmarks = false;
        app.render_frame_into_buffer(&mut buf, w, h, 1.0);
        save_u32_rgb_png(&buf, w, h, &out_dir.join("06_pdf_distinct_light.png"));

        let _ = std::fs::remove_dir_all(&tmp);
        println!("Generated 6 screenshots in {}", out_dir.display());
    }

    #[test]
    fn test_ten_ux_tweaks_regressions() {
        use super::book_ui::{
            AaPopoverHit, BookSidebarHit, BookSidebarTab, EpubBookmark, EpubHighlight,
            EpubReaderPrefs,
        };
        use super::markdown_view::{layout_blocks, BookLayoutOpts, LaidOutBlock};
        use super::state::{InputMode, MenuCategory, PdfViewerApp, SessionConfig};
        use super::theme::UiSkin;
        use super::toolbar::ToolbarAction;
        use super::ui_draw::FontFamily;
        use crate::markdown::{BookMetadata, InlineSpan, MarkdownDoc, MdBlock, TocEntry};
        use std::collections::HashMap;
        use std::path::PathBuf;

        // 1. Independent 4-side EPUB margins: changing Top must NOT alter Inner/Spine, Bottom, or Outer
        let mut prefs = EpubReaderPrefs::default();
        let orig_inner = prefs.effective_inner_margin_ratio();
        let orig_outer = prefs.effective_margin_ratio();
        let orig_bottom = prefs.margin_bottom;
        prefs.set_margin_part_norm(0, 1.0); // Max Top margin
        assert!(prefs.margin_top > orig_bottom);
        assert!((prefs.effective_inner_margin_ratio() - orig_inner).abs() < 1e-5, "Top margin must not widen spine gap");
        assert!((prefs.effective_margin_ratio() - orig_outer).abs() < 1e-5, "Top margin must not alter outer side");
        assert!((prefs.margin_bottom - orig_bottom).abs() < 1e-5, "Top margin must not alter bottom");

        // 2. Line, word & paragraph spacing + separate Code & Preview font INI roundtrip
        let mut cfg = SessionConfig::default();
        cfg.md_code_font_px = 16.5;
        cfg.md_preview_font_px = 18.0;
        cfg.md_line_spacing = 1.75;
        cfg.md_word_spacing_px = 2.0;
        cfg.md_para_spacing_mult = 1.60;
        let ini = PdfViewerApp::serialize_session_ini(&cfg);
        let parsed = PdfViewerApp::parse_session_ini(&ini);
        assert!((parsed.md_code_font_px - 16.5).abs() < 0.01);
        assert!((parsed.md_preview_font_px - 18.0).abs() < 0.01);
        assert!((parsed.md_line_spacing - 1.75).abs() < 0.01);
        assert!((parsed.md_word_spacing_px - 2.0).abs() < 0.01);
        assert!((parsed.md_para_spacing_mult - 1.60).abs() < 0.01);

        // Verify paragraph spacing multiplier & word spacing increase layout height cleanly
        let ast = vec![
            MdBlock::Paragraph(vec![InlineSpan::Text("Alpha beta gamma delta.".into())]),
            MdBlock::Paragraph(vec![InlineSpan::Text("Second paragraph after gap.".into())]),
        ];
        let base_opts = BookLayoutOpts {
            book_mode: false,
            font_family: FontFamily::BookSerif,
            line_mult: 1.45,
            word_spacing_px: 0.0,
            para_mult: 1.0,
            justify: false,
            hyphenate: false,
            hypher_lang: hypher::Lang::English,
        };
        let wide_opts = BookLayoutOpts {
            line_mult: 1.85,
            word_spacing_px: 3.0,
            para_mult: 2.0,
            ..base_opts
        };
        let mut l1 = Vec::new();
        let mut cb1 = Vec::new();
        let mut d1 = HashMap::new();
        let emb = HashMap::new();
        let (b_base, h_base) = layout_blocks(&ast, 300.0, 15.0, 1.0, 0.0, &mut l1, &mut cb1, &mut d1, &emb, false, None, base_opts, None);
        let (_, h_wide) = layout_blocks(&ast, 300.0, 15.0, 1.0, 0.0, &mut l1, &mut cb1, &mut d1, &emb, false, None, wide_opts, None);
        assert!(matches!(b_base[0], LaidOutBlock::Paragraph { .. }));
        assert!(h_wide > h_base + 10.0, "Larger line/para spacing must increase total height");

        // 3. PdfViewerApp-backed checks (runs whenever PDFium shared library is available)
        let Ok(engine) = crate::engine::PdfEngine::new() else {
            return;
        };
        let app_empty = PdfViewerApp::new(&engine, None).expect("init empty app");
        assert!(!app_empty.menu_item_enabled(MenuCategory::Page, 0));
        assert!(!app_empty.menu_item_enabled(MenuCategory::Tools, 0));
        assert!(app_empty.toolbar_action_enabled(ToolbarAction::Open));
        assert!(!app_empty.toolbar_action_enabled(ToolbarAction::Save));
        assert!(!app_empty.toolbar_action_enabled(ToolbarAction::Print));

        // Separate Code vs Preview font sizes on Markdown tab via Aa handler + persistent Find bar
        let mut app = PdfViewerApp::new(&engine, None).expect("init app");
        app.ui_skin = UiSkin::Distinct;
        app.file_path = PathBuf::from("notes.md");
        app.markdown_doc = Some(MarkdownDoc {
            title: "Notes".into(),
            blocks: ast,
            headings: Vec::new(),
            embedded_images: HashMap::new(),
            book_meta: None,
        });
        assert!(app.menu_item_enabled(MenuCategory::File, 5)); // Save Document enabled for MD
        assert!(!app.menu_item_enabled(MenuCategory::Page, 0)); // PDF Page menu disabled
        assert!(app.toolbar_action_enabled(ToolbarAction::MdSave));

        let orig_code = app.md_code_font_px;
        let orig_prev = app.md_preview_font_px;
        app.handle_aa_popover_click(AaPopoverHit::CodeSizePlus);
        assert!(app.md_code_font_px > orig_code);
        assert!((app.md_preview_font_px - orig_prev).abs() < 0.01);
        app.handle_aa_popover_click(AaPopoverHit::SizePlus);
        assert!(app.md_preview_font_px > orig_prev);

        // Persistent Find bar stays open after handle_prompt_submission
        app.input_mode = InputMode::SearchText {
            query_input: "Alpha".to_string(),
        };
        app.handle_prompt_submission();
        assert!(matches!(app.input_mode, InputMode::SearchText { ref query_input } if query_input == "Alpha"));
        assert!(app.search_bar_focused);

        // 4. EPUB sidebar width, scrollbar across tabs, chapter Loc labels & chapter time remaining
        app.file_path = PathBuf::from("novel.epub");
        let many_toc: Vec<TocEntry> = (0..40)
            .map(|i| TocEntry {
                title: format!("Chapter {}", i + 1),
                level: 0,
                block_idx: i % 2,
                children: Vec::new(),
            })
            .collect();
        let mut long_ast = Vec::new();
        for i in 0..4 {
            long_ast.push(MdBlock::Heading {
                level: 1,
                spans: vec![InlineSpan::Text(format!("Chapter {}", i + 1))],
            });
            long_ast.push(MdBlock::Paragraph(vec![InlineSpan::Text("word ".repeat(260))]));
        }
        app.markdown_doc = Some(MarkdownDoc {
            title: "Novel".into(),
            blocks: long_ast,
            headings: Vec::new(),
            embedded_images: HashMap::new(),
            book_meta: Some(BookMetadata {
                title: "Novel".into(),
                author: Some("Author".into()),
                publisher: None,
                language: None,
                description: None,
                cover_image_key: None,
                toc: many_toc,
            }),
        });
        app.show_bookmarks = true;
        assert!(app.sidebar_width_px(1.0) >= 315);
        assert!(!app.menu_item_enabled(MenuCategory::File, 5)); // Save disabled for read-only EPUB

        // Ensure scrollbar metrics overflow & click scrolls on Contents, Annotations & Bookmarks tabs
        for i in 0..25u64 {
            app.epub_book_data.highlights.push(EpubHighlight {
                id: i + 1,
                block_idx: 0,
                start_word: 0,
                end_word: 2,
                quote: format!("Quote {i}"),
                color_idx: 0,
                note: Some("Note".into()),
            });
            app.epub_book_data.bookmarks.push(EpubBookmark {
                block_idx: 0,
                page_num: (i + 1) as usize,
                label: format!("Bookmark {i}"),
            });
        }
        for tab in [BookSidebarTab::Contents, BookSidebarTab::Annotations, BookSidebarTab::Bookmarks] {
            app.book_sidebar_tab = tab;
            app.bookmark_scroll_y = 0;
            let (_, _, _, tot, vis) = app.epub_sidebar_list_metrics(420, 64, 1.0);
            assert!(tot > vis, "Expected overflow for scrollbar on {tab:?}");
            app.handle_distinct_epub_sidebar_click(BookSidebarHit::ScrollbarTrack(1.0), 420, 64, 1.0);
            assert!(app.bookmark_scroll_y > 0, "Scrollbar click must scroll {tab:?}");
        }

        app.ensure_md_layout(1280, 1.0);
        let time_lbl = app.epub_chapter_time_left_label().expect("Expected chapter time remaining");
        assert!(time_lbl.contains("left"), "Unexpected time-left label: {time_lbl}");
        let full_loc = app.epub_loc_and_time_label(1, 5);
        assert!(full_loc.contains("Loc. 1 of 5 ·"), "Expected combined Loc + time-left, got {full_loc}");
        assert!(app.epub_loc_for_block(0) >= 1);
    }

    #[test]
    fn test_milestone8_6_format_palettes_docx_cbz_and_cache_discipline() {
        use crate::comic::ComicArchive;
        use crate::docx_reader::parse_docx_bytes;
        use crate::markdown::parse_markdown;
        use crate::markdown_docx::export_markdown_to_docx;
        use crate::zip_mini::ZipWriter;
        use std::collections::HashSet;
        use super::theme::{DocKind, UiSkin, UiThemeMode};

        // 1. All 6 DocKinds must map from extensions and expose 6 distinct accents in Light & Dark
        let all_kinds = [
            DocKind::Pdf,
            DocKind::Markdown,
            DocKind::LaTeX,
            DocKind::Epub,
            DocKind::Word,
            DocKind::Comic,
        ];
        assert_eq!(DocKind::from_path(Path::new("doc.pdf")), DocKind::Pdf);
        assert_eq!(DocKind::from_path(Path::new("notes.md")), DocKind::Markdown);
        assert_eq!(DocKind::from_path(Path::new("paper.tex")), DocKind::LaTeX);
        assert_eq!(DocKind::from_path(Path::new("book.epub")), DocKind::Epub);
        assert_eq!(DocKind::from_path(Path::new("spec.docx")), DocKind::Word);
        assert_eq!(DocKind::from_path(Path::new("manga.cbz")), DocKind::Comic);

        for mode in [UiThemeMode::Light, UiThemeMode::Dark] {
            let mut accents = HashSet::new();
            for kind in all_kinds {
                let theme = mode.theme_for(UiSkin::Distinct, kind);
                assert!(
                    accents.insert(theme.accent),
                    "Duplicate Distinct accent color {:#08X} for {kind:?} in {mode:?}",
                    theme.accent
                );
            }
            assert_eq!(accents.len(), 6);
        }

        let tmp = std::env::temp_dir().join(format!(
            "micropdf_m8_6fmt_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        // 2. Real Word (.docx) roundtrip -> headings TOC & blocks
        let docx_src_md = parse_markdown(
            "# Executive Summary\n\nIntro **bold** and *italic*.\n\n## Findings\n\n| Metric | Value |\n| --- | --- |\n| Speed | Fast |\n\n- Item A\n- Item B\n",
            "Executive Spec",
        );
        let docx_path = tmp.join("spec.docx");
        export_markdown_to_docx(&docx_src_md, &docx_path).expect("export synthetic docx");
        let docx_raw = std::fs::read(&docx_path).expect("read synthetic docx");
        let parsed_docx = parse_docx_bytes(&docx_raw, "Executive Spec").expect("parse synthetic docx");
        assert!(parsed_docx.headings.len() >= 2, "Expected Word Navigation Pane headings");
        assert!(!parsed_docx.blocks.is_empty(), "Expected Word parsed blocks");

        // 3. Real Comic (.cbz) with PNG + JPEG + WebP + ComicInfo.xml -> decode & RTL/Spread checks
        let encode_img = |r: u8, g: u8, b: u8, fmt: image::ImageFormat| -> Vec<u8> {
            let mut img = image::RgbaImage::new(16, 24);
            for p in img.pixels_mut() {
                *p = image::Rgba([r, g, b, 255]);
            }
            let mut out = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut std::io::Cursor::new(&mut out), fmt)
                .expect("encode comic frame");
            out
        };
        let png_b = encode_img(220, 40, 60, image::ImageFormat::Png);
        let jpg_b = encode_img(40, 120, 220, image::ImageFormat::Jpeg);
        let webp_b = encode_img(30, 180, 90, image::ImageFormat::WebP);
        let mut zw = ZipWriter::new();
        zw.add_deflated(
            "ComicInfo.xml",
            br#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Series>Neon Horizon</Series>
  <Title>Chapter 1</Title>
  <Writer>A. Mori</Writer>
  <Manga>YesAndRightToLeft</Manga>
</ComicInfo>"#,
        );
        zw.add_stored("p10.webp", &webp_b);
        zw.add_stored("p2.jpg", &jpg_b);
        zw.add_stored("p1.png", &png_b);
        let cbz_path = tmp.join("neon.cbz");
        std::fs::write(&cbz_path, zw.finish()).unwrap();

        let archive = ComicArchive::open(&cbz_path).expect("open synthetic cbz");
        assert_eq!(archive.pages, vec!["p1.png", "p2.jpg", "p10.webp"]);
        assert!(archive.info.as_ref().is_some_and(|i| i.manga_rtl));
        for idx in 0..3 {
            let bmp = archive.decode_page(idx).expect("decode comic page");
            assert_eq!((bmp.width, bmp.height), (16, 24));
        }
        assert_eq!(
            PdfViewerApp::comic_spread_visual_pages_for(3, 1, true, true),
            (Some(2), Some(1))
        );

        // 4. Full 6-tab PdfViewerApp integration (runs when PDFium shared library is available)
        if let Ok(engine) = crate::engine::PdfEngine::new() {
            let md_path = tmp.join("notes.md");
            let tex_path = tmp.join("paper.tex");
            let epub_path = tmp.join("book.epub");
            let pdf_path = tmp.join("report.pdf");
            let docx_export_pdf = tmp.join("spec_exported.pdf");
            std::fs::write(&md_path, "# Notes\n\nMarkdown body.\n").unwrap();
            std::fs::write(&tex_path, "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\nHello \\LaTeX.\n\\end{document}\n").unwrap();

            let mut ezw = ZipWriter::new();
            ezw.add_stored("mimetype", b"application/epub+zip");
            ezw.add_deflated(
                "META-INF/container.xml",
                br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            );
            ezw.add_deflated(
                "OEBPS/content.opf",
                br#"<package xmlns="http://www.idpf.org/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Book</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
            );
            ezw.add_deflated("OEBPS/c1.xhtml", b"<html><body><h1>Ch 1</h1><p>EPUB body.</p></body></html>");
            std::fs::write(&epub_path, ezw.finish()).unwrap();

            crate::markdown_pdf::export_markdown_to_pdf(
                &engine.pdfium,
                &docx_src_md,
                &pdf_path,
                crate::markdown_pdf::MdPdfPageSize::A4,
            )
            .expect("create sample pdf");
            crate::markdown_pdf::export_markdown_to_pdf(
                &engine.pdfium,
                &parsed_docx,
                &docx_export_pdf,
                crate::markdown_pdf::MdPdfPageSize::A4,
            )
            .expect("export docx to pdf");
            assert!(std::fs::metadata(&docx_export_pdf).unwrap().len() > 200);

            let mut app = PdfViewerApp::new(&engine, Some(pdf_path)).expect("open pdf tab");
            app.ui_skin = UiSkin::Distinct;
            app.open_document_in_new_tab(md_path).expect("open md tab");
            app.open_document_in_new_tab(tex_path).expect("open tex tab");
            app.open_document_in_new_tab(epub_path).expect("open epub tab");
            app.open_document_in_new_tab(docx_path.clone()).expect("open docx tab");
            app.open_document_in_new_tab(cbz_path.clone()).expect("open cbz tab");

            assert_eq!(app.tabs.len(), 6, "All 6 formats should be open in tabs");
            assert_eq!(app.active_doc_kind(), DocKind::Comic);
            assert!(app.comic_prefs.rtl_manga, "ComicInfo.xml must enable RTL Manga on open");
            assert!(app.ensure_comic_page_cached(0).is_some());
            assert!(app.ensure_comic_thumb_cached(0, 120, 160).is_some());
            assert!(!app.comic_page_cache.is_empty());
            assert!(!app.comic_thumb_cache.is_empty());

            // Switching away from .cbz must flush all decoded comic pixel buffers immediately
            app.switch_to_tab(4);
            assert_eq!(app.active_doc_kind(), DocKind::Word);
            assert!(app.comic_page_cache.is_empty(), "Inactive CBZ tab must drop comic_page_cache");
            assert!(app.comic_thumb_cache.is_empty(), "Inactive CBZ tab must drop comic_thumb_cache");
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}





