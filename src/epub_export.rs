//! Spec-compliant pure-Rust EPUB 3 Exporter (`src/zip_mini.rs`).
//!
//! Exports any `MarkdownDoc` (from `.md`, `.tex`, or extracted `PdfDocument` blocks)
//! into a valid `.epub` archive with uncompressed `mimetype` first entry,
//! `META-INF/container.xml`, `OEBPS/content.opf`, `OEBPS/nav.xhtml`, `OEBPS/toc.ncx`,
//! `OEBPS/style.css`, and XHTML chapters auto-split on H1 headings.

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

use crate::markdown::{InlineSpan, MarkdownDoc, MdAlign, MdBlock, MdListItem};
use crate::zip_mini::ZipWriter;

struct EpubChapter {
    id: String,
    file: String,
    title: String,
    xhtml: String,
}

pub fn export_markdown_doc_to_epub(doc: &MarkdownDoc, out_path: &Path) -> Result<()> {
    let bytes = export_markdown_doc_to_epub_bytes(doc);
    std::fs::write(out_path, &bytes)?;
    Ok(())
}

pub fn export_markdown_doc_to_epub_bytes(doc: &MarkdownDoc) -> Vec<u8> {
    let mut zw = ZipWriter::new();
    // EPUB spec requires `mimetype` as the first, uncompressed entry
    zw.add_stored("mimetype", b"application/epub+zip");
    zw.add_deflated(
        "META-INF/container.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#,
    );

    let css = r#"body { font-family: serif; line-height: 1.55; margin: 1.2em; color: #1f2937; }
h1, h2, h3, h4, h5, h6 { font-family: sans-serif; color: #111827; margin-top: 1.2em; margin-bottom: 0.4em; }
p { margin: 0.5em 0; }
pre { background: #f3f4f6; padding: 0.75em; border-radius: 4px; overflow-x: auto; font-family: monospace; font-size: 0.9em; }
code { background: #f3f4f6; padding: 0.1em 0.3em; border-radius: 3px; font-family: monospace; font-size: 0.9em; }
blockquote { border-left: 3px solid #3b82f6; margin: 0.8em 0; padding: 0.3em 0.9em; color: #4b5563; background: #f9fafb; }
table { border-collapse: collapse; width: 100%; margin: 0.8em 0; }
th, td { border: 1px solid #d1d5db; padding: 0.4em 0.6em; text-align: left; }
th { background: #f3f4f6; font-weight: bold; }
.math-block { text-align: center; font-family: serif; font-style: italic; margin: 0.8em 0; padding: 0.5em; background: #f9fafb; }
figure { text-align: center; margin: 1em 0; }
img { max-width: 100%; height: auto; }
figcaption { font-size: 0.88em; color: #6b7280; margin-top: 0.3em; }
hr { border: none; border-top: 1px solid #d1d5db; margin: 1.2em 0; }
"#;
    zw.add_deflated("OEBPS/style.css", css.as_bytes());

    // Split blocks into chapters at level-1 headings
    let mut raw_chapters: Vec<(String, Vec<&MdBlock>)> = Vec::new();
    let mut cur_title = doc.title.clone();
    let mut cur_blocks: Vec<&MdBlock> = Vec::new();

    for blk in &doc.blocks {
        if let MdBlock::Heading { level: 1, spans } = blk {
            if !cur_blocks.is_empty() {
                raw_chapters.push((cur_title.clone(), std::mem::take(&mut cur_blocks)));
            }
            let t: String = spans.iter().map(|s| s.as_plain_text()).collect();
            let t = t.trim();
            if !t.is_empty() {
                cur_title = t.to_string();
            }
        }
        cur_blocks.push(blk);
    }
    if !cur_blocks.is_empty() || raw_chapters.is_empty() {
        raw_chapters.push((cur_title, cur_blocks));
    }

    let mut img_manifest: Vec<(String, String, String)> = Vec::new(); // (id, href, media_type)
    let mut img_seen: HashMap<String, String> = HashMap::new();
    let mut chapters: Vec<EpubChapter> = Vec::with_capacity(raw_chapters.len());

    for (idx, (ch_title, ch_blocks)) in raw_chapters.into_iter().enumerate() {
        let num = idx + 1;
        let id = format!("ch{num:03}");
        let file = format!("ch_{num:03}.xhtml");
        let mut body = String::new();
        for blk in ch_blocks {
            render_block_xhtml(
                blk,
                doc,
                &mut body,
                &mut img_seen,
                &mut img_manifest,
                &mut zw,
            );
        }
        if body.is_empty() {
            body.push_str("<p></p>\n");
        }
        let esc_title = xml_escape(&ch_title);
        let xhtml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE html>\n\
             <html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\">\n\
             <head>\n\
               <meta charset=\"UTF-8\"/>\n\
               <title>{esc_title}</title>\n\
               <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>\n\
             </head>\n\
             <body>\n{body}</body>\n</html>\n"
        );
        zw.add_deflated(&format!("OEBPS/{file}"), xhtml.as_bytes());
        chapters.push(EpubChapter {
            id,
            file,
            title: ch_title,
            xhtml,
        });
    }

    // EPUB 3 nav.xhtml
    let mut nav_items = String::new();
    for ch in &chapters {
        nav_items.push_str(&format!(
            "      <li><a href=\"{}\">{}</a></li>\n",
            xml_escape(&ch.file),
            xml_escape(&ch.title)
        ));
    }
    let nav_xhtml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE html>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
         <head><meta charset=\"UTF-8\"/><title>Table of Contents</title></head>\n\
         <body>\n  <nav epub:type=\"toc\" id=\"toc\">\n    <h1>Table of Contents</h1>\n    <ol>\n{nav_items}    </ol>\n  </nav>\n</body>\n</html>\n"
    );
    zw.add_deflated("OEBPS/nav.xhtml", nav_xhtml.as_bytes());

    // EPUB 2 toc.ncx
    let mut ncx_points = String::new();
    for (i, ch) in chapters.iter().enumerate() {
        let play = i + 1;
        ncx_points.push_str(&format!(
            "    <navPoint id=\"np-{play}\" playOrder=\"{play}\">\n      <navLabel><text>{}</text></navLabel>\n      <content src=\"{}\"/>\n    </navPoint>\n",
            xml_escape(&ch.title),
            xml_escape(&ch.file)
        ));
    }
    let toc_ncx = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n\
           <head><meta name=\"dtb:uid\" content=\"urn:micropdf:epub\"/></head>\n\
           <docTitle><text>{}</text></docTitle>\n\
           <navMap>\n{ncx_points}  </navMap>\n</ncx>\n",
        xml_escape(&doc.title)
    );
    zw.add_deflated("OEBPS/toc.ncx", toc_ncx.as_bytes());

    // OEBPS/content.opf
    let mut manifest_items = String::from(
        "    <item id=\"style\" href=\"style.css\" media-type=\"text/css\"/>\n\
         <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n\
         <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n",
    );
    let mut spine_items = String::new();
    for ch in &chapters {
        let _ = &ch.xhtml;
        manifest_items.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            ch.id,
            xml_escape(&ch.file)
        ));
        spine_items.push_str(&format!("    <itemref idref=\"{}\"/>\n", ch.id));
    }
    for (id, href, media) in &img_manifest {
        manifest_items.push_str(&format!(
            "    <item id=\"{id}\" href=\"{}\" media-type=\"{media}\"/>\n",
            xml_escape(href)
        ));
    }

    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"uid\">\n\
           <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
             <dc:identifier id=\"uid\">urn:micropdf:epub</dc:identifier>\n\
             <dc:title>{}</dc:title>\n\
             <dc:language>en</dc:language>\n\
             <dc:creator>MicroPDF</dc:creator>\n\
           </metadata>\n\
           <manifest>\n{manifest_items}  </manifest>\n\
           <spine toc=\"ncx\">\n{spine_items}  </spine>\n\
         </package>\n",
        xml_escape(&doc.title)
    );
    zw.add_deflated("OEBPS/content.opf", opf.as_bytes());

    zw.finish()
}

fn render_block_xhtml(
    blk: &MdBlock,
    doc: &MarkdownDoc,
    out: &mut String,
    img_seen: &mut HashMap<String, String>,
    img_manifest: &mut Vec<(String, String, String)>,
    zw: &mut ZipWriter,
) {
    match blk {
        MdBlock::Heading { level, spans } => {
            let lvl = (*level).clamp(1, 6);
            out.push_str(&format!("<h{lvl}>{}</h{lvl}>\n", render_spans_xhtml(spans)));
        }
        MdBlock::Paragraph(spans) => {
            out.push_str(&format!("<p>{}</p>\n", render_spans_xhtml(spans)));
        }
        MdBlock::CodeBlock { code, .. } => {
            out.push_str(&format!("<pre><code>{}</code></pre>\n", xml_escape(code)));
        }
        MdBlock::Blockquote(inner) => {
            out.push_str("<blockquote>\n");
            for b in inner {
                render_block_xhtml(b, doc, out, img_seen, img_manifest, zw);
            }
            out.push_str("</blockquote>\n");
        }
        MdBlock::List { ordered, items, .. } => {
            let tag = if *ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}>\n"));
            for it in items {
                render_list_item_xhtml(it, doc, out, img_seen, img_manifest, zw);
            }
            out.push_str(&format!("</{tag}>\n"));
        }
        MdBlock::Table { alignments, headers, rows } => {
            out.push_str("<table>\n<thead><tr>");
            for (ci, h) in headers.iter().enumerate() {
                let align_attr = align_style(alignments.get(ci).copied().unwrap_or(MdAlign::None));
                out.push_str(&format!("<th{align_attr}>{}</th>", render_spans_xhtml(h)));
            }
            out.push_str("</tr></thead>\n<tbody>\n");
            for row in rows {
                out.push_str("<tr>");
                for (ci, cell) in row.iter().enumerate() {
                    let align_attr = align_style(alignments.get(ci).copied().unwrap_or(MdAlign::None));
                    out.push_str(&format!("<td{align_attr}>{}</td>", render_spans_xhtml(cell)));
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</tbody></table>\n");
        }
        MdBlock::HorizontalRule => out.push_str("<hr/>\n"),
        MdBlock::MathBlock(tex) => {
            let readable = crate::math_render::math_to_readable_text(tex);
            out.push_str(&format!("<div class=\"math-block\">{}</div>\n", xml_escape(&readable)));
        }
        MdBlock::Image { path, caption } => {
            let href = if let Some(existing) = img_seen.get(path) {
                existing.clone()
            } else if let Some(bytes) = doc
                .embedded_images
                .get(path)
                .cloned()
                .or_else(|| std::fs::read(path).ok())
            {
                let ext = Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_else(|| "png".to_string());
                let (ext_norm, media) = if ext == "jpg" || ext == "jpeg" {
                    ("jpg", "image/jpeg")
                } else {
                    ("png", "image/png")
                };
                let idx = img_manifest.len() + 1;
                let href = format!("images/img_{idx:03}.{ext_norm}");
                let id = format!("img{idx:03}");
                zw.add_stored(&format!("OEBPS/{href}"), &bytes);
                img_manifest.push((id, href.clone(), media.to_string()));
                img_seen.insert(path.clone(), href.clone());
                href
            } else {
                String::new()
            };

            if !href.is_empty() {
                let alt = caption.as_deref().unwrap_or("Image");
                out.push_str("<figure>");
                out.push_str(&format!("<img src=\"{}\" alt=\"{}\"/>", xml_escape(&href), xml_escape(alt)));
                if let Some(cap) = caption {
                    out.push_str(&format!("<figcaption>{}</figcaption>", xml_escape(cap)));
                }
                out.push_str("</figure>\n");
            }
        }
    }
}

fn render_list_item_xhtml(
    it: &MdListItem,
    doc: &MarkdownDoc,
    out: &mut String,
    img_seen: &mut HashMap<String, String>,
    img_manifest: &mut Vec<(String, String, String)>,
    zw: &mut ZipWriter,
) {
    out.push_str("<li>");
    if let Some(checked) = it.task_checked {
        out.push_str(if checked { "[x] " } else { "[ ] " });
    }
    out.push_str(&render_spans_xhtml(&it.spans));
    for sub in &it.sublists {
        render_block_xhtml(sub, doc, out, img_seen, img_manifest, zw);
    }
    out.push_str("</li>\n");
}

fn render_spans_xhtml(spans: &[InlineSpan]) -> String {
    let mut out = String::new();
    for sp in spans {
        match sp {
            InlineSpan::Text(t) => out.push_str(&xml_escape(t)),
            InlineSpan::Bold(t) => out.push_str(&format!("<strong>{}</strong>", xml_escape(t))),
            InlineSpan::Italic(t) => out.push_str(&format!("<em>{}</em>", xml_escape(t))),
            InlineSpan::BoldItalic(t) => out.push_str(&format!("<strong><em>{}</em></strong>", xml_escape(t))),
            InlineSpan::Strikethrough(t) => out.push_str(&format!("<s>{}</s>", xml_escape(t))),
            InlineSpan::Code(t) => out.push_str(&format!("<code>{}</code>", xml_escape(t))),
            InlineSpan::Math(tex) => {
                let readable = crate::math_render::math_to_readable_text(tex);
                out.push_str(&format!("<em>{}</em>", xml_escape(&readable)));
            }
            InlineSpan::Link { text, url } => {
                out.push_str(&format!("<a href=\"{}\">{}</a>", xml_escape(url), xml_escape(text)));
            }
        }
    }
    out
}

fn align_style(align: MdAlign) -> &'static str {
    match align {
        MdAlign::Center => " style=\"text-align:center\"",
        MdAlign::Right => " style=\"text-align:right\"",
        _ => "",
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epub::parse_epub_bytes;
    use crate::markdown::parse_markdown;

    #[test]
    fn test_epub_export_and_reader_roundtrip() {
        let md = "# Book Title\n\nIntro paragraph with **bold**, *italic*, and `code`.\n\n# Chapter Two\n\n- Item A\n- Item B\n\n| Key | Value |\n| --- | --- |\n| Alpha | 42 |\n";
        let doc = parse_markdown(md, "Book");
        let epub_bytes = export_markdown_doc_to_epub_bytes(&doc);

        // Verify mimetype is stored uncompressed near start of ZIP
        assert!(epub_bytes.windows(20).any(|w| w == b"application/epub+zip"));

        // Re-import via our EPUB Reader and verify structure
        let parsed = parse_epub_bytes(&epub_bytes, "Fallback").expect("re-parse exported epub");
        assert_eq!(parsed.title, "Book Title");
        assert!(parsed.headings.iter().any(|h| h.title == "Book Title"));
        assert!(parsed.headings.iter().any(|h| h.title == "Chapter Two"));
        assert!(parsed.blocks.iter().any(|b| matches!(b, MdBlock::List { .. })));
        assert!(parsed.blocks.iter().any(|b| matches!(b, MdBlock::Table { .. })));
    }
}
