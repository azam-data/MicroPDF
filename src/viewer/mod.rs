mod bookmarks;
mod grid;
mod input;
mod menu;
mod render;
mod state;
mod statusbar;
mod tabs;

use crate::engine::PdfEngine;
use anyhow::{Context, Result};
use std::path::Path;
use winit::event_loop::EventLoop;

#[allow(unused_imports)]
pub use state::{MenuCategory, MouseMode, PdfViewerApp, ReadingFilter, ViewMode};

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

        // Primary instance polls listener and opens pdf_b in Tab 2
        let opened = app.poll_single_instance_listener();
        assert_eq!(opened, 1);
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
}
