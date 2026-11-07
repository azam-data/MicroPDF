#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod viewer;

use anyhow::Result;
use engine::PdfEngine;
use std::path::PathBuf;
use std::time::Instant;

/// Generates a temporary sample PDF for `--self-test` verification only.
fn generate_self_test_pdf(engine: &PdfEngine) -> Result<PathBuf> {
    let start_time = Instant::now();
    println!("=== MicroPDF: Generating self-test PDF ===");

    let temp_dir = std::env::temp_dir();
    let input_pdf = temp_dir.join("micropdf_self_test_input.pdf");
    let output_pdf = temp_dir.join("micropdf_self_test_output.pdf");

    engine.create_sample_pdf(&input_pdf, 10)?;
    let _ = engine.add_page_numbers(&input_pdf, &output_pdf, 5);
    let _ = engine.add_header_footer(&output_pdf, &output_pdf, Some("MicroPDF Draft"), None, 1, 10);
    let _ = engine.add_watermark(&output_pdf, &output_pdf, "CONFIDENTIAL", true);
    let _ = engine.rotate_pages(&output_pdf, &output_pdf, &[2], 90);
    let _ = std::fs::remove_file(&input_pdf);
    println!("Generated {:?} in {:.2?}.", output_pdf, start_time.elapsed());
    Ok(output_pdf)
}

fn resolve_startup_pdf(engine: &PdfEngine) -> Result<(Option<PathBuf>, bool)> {
    let mut run_self_test = false;
    let mut cli_pdf: Option<PathBuf> = None;

    for arg in std::env::args_os().skip(1) {
        if arg == "--self-test" {
            run_self_test = true;
        } else if cli_pdf.is_none() {
            let candidate = PathBuf::from(&arg);
            if candidate.exists() {
                cli_pdf = Some(candidate);
            } else {
                eprintln!("Warning: CLI path {:?} does not exist.", candidate);
            }
        }
    }

    if let Some(path) = cli_pdf {
        return Ok((Some(path), run_self_test));
    }

    if run_self_test {
        let generated = generate_self_test_pdf(engine)?;
        return Ok((Some(generated), true));
    }

    Ok((None, false))
}

fn verify_external_pdfs_with_load_pdf_safe(engine: &PdfEngine) {
    println!("=== Testing load_pdf_safe on local & Downloads PDFs ===");
    let mut dirs_to_scan = vec![PathBuf::from(".")];
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let mut downloads = PathBuf::from(home);
        downloads.push("Downloads");
        dirs_to_scan.push(downloads);
    }

    for dir in dirs_to_scan {
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let mut pdf_paths: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|de| de.path()))
                    .filter(|p| {
                        p.is_file()
                            && p.extension()
                                .and_then(|ext| ext.to_str())
                                .map(|ext| ext.eq_ignore_ascii_case("pdf"))
                                .unwrap_or(false)
                    })
                    .collect();
                pdf_paths.sort();

                for pdf_path in pdf_paths {
                    match engine.load_pdf_safe(&pdf_path) {
                        Ok(doc) => {
                            let pages = doc.pages().len();
                            println!("  [OK] {:?} -> {} page(s)", pdf_path, pages);
                        }
                        Err(e) => {
                            println!("  [ERR] {:?} -> {}", pdf_path, e);
                        }
                    }
                }
            }
            Err(e) => {
                println!("  [SKIP] Could not list directory {:?}: {}", dir, e);
            }
        }
    }
    println!("=========================================================");
}

fn main() -> Result<()> {
    // Single-instance check: if a PDF path is passed on the CLI and MicroPDF is already running,
    // forward the path over the local socket and exit within 200 ms before loading PDFium.
    let mut cli_pdf_for_ipc: Option<PathBuf> = None;
    let mut is_self_test = false;
    for arg in std::env::args_os().skip(1) {
        if arg == "--self-test" {
            is_self_test = true;
        } else if cli_pdf_for_ipc.is_none() {
            let candidate = PathBuf::from(&arg);
            if candidate.exists() {
                cli_pdf_for_ipc = Some(candidate);
            }
        }
    }
    if !is_self_test {
        if let Some(ref pdf_path) = cli_pdf_for_ipc {
            if viewer::try_forward_to_running_instance(pdf_path, None) {
                return Ok(());
            }
        }
    }

    let engine = PdfEngine::new()?;
    let (startup_pdf, run_self_test) = resolve_startup_pdf(&engine)?;

    if run_self_test {
        verify_external_pdfs_with_load_pdf_safe(&engine);
    }

    println!(
        "Launching MicroPDF Viewer with {:?} (Current RSS: {:.2} MB)...",
        startup_pdf,
        viewer::get_current_rss_mb().unwrap_or(0.0)
    );

    viewer::run_viewer(&engine, startup_pdf.as_deref())
}
