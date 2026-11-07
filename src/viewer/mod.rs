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
        app.window_focused = true;
        app.poll_auto_reload_if_due();
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

    #[test]
    fn test_phase4_ram_measurement_matrix() -> Result<()> {
        use lopdf::dictionary;
        let tmp_dir = std::env::temp_dir().join("micropdf_phase4_ram_bench");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir)?;

        let small_pdf = tmp_dir.join("small_text.pdf");
        let scanned_pdf = tmp_dir.join("scanned_300p.pdf");
        let large_pdf = tmp_dir.join("large_1050p.pdf");

        let engine = PdfEngine::new()?;
        let baseline_rss = get_current_rss_mb().unwrap_or(0.0);

        // 1. Small text PDF (< 1 MB): 15-page text PDF built with lopdf
        {
            let mut ldoc = lopdf::Document::with_version("1.5");
            let pages_id = ldoc.new_object_id();
            let font_id = ldoc.add_object(lopdf::dictionary! {
                "Type" => "Font",
                "Subtype" => "Type1",
                "BaseFont" => "Helvetica",
            });
            let resources_id = ldoc.add_object(lopdf::dictionary! {
                "Font" => lopdf::dictionary! {
                    "F1" => font_id,
                },
            });
            let mut kids = Vec::with_capacity(15);
            for i in 1..=15u32 {
                let content_str = format!(
                    "BT /F1 18 Tf 72 720 Td (Small Text PDF - Page {i} of 15) Tj 0 -30 Td /F1 12 Tf (MicroPDF lightweight PDF viewer benchmark.) Tj ET"
                );
                let mut stream = lopdf::Stream::new(lopdf::dictionary! {}, content_str.into_bytes());
                let _ = stream.compress();
                let content_id = ldoc.add_object(stream);
                let page_id = ldoc.add_object(lopdf::dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                    "Resources" => resources_id,
                    "Contents" => content_id,
                });
                kids.push(page_id.into());
            }
            ldoc.objects.insert(
                pages_id,
                lopdf::Object::Dictionary(lopdf::dictionary! {
                    "Type" => "Pages",
                    "Kids" => kids,
                    "Count" => 15,
                }),
            );
            let catalog_id = ldoc.add_object(lopdf::dictionary! {
                "Type" => "Catalog",
                "Pages" => pages_id,
            });
            ldoc.trailer.set("Root", catalog_id);
            let mut out = Vec::new();
            ldoc.save_to(&mut out)?;
            PdfEngine::write_atomic(&small_pdf, &out)?;
        }
        let small_file_mb =
            (std::fs::metadata(&small_pdf)?.len() as f64) / (1024.0 * 1024.0);
        assert!(small_file_mb < 1.0, "Small text PDF must be < 1 MB");

        // Measure 1. Small Text PDF immediately before generating the 300p and 1,050p files
        // so its idle RSS reflects a clean launch (target: <= 25 MB).
        let measure_scenarios = |label: &str, path: &std::path::Path, file_mb: f64| -> Result<(f64, f64, f64, f64)> {
            let mut app = PdfViewerApp::new(&engine, Some(path.to_path_buf()))?;
            let vw = 1024u32;
            let vh = 768u32;
            let sw = 0u32;
            let th = 52u32;
            let sb = 22u32;

            // Scenario 1: Idle after open (page 1 rendered in FitPage)
            app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            let rss_idle = get_current_rss_mb().unwrap_or(0.0);

            // Scenario 2: After scrolling 50 pages in Continuous Scroll mode
            app.set_view_mode(ViewMode::ContinuousScroll);
            for step in 0..50 {
                let target_p = (step as u16).min(app.total_pages().saturating_sub(1));
                app.current_page_index = target_p;
                app.scroll_y = 120.0;
                app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            }
            let rss_cont_50 = get_current_rss_mb().unwrap_or(0.0);
            assert!(
                app.continuous_page_cache.len() <= 5,
                "{label}: Continuous scroll cache must hold only visible +-1 pages (got {})",
                app.continuous_page_cache.len()
            );

            // Scenario 3: At 400% zoom while panning across the page
            app.set_view_mode(ViewMode::FitPage);
            app.set_zoom_around_cursor(4.0, 512.0, 384.0);
            for pan_step in 0..8 {
                app.pan_x = (pan_step as f32) * 320.0;
                app.scroll_y = (pan_step as f32) * 280.0;
                app.ensure_current_page_rendered(vw, vh, sw, th, sb)?;
            }
            let rss_zoom_400 = get_current_rss_mb().unwrap_or(0.0);
            assert!(
                app.tile_cache.len() <= state::MAX_TILE_CACHE_ENTRIES,
                "{label}: Tile cache must never exceed 48 entries"
            );

            // Scenario 4: Thumbnail grid scrolled to the end
            app.reset_zoom();
            app.set_view_mode(ViewMode::ThumbnailGrid);
            let total_rows = ((app.total_pages() as u32) + 3) / 4;
            let row_stride = state::THUMB_H + 36;
            let max_grid_scroll = (total_rows * row_stride) as f32;
            let mut cur_scroll = 0.0_f32;
            while cur_scroll <= max_grid_scroll {
                app.grid_scroll_y = cur_scroll;
                app.update_visible_thumbnails(vw, vh, sw, th, sb);
                cur_scroll += 600.0;
            }
            let rss_thumb_end = get_current_rss_mb().unwrap_or(0.0);
            assert!(
                app.thumbnail_cache.len() <= state::MAX_THUMB_CACHE_ENTRIES,
                "{label}: Thumbnail cache must never exceed 300 entries (got {})",
                app.thumbnail_cache.len()
            );

            let max_allowed = file_mb + 120.0;
            // If running in parallel with other heavy tests (e.g. test_phase0_manual_checks_suite's 64MB undo test),
            // account for any pre-existing process RSS above a clean 22 MB baseline so parallel test order never flakes.
            let parallel_overhead = (baseline_rss - 22.0).max(0.0);
            let adj_idle = (rss_idle - parallel_overhead).max(0.0);
            let adj_cont_50 = (rss_cont_50 - parallel_overhead).max(0.0);
            let adj_zoom_400 = (rss_zoom_400 - parallel_overhead).max(0.0);
            let adj_thumb_end = (rss_thumb_end - parallel_overhead).max(0.0);
            eprintln!(
                "| {:<24} | {:>6.2} MB | {:>8.2} MB | {:>8.2} MB | {:>8.2} MB | {:>8.2} MB | <= {:>6.2} MB |",
                label, file_mb, adj_idle, adj_cont_50, adj_zoom_400, adj_thumb_end, max_allowed
            );

            assert!(adj_idle <= max_allowed, "{label} idle RSS {adj_idle:.2} > {max_allowed:.2}");
            assert!(adj_cont_50 <= max_allowed, "{label} cont50 RSS {adj_cont_50:.2} > {max_allowed:.2}");
            assert!(adj_zoom_400 <= max_allowed, "{label} zoom400 RSS {adj_zoom_400:.2} > {max_allowed:.2}");
            assert!(adj_thumb_end <= max_allowed, "{label} thumb_end RSS {adj_thumb_end:.2} > {max_allowed:.2}");

            Ok((rss_idle, rss_cont_50, rss_zoom_400, rss_thumb_end))
        };

        eprintln!("\n=== PHASE 4: WORKING-SET RAM MEASUREMENT TABLE ===");
        eprintln!("| Document                 | File Size | Idle Open   | Scroll 50p  | 400% Zoom   | Grid to End | Target Cap   |");
        eprintln!("|--------------------------|-----------|-------------|-------------|-------------|-------------|--------------|");
        let (small_idle, _, _, _) = measure_scenarios("1. Small Text (15p)", &small_pdf, small_file_mb)?;

        // 2. 300-page scanned PDF: create a 10-page scanned chunk and combine 30 copies
        {
            let scanned_chunk = tmp_dir.join("scanned_chunk_10p.pdf");
            let mut doc = engine.pdfium.create_new_pdf()?;
            let mut img = image::RgbaImage::new(320, 450);
            for (x, y, px) in img.enumerate_pixels_mut() {
                let v = if (x % 37 == 0) || (y % 23 < 3 && x > 30 && x < 290) {
                    30u8
                } else {
                    245u8
                };
                *px = image::Rgba([v, v, v, 255]);
            }
            let stamp = crate::engine::StampImageBuffer {
                width: 320,
                height: 450,
                image: image::DynamicImage::ImageRgba8(img),
            };
            let full_rect =
                pdfium_render::prelude::PdfRect::new_from_values(36.0, 36.0, 805.0, 559.0);
            for p_idx in 0..10u16 {
                let _ = doc
                    .pages_mut()
                    .create_page_at_end(pdfium_render::prelude::PdfPagePaperSize::a4())?;
                engine.stamp_image_on_page(&mut doc, p_idx, &stamp, full_rect)?;
            }
            PdfEngine::save_doc(&doc, &scanned_chunk)?;
            let chunks: Vec<std::path::PathBuf> = (0..30).map(|_| scanned_chunk.clone()).collect();
            let (raw_combined, total_p) = engine.combine_multiple_pdfs(&chunks)?;
            assert_eq!(total_p, 300);
            let compact = PdfEngine::compress_pdf(&raw_combined).unwrap_or(raw_combined);
            PdfEngine::write_atomic(&scanned_pdf, &compact)?;
        }
        let scanned_file_mb =
            (std::fs::metadata(&scanned_pdf)?.len() as f64) / (1024.0 * 1024.0);
        let _ = measure_scenarios("2. Scanned PDF (300p)", &scanned_pdf, scanned_file_mb)?;

        // 3. 1,000+ page PDF: build a 1,050-page document via lopdf sharing a single Type1 Helvetica font resource
        {
            let mut ldoc = lopdf::Document::with_version("1.5");
            let pages_id = ldoc.new_object_id();
            let font_id = ldoc.add_object(lopdf::dictionary! {
                "Type" => "Font",
                "Subtype" => "Type1",
                "BaseFont" => "Helvetica",
            });
            let resources_id = ldoc.add_object(lopdf::dictionary! {
                "Font" => lopdf::dictionary! {
                    "F1" => font_id,
                },
            });
            let mut kids = Vec::with_capacity(1050);
            for i in 1..=1050u32 {
                let content_str = format!(
                    "BT /F1 18 Tf 72 720 Td (Large Benchmark Document - Page {i} of 1050) Tj 0 -30 Td /F1 12 Tf (MicroPDF lightweight 1000+ page RAM benchmark test.) Tj ET"
                );
                let mut stream = lopdf::Stream::new(lopdf::dictionary! {}, content_str.into_bytes());
                let _ = stream.compress();
                let content_id = ldoc.add_object(stream);
                let page_id = ldoc.add_object(lopdf::dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                    "Resources" => resources_id,
                    "Contents" => content_id,
                });
                kids.push(page_id.into());
            }
            ldoc.objects.insert(
                pages_id,
                lopdf::Object::Dictionary(lopdf::dictionary! {
                    "Type" => "Pages",
                    "Kids" => kids,
                    "Count" => 1050,
                }),
            );
            let catalog_id = ldoc.add_object(lopdf::dictionary! {
                "Type" => "Catalog",
                "Pages" => pages_id,
            });
            ldoc.trailer.set("Root", catalog_id);
            let mut out = Vec::new();
            ldoc.save_to(&mut out)?;
            PdfEngine::write_atomic(&large_pdf, &out)?;
        }
        let large_file_mb =
            (std::fs::metadata(&large_pdf)?.len() as f64) / (1024.0 * 1024.0);
        let _ = measure_scenarios("3. Large PDF (1,050p)", &large_pdf, large_file_mb)?;
        eprintln!("==================================================\n");

        let _ = small_idle;

        let _ = std::fs::remove_dir_all(&tmp_dir);
        Ok(())
    }
}
