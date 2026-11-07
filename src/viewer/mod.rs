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
        //    v1.2.1: a stale pre-reload snapshot must not survive (Ctrl+Z would revert the change)
        app.undo_stack.push_back(b"stale snapshot".to_vec());
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

        // 1. Open Tools -> Optimize... (item 7 in Tools menu)
        app.execute_menu_item(state::MenuCategory::Tools, 7);
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
        app.execute_menu_item(state::MenuCategory::Tools, 7);
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
}
