#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod viewer;

use anyhow::Result;
use engine::PdfEngine;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Generates `test_output.pdf` using the Phase 1A structural self-test pipeline.
fn generate_default_test_pdf(engine: &PdfEngine) -> Result<PathBuf> {
    let start_time = Instant::now();
    println!("=== MicroPDF: Generating default 'test_output.pdf' ===");

    let input_pdf = "test_input.pdf";
    let output_pdf = "test_output.pdf";

    if engine.create_sample_pdf(input_pdf, 10).is_ok() {
        let _ = engine.add_page_numbers(input_pdf, output_pdf, 5);
        let _ = engine.add_header_footer(output_pdf, output_pdf, Some("MicroPDF Draft"), None, 1, 10);
        let _ = engine.add_watermark(output_pdf, output_pdf, "CONFIDENTIAL", true);
        let _ = engine.rotate_pages(output_pdf, output_pdf, &[2], 90);
        println!("Generated '{output_pdf}' in {:.2?}.", start_time.elapsed());
        return Ok(PathBuf::from(output_pdf));
    }

    // Fallback to OS temp dir if current directory is read-only
    let temp_output = std::env::temp_dir().join("micro_pdf_welcome.pdf");
    engine.create_sample_pdf(&temp_output, 5)?;
    Ok(temp_output)
}

fn resolve_startup_pdf(engine: &PdfEngine) -> Result<(PathBuf, bool)> {
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
                eprintln!(
                    "Warning: CLI path {:?} does not exist, falling back to default document...",
                    candidate
                );
            }
        }
    }

    // 1. If a valid PDF path was passed via CLI ("Open With -> micro_pdf.exe"), open it directly
    if let Some(path) = cli_pdf {
        return Ok((path, run_self_test));
    }

    // 2. Otherwise, if `test_output.pdf` exists in current directory, open it automatically
    let default_path = Path::new("test_output.pdf");
    if default_path.exists() {
        return Ok((default_path.to_path_buf(), run_self_test));
    }

    // 3. If neither exists, generate `test_output.pdf` and open it
    let generated = generate_default_test_pdf(engine)?;
    Ok((generated, run_self_test))
}

fn verify_external_pdfs_with_load_pdf_safe(engine: &PdfEngine) {
    println!("=== Testing load_pdf_safe on local & ~/Downloads PDFs ===");
    let mut dirs_to_scan = vec![PathBuf::from(".")];
    if let Some(home) = std::env::var_os("HOME") {
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

    viewer::run_viewer(&engine, &startup_pdf)
}
