//! Styled Word (`.docx`) Exporter for `MarkdownDoc` powered by `docx-rs`.
//!
//! Produces clean, standard OOXML `.docx` archives that open in desktop Microsoft Word
//! with zero repair warnings: styled headings (`keep_next`), inline formatting,
//! shaded monospace code blocks, left-bordered blockquotes, full-width aligned tables,
//! and bullet/numbered/task lists.

use crate::markdown::{InlineSpan, MarkdownDoc, MdAlign, MdBlock, MdListItem};
use anyhow::{Context, Result};
use docx_rs::*;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

fn align_to_docx(align: MdAlign) -> AlignmentType {
    match align {
        MdAlign::Center => AlignmentType::Center,
        MdAlign::Right => AlignmentType::Right,
        MdAlign::Left | MdAlign::None => AlignmentType::Left,
    }
}

fn add_inline_spans(mut p: Paragraph, spans: &[InlineSpan], base_size_half_pt: usize, force_bold: bool, base_color: &str) -> Paragraph {
    for sp in spans {
        match sp {
            InlineSpan::Text(t) => {
                let mut r = Run::new().add_text(t).size(base_size_half_pt).color(base_color);
                if force_bold {
                    r = r.bold();
                }
                p = p.add_run(r);
            }
            InlineSpan::Bold(t) => {
                p = p.add_run(Run::new().add_text(t).size(base_size_half_pt).color(base_color).bold());
            }
            InlineSpan::Italic(t) => {
                let mut r = Run::new().add_text(t).size(base_size_half_pt).color(base_color).italic();
                if force_bold {
                    r = r.bold();
                }
                p = p.add_run(r);
            }
            InlineSpan::BoldItalic(t) => {
                p = p.add_run(Run::new().add_text(t).size(base_size_half_pt).color(base_color).bold().italic());
            }
            InlineSpan::Strikethrough(t) => {
                p = p.add_run(Run::new().add_text(t).size(base_size_half_pt).color("64748B").strike());
            }
            InlineSpan::Code(t) => {
                let r = Run::new()
                    .add_text(t)
                    .size(base_size_half_pt.saturating_sub(2).max(16))
                    .color("9F1239")
                    .fonts(RunFonts::new().ascii("Consolas").hi_ansi("Consolas"))
                    .shading(Shading::new().shd_type(ShdType::Clear).fill("F1F5F9"));
                p = p.add_run(r);
            }
            InlineSpan::Link { text, url } => {
                let r = Run::new()
                    .add_text(text)
                    .size(base_size_half_pt)
                    .color("1D4ED8")
                    .underline("single");
                let hl = Hyperlink::new(url, HyperlinkType::External).add_run(r);
                p = p.add_hyperlink(hl);
            }
            InlineSpan::Math(tex) => {
                let readable = crate::math_render::math_to_readable_text(tex);
                let r = Run::new()
                    .add_text(readable)
                    .size(base_size_half_pt)
                    .italic()
                    .color("1E3A8A");
                p = p.add_run(r);
            }
        }
    }
    p
}

fn with_para_borders(mut p: Paragraph, borders: ParagraphBorders) -> Paragraph {
    p.property = p.property.set_borders(borders);
    p
}

fn with_para_shading(mut p: Paragraph, shading: Shading) -> Paragraph {
    p.property = p.property.shading(shading);
    p
}

fn emit_blocks_to_docx(mut docx: Docx, blocks: &[MdBlock]) -> Docx {
    for blk in blocks {
        match blk {
            MdBlock::Heading { level, spans } => {
                let size_half_pt = match level {
                    1 => 40, // 20 pt
                    2 => 32, // 16 pt
                    3 => 26, // 13 pt
                    4 => 24, // 12 pt
                    5 => 22, // 11 pt
                    _ => 20, // 10 pt
                };
                let before = if *level == 1 { 240 } else { 180 };
                let mut p = Paragraph::new()
                    .keep_next(true)
                    .line_spacing(LineSpacing::new().before(before).after(100));
                if *level <= 2 {
                    p = with_para_borders(
                        p,
                        ParagraphBorders::new().set(
                            ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                                .val(BorderType::Single)
                                .size(6)
                                .color("CBD5E1"),
                        ),
                    );
                }
                p = add_inline_spans(p, spans, size_half_pt, true, "1E293B");
                docx = docx.add_paragraph(p);
            }
            MdBlock::Paragraph(spans) => {
                let p = Paragraph::new()
                    .line_spacing(LineSpacing::new().before(40).after(140).line(276));
                let p = add_inline_spans(p, spans, 22, false, "1F2937");
                docx = docx.add_paragraph(p);
            }
            MdBlock::CodeBlock { lang, code } => {
                let cell_borders = TableCellBorders::new()
                    .set(TableCellBorder::new(TableCellBorderPosition::Top).border_type(BorderType::Single).size(4).color("CBD5E1"))
                    .set(TableCellBorder::new(TableCellBorderPosition::Bottom).border_type(BorderType::Single).size(4).color("CBD5E1"))
                    .set(TableCellBorder::new(TableCellBorderPosition::Left).border_type(BorderType::Single).size(4).color("CBD5E1"))
                    .set(TableCellBorder::new(TableCellBorderPosition::Right).border_type(BorderType::Single).size(4).color("CBD5E1"));
                let mut cell = TableCell::new()
                    .shading(Shading::new().shd_type(ShdType::Clear).fill("F8FAFC"))
                    .set_borders(cell_borders)
                    .width(9360, WidthType::Dxa);
                if let Some(l) = lang {
                    let hp = Paragraph::new()
                        .line_spacing(LineSpacing::new().before(40).after(60))
                        .add_run(Run::new().add_text(l.to_uppercase()).size(16).bold().color("64748B"));
                    cell = cell.add_paragraph(hp);
                }
                let lines: Vec<&str> = if code.is_empty() { vec![""] } else { code.lines().collect() };
                for line in lines {
                    let cp = Paragraph::new()
                        .line_spacing(LineSpacing::new().before(0).after(20))
                        .add_run(
                            Run::new()
                                .add_text(line)
                                .size(19)
                                .color("0F172A")
                                .fonts(RunFonts::new().ascii("Consolas").hi_ansi("Consolas")),
                        );
                    cell = cell.add_paragraph(cp);
                }
                let tbl = Table::new(vec![TableRow::new(vec![cell])])
                    .width(9360, WidthType::Dxa)
                    .margins(TableCellMargins::new().margin(100, 140, 100, 140));
                docx = docx.add_table(tbl);
                docx = docx.add_paragraph(Paragraph::new().line_spacing(LineSpacing::new().before(0).after(100)));
            }
            MdBlock::Blockquote(inner) => {
                for ib in inner {
                    if let MdBlock::Paragraph(spans) = ib {
                        let mut p = Paragraph::new()
                            .indent(Some(360), None, None, None)
                            .line_spacing(LineSpacing::new().before(60).after(100));
                        p = with_para_borders(
                            p,
                            ParagraphBorders::new().set(
                                ParagraphBorder::new(ParagraphBorderPosition::Left)
                                    .val(BorderType::Single)
                                    .size(18)
                                    .color("3B82F6"),
                            ),
                        );
                        p = with_para_shading(p, Shading::new().shd_type(ShdType::Clear).fill("F8FAFC"));
                        let p = add_inline_spans(p, spans, 21, false, "475569");
                        docx = docx.add_paragraph(p);
                    } else {
                        docx = emit_blocks_to_docx(docx, std::slice::from_ref(ib));
                    }
                }
            }
            MdBlock::Table { headers, alignments, rows } => {
                let ncols = headers.len().max(1);
                let col_w = 9360 / ncols;
                let mut tbl_rows = Vec::with_capacity(1 + rows.len());

                let mut hdr_cells = Vec::with_capacity(ncols);
                for c in 0..ncols {
                    let sp = headers.get(c).map(|v| v.as_slice()).unwrap_or(&[]);
                    let align = align_to_docx(alignments.get(c).copied().unwrap_or(MdAlign::Left));
                    let p = add_inline_spans(Paragraph::new().align(align), sp, 20, true, "1E293B");
                    hdr_cells.push(
                        TableCell::new()
                            .add_paragraph(p)
                            .shading(Shading::new().shd_type(ShdType::Clear).fill("EEF2F7"))
                            .width(col_w, WidthType::Dxa),
                    );
                }
                tbl_rows.push(TableRow::new(hdr_cells).cant_split());

                for (ri, row) in rows.iter().enumerate() {
                    let mut r_cells = Vec::with_capacity(ncols);
                    for c in 0..ncols {
                        let sp = row.get(c).map(|v| v.as_slice()).unwrap_or(&[]);
                        let align = align_to_docx(alignments.get(c).copied().unwrap_or(MdAlign::Left));
                        let p = add_inline_spans(Paragraph::new().align(align), sp, 20, false, "1F2937");
                        let mut cell = TableCell::new().add_paragraph(p).width(col_w, WidthType::Dxa);
                        if ri % 2 == 1 {
                            cell = cell.shading(Shading::new().shd_type(ShdType::Clear).fill("F8FAFC"));
                        }
                        r_cells.push(cell);
                    }
                    tbl_rows.push(TableRow::new(r_cells).cant_split());
                }

                let borders = TableBorders::new()
                    .set(TableBorder::new(TableBorderPosition::Top).border_type(BorderType::Single).size(4).color("CBD5E1"))
                    .set(TableBorder::new(TableBorderPosition::Bottom).border_type(BorderType::Single).size(4).color("CBD5E1"))
                    .set(TableBorder::new(TableBorderPosition::Left).border_type(BorderType::Single).size(4).color("CBD5E1"))
                    .set(TableBorder::new(TableBorderPosition::Right).border_type(BorderType::Single).size(4).color("CBD5E1"))
                    .set(TableBorder::new(TableBorderPosition::InsideH).border_type(BorderType::Single).size(4).color("E2E8F0"))
                    .set(TableBorder::new(TableBorderPosition::InsideV).border_type(BorderType::Single).size(4).color("E2E8F0"));

                let tbl = Table::new(tbl_rows)
                    .width(9360, WidthType::Dxa)
                    .set_borders(borders)
                    .margins(TableCellMargins::new().margin(80, 110, 80, 110));
                docx = docx.add_table(tbl);
                docx = docx.add_paragraph(Paragraph::new().line_spacing(LineSpacing::new().before(0).after(100)));
            }
            MdBlock::List { ordered, start, items } => {
                docx = emit_list_to_docx(docx, items, *ordered, *start, 0);
                docx = docx.add_paragraph(Paragraph::new().line_spacing(LineSpacing::new().before(0).after(60)));
            }
            MdBlock::HorizontalRule => {
                let p = with_para_borders(
                    Paragraph::new().line_spacing(LineSpacing::new().before(80).after(120)),
                    ParagraphBorders::new().set(
                        ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                            .val(BorderType::Single)
                            .size(6)
                            .color("CBD5E1"),
                    ),
                );
                docx = docx.add_paragraph(p);
            }
            MdBlock::PageBreak => {
                docx = docx.add_paragraph(Paragraph::new().page_break_before(true));
            }
            MdBlock::MathBlock(tex) => {
                let readable = crate::math_render::math_to_readable_text(tex);
                let mut p = Paragraph::new()
                    .align(AlignmentType::Center)
                    .line_spacing(LineSpacing::new().before(80).after(120));
                p = with_para_shading(p, Shading::new().shd_type(ShdType::Clear).fill("F8FAFC"));
                p = p.add_run(Run::new().add_text(readable).size(24).italic().color("1E3A8A"));
                docx = docx.add_paragraph(p);
            }
            MdBlock::Image { path, .. } => {
                let lbl = format!("[Image: {}]", std::path::Path::new(path).file_name().and_then(|s| s.to_str()).unwrap_or(path));
                let p = Paragraph::new()
                    .line_spacing(LineSpacing::new().before(40).after(100))
                    .add_run(Run::new().add_text(lbl).size(20).italic().color("64748B"));
                docx = docx.add_paragraph(p);
            }
        }
    }
    docx
}

fn emit_list_to_docx(mut docx: Docx, items: &[MdListItem], ordered: bool, start: u64, depth: usize) -> Docx {
    let indent_twips = ((depth + 1) * 360) as i32;
    for (i, item) in items.iter().enumerate() {
        let marker = if let Some(checked) = item.task_checked {
            if checked { "☑ " } else { "☐ " }.to_string()
        } else if ordered {
            format!("{}. ", start + i as u64)
        } else if depth % 2 == 0 {
            "• ".to_string()
        } else {
            "◦ ".to_string()
        };
        let mut p = Paragraph::new()
            .indent(Some(indent_twips), None, None, None)
            .line_spacing(LineSpacing::new().before(20).after(50));
        p = p.add_run(Run::new().add_text(marker).size(22).bold().color("3B82F6"));
        p = add_inline_spans(p, &item.spans, 22, false, "1F2937");
        docx = docx.add_paragraph(p);

        for sub in &item.sublists {
            if let MdBlock::List { ordered: so, start: ss, items: si } = sub {
                docx = emit_list_to_docx(docx, si, *so, *ss, depth + 1);
            }
        }
    }
    docx
}

pub fn export_markdown_to_docx(md: &MarkdownDoc, output_path: &Path) -> Result<()> {
    let docx = Docx::new()
        .page_size(11906, 16838) // A4 in twips
        .page_margin(PageMargin::new().top(1080).bottom(1080).left(1080).right(1080)); // 0.75 in
    let docx = emit_blocks_to_docx(docx, &md.blocks);
    let file = File::create(output_path)
        .with_context(|| format!("Failed to create Word document {:?}", output_path))?;
    let writer = BufWriter::new(file);
    docx.build()
        .pack(writer)
        .with_context(|| format!("Failed to pack .docx archive {:?}", output_path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse_markdown;
    use flate2::read::DeflateDecoder;
    use std::collections::HashMap;
    use std::io::Read;

    fn read_zip_entries(bytes: &[u8]) -> HashMap<String, Vec<u8>> {
        let mut out = HashMap::new();
        let mut i = 0usize;
        while i + 30 <= bytes.len() {
            if &bytes[i..i + 4] != b"PK\x03\x04" {
                i += 1;
                continue;
            }
            let method = u16::from_le_bytes([bytes[i + 8], bytes[i + 9]]);
            let comp_sz = u32::from_le_bytes([bytes[i + 18], bytes[i + 19], bytes[i + 20], bytes[i + 21]]) as usize;
            let name_len = u16::from_le_bytes([bytes[i + 26], bytes[i + 27]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
            let name_start = i + 30;
            let data_start = name_start + name_len + extra_len;
            if data_start + comp_sz > bytes.len() {
                break;
            }
            let name = String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).to_string();
            let raw = &bytes[data_start..data_start + comp_sz];
            let data = if method == 0 {
                raw.to_vec()
            } else if method == 8 {
                let mut dec = DeflateDecoder::new(raw);
                let mut buf = Vec::new();
                let _ = dec.read_to_end(&mut buf);
                buf
            } else {
                Vec::new()
            };
            out.insert(name, data);
            i = data_start + comp_sz;
        }
        out
    }

    #[test]
    fn test_export_markdown_to_docx_valid_ooxml_zip() {
        let md = "\
# Project Summary

Body text with **bold**, *italic*, ~~strike~~, `inline_code`, and [MicroPDF](https://example.com).

## Metrics Table

| Metric | Value |
| :--- | ---: |
| Speed | Fast |
| RAM | Low |

- [x] Shipped Markdown Reader
- [ ] Verify Word Export

> Clean OOXML output via docx-rs.

```rust
let ok = true;
```
";
        let doc = parse_markdown(md, "summary.md");
        let tmp = std::env::temp_dir().join("micropdf_test_md_export.docx");
        let _ = std::fs::remove_file(&tmp);
        export_markdown_to_docx(&doc, &tmp).expect("export_markdown_to_docx should succeed");

        let bytes = std::fs::read(&tmp).expect("exported docx should exist");
        let entries = read_zip_entries(&bytes);
        assert!(entries.contains_key("[Content_Types].xml"), "[Content_Types].xml missing");
        assert!(entries.contains_key("word/styles.xml"), "word/styles.xml missing");
        let doc_bytes = entries.get("word/document.xml").expect("word/document.xml missing");
        let doc_xml = String::from_utf8_lossy(doc_bytes);
        assert!(doc_xml.contains("Project Summary"), "heading missing in document.xml");
        assert!(doc_xml.contains("Metrics Table"), "h2 missing in document.xml");
        assert!(doc_xml.contains("Speed"), "table cell missing in document.xml");

        let _ = std::fs::remove_file(&tmp);
    }
}
