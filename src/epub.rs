//! Pure-Rust Portable EPUB 2 & EPUB 3 Reader.
//!
//! Parses `.epub` archives directly from memory using `src/zip_mini.rs` (zero temp files,
//! zero external dependencies) into MicroPDF's shared `MarkdownDoc` AST.

use anyhow::{Result, bail};
use std::collections::HashMap;

use crate::markdown::{InlineSpan, MarkdownDoc, MdAlign, MdBlock, MdHeading, MdListItem};
use crate::zip_mini::ZipReader;

/// Parses an `.epub` archive from raw bytes into `MarkdownDoc`.
pub fn parse_epub_bytes(data: &[u8], fallback_title: &str) -> Result<MarkdownDoc> {
    let zip = ZipReader::from_bytes(data)?;
    let container_bytes = zip.read_file("META-INF/container.xml")?;
    let container_xml = String::from_utf8_lossy(&container_bytes);

    let opf_path = extract_attr_from_tag(&container_xml, "rootfile", "full-path")
        .map(|s| normalize_zip_path("", &s))
        .ok_or_else(|| anyhow::anyhow!("EPUB missing rootfile full-path in META-INF/container.xml"))?;

    let opf_bytes = zip.read_file(&opf_path)?;
    let opf_xml = String::from_utf8_lossy(&opf_bytes);
    let opf_dir = parent_dir(&opf_path);

    let title = extract_tag_text(&opf_xml, "dc:title")
        .or_else(|| extract_tag_text(&opf_xml, "title"))
        .map(|t| decode_html_entities(t.trim()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| fallback_title.to_string());

    let author = extract_tag_text(&opf_xml, "dc:creator")
        .map(|a| decode_html_entities(a.trim()))
        .filter(|a| !a.is_empty());

    // Manifest items: id -> (resolved_zip_path, media_type, properties)
    let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
    let mut nav_path: Option<String> = None;
    let mut ncx_path: Option<String> = None;

    for tag in iter_tags(&opf_xml) {
        if tag.name.eq_ignore_ascii_case("item") {
            let id = get_attr(&tag.raw, "id").unwrap_or_default();
            let href = get_attr(&tag.raw, "href").unwrap_or_default();
            let media = get_attr(&tag.raw, "media-type").unwrap_or_default();
            let props = get_attr(&tag.raw, "properties").unwrap_or_default();
            if !id.is_empty() && !href.is_empty() {
                let resolved = normalize_zip_path(&opf_dir, &percent_decode(&href));
                if props.split_whitespace().any(|p| p == "nav") {
                    nav_path = Some(resolved.clone());
                }
                if media.eq_ignore_ascii_case("application/x-dtbncx+xml") {
                    ncx_path = Some(resolved.clone());
                }
                manifest.insert(id, (resolved, media, props));
            }
        }
    }

    // Spine order
    let mut spine_hrefs: Vec<String> = Vec::new();
    for tag in iter_tags(&opf_xml) {
        if tag.name.eq_ignore_ascii_case("itemref") {
            if let Some(idref) = get_attr(&tag.raw, "idref") {
                if let Some((href, media, _)) = manifest.get(&idref) {
                    if media.contains("html") || media.contains("xml") || media.is_empty() {
                        spine_hrefs.push(href.clone());
                    }
                }
            }
        }
    }
    if spine_hrefs.is_empty() {
        bail!("EPUB spine contains no readable chapters");
    }

    // Optional TOC label lookup (nav.xhtml or toc.ncx): chapter_path -> label
    let mut toc_labels: HashMap<String, String> = HashMap::new();
    if let Some(ref np) = nav_path {
        if let Ok(nb) = zip.read_file(np) {
            let nav_xml = String::from_utf8_lossy(&nb);
            collect_nav_labels(&nav_xml, &parent_dir(np), &mut toc_labels);
        }
    } else if let Some(ref ncx) = ncx_path {
        if let Ok(nb) = zip.read_file(ncx) {
            let ncx_xml = String::from_utf8_lossy(&nb);
            collect_ncx_labels(&ncx_xml, &parent_dir(ncx), &mut toc_labels);
        }
    }

    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut headings: Vec<MdHeading> = Vec::new();
    let mut embedded_images: HashMap<String, Vec<u8>> = HashMap::new();

    if let Some(auth) = author {
        blocks.push(MdBlock::Paragraph(vec![InlineSpan::Italic(format!("By {auth}"))]));
    }

    for (chap_idx, chap_path) in spine_hrefs.iter().enumerate() {
        // Skip standalone EPUB3 nav file if it has no narrative content
        if nav_path.as_deref() == Some(chap_path.as_str()) && spine_hrefs.len() > 1 {
            continue;
        }
        let Ok(chap_bytes) = zip.read_file(chap_path) else {
            continue;
        };
        let html = String::from_utf8_lossy(&chap_bytes);
        let chap_dir = parent_dir(chap_path);

        if chap_idx > 0 && !blocks.is_empty() {
            blocks.push(MdBlock::HorizontalRule);
        }

        let start_block_idx = blocks.len();
        let before_headings = headings.len();
        parse_xhtml_chapter(&html, &chap_dir, &zip, &mut blocks, &mut headings, &mut embedded_images);

        // If chapter had no explicit <h1>-<h3>, inject TOC label as chapter heading
        if headings.len() == before_headings {
            if let Some(lbl) = toc_labels.get(chap_path) {
                let b_idx = start_block_idx;
                blocks.insert(
                    b_idx,
                    MdBlock::Heading {
                        level: 2,
                        spans: vec![InlineSpan::Text(lbl.clone())],
                    },
                );
                // Shift any headings added after b_idx (none here, but clean)
                headings.push(MdHeading {
                    level: 2,
                    title: lbl.clone(),
                    block_idx: b_idx,
                });
            }
        }
    }

    if blocks.is_empty() {
        blocks.push(MdBlock::Paragraph(vec![InlineSpan::Text(
            "(Empty EPUB document)".to_string(),
        )]));
    }

    Ok(MarkdownDoc {
        title,
        blocks,
        headings,
        embedded_images,
    })
}

// ---------------------------------------------------------------------------
// XHTML / HTML Chapter Parser
// ---------------------------------------------------------------------------

struct RawTag {
    name: String,
    raw: String,
    is_close: bool,
}

fn iter_tags(xml: &str) -> Vec<RawTag> {
    let mut out = Vec::new();
    let bytes = xml.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if xml[i..].starts_with("<!--") {
                if let Some(end) = xml[i + 4..].find("-->") {
                    i += 4 + end + 3;
                    continue;
                }
            }
            if let Some(rel_end) = xml[i + 1..].find('>') {
                let raw = &xml[i + 1..i + 1 + rel_end];
                let trimmed = raw.trim();
                if !trimmed.starts_with('?') && !trimmed.starts_with('!') {
                    let is_close = trimmed.starts_with('/');
                    let name_start = if is_close { 1 } else { 0 };
                    let name_part = trimmed[name_start..].trim_end_matches('/');
                    let name = name_part
                        .split(|c: char| c.is_whitespace())
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        out.push(RawTag {
                            name,
                            raw: trimmed.to_string(),
                            is_close,
                        });
                    }
                }
                i += 1 + rel_end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn get_attr(tag_raw: &str, attr: &str) -> Option<String> {
    let lower = tag_raw.to_ascii_lowercase();
    let target = format!("{}=", attr.to_ascii_lowercase());
    let mut search_from = 0usize;
    while let Some(pos) = lower[search_from..].find(&target) {
        let abs = search_from + pos;
        let prev_ok = abs == 0 || lower.as_bytes()[abs - 1].is_ascii_whitespace();
        if prev_ok {
            let val_start = abs + target.len();
            let rest = tag_raw[val_start..].trim_start();
            if let Some(q) = rest.chars().next().filter(|&c| c == '"' || c == '\'') {
                if let Some(end_q) = rest[1..].find(q) {
                    return Some(decode_html_entities(&rest[1..1 + end_q]));
                }
            }
        }
        search_from = abs + target.len();
    }
    None
}

fn extract_attr_from_tag(xml: &str, tag_name: &str, attr: &str) -> Option<String> {
    for t in iter_tags(xml) {
        if !t.is_close && t.name.eq_ignore_ascii_case(tag_name) {
            if let Some(v) = get_attr(&t.raw, attr) {
                return Some(v);
            }
        }
    }
    None
}

fn extract_tag_text(xml: &str, tag_name: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let open_pat = format!("<{}", tag_name.to_ascii_lowercase());
    let close_pat = format!("</{}>", tag_name.to_ascii_lowercase());
    let start = lower.find(&open_pat)?;
    let gt = lower[start..].find('>')? + start + 1;
    let end = lower[gt..].find(&close_pat)? + gt;
    Some(xml[gt..end].to_string())
}

fn collect_nav_labels(nav_xml: &str, nav_dir: &str, out: &mut HashMap<String, String>) {
    let bytes = nav_xml.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(rel_end) = nav_xml[i..].find('>') {
                let tag = &nav_xml[i + 1..i + rel_end];
                if tag.trim_start().to_ascii_lowercase().starts_with('a')
                    && tag.trim_start().as_bytes().get(1).is_some_and(|b| b.is_ascii_whitespace())
                {
                    if let Some(href) = get_attr(tag, "href") {
                        let text_start = i + rel_end + 1;
                        if let Some(close_rel) = nav_xml[text_start..].to_ascii_lowercase().find("</a>") {
                            let label = strip_all_tags(&nav_xml[text_start..text_start + close_rel]);
                            let clean_href = href.split('#').next().unwrap_or(&href);
                            let key = normalize_zip_path(nav_dir, &percent_decode(clean_href));
                            let lbl = decode_html_entities(label.trim());
                            if !key.is_empty() && !lbl.is_empty() {
                                out.entry(key).or_insert(lbl);
                            }
                            i = text_start + close_rel + 4;
                            continue;
                        }
                    }
                }
                i += rel_end + 1;
                continue;
            }
        }
        i += 1;
    }
}

fn collect_ncx_labels(ncx_xml: &str, ncx_dir: &str, out: &mut HashMap<String, String>) {
    let lower = ncx_xml.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(np) = lower[pos..].find("<navpoint") {
        let abs = pos + np;
        let end_np = lower[abs..].find("</navpoint>").map(|e| abs + e).unwrap_or(ncx_xml.len());
        let chunk = &ncx_xml[abs..end_np];
        if let (Some(txt), Some(src)) = (
            extract_tag_text(chunk, "text"),
            extract_attr_from_tag(chunk, "content", "src"),
        ) {
            let clean_src = src.split('#').next().unwrap_or(&src);
            let key = normalize_zip_path(ncx_dir, &percent_decode(clean_src));
            let lbl = decode_html_entities(txt.trim());
            if !key.is_empty() && !lbl.is_empty() {
                out.entry(key).or_insert(lbl);
            }
        }
        pos = abs + 9;
    }
}

fn strip_all_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(ch);
        }
    }
    out
}

#[derive(Default)]
struct HtmlStyle {
    bold: usize,
    italic: usize,
    strike: usize,
    code: usize,
    link_url: Option<String>,
    link_text: String,
}

impl HtmlStyle {
    fn push_text(&mut self, target: &mut Vec<InlineSpan>, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.link_url.is_some() {
            self.link_text.push_str(text);
            return;
        }
        let span = if self.code > 0 {
            InlineSpan::Code(text.to_string())
        } else if self.strike > 0 {
            InlineSpan::Strikethrough(text.to_string())
        } else if self.bold > 0 && self.italic > 0 {
            InlineSpan::BoldItalic(text.to_string())
        } else if self.bold > 0 {
            InlineSpan::Bold(text.to_string())
        } else if self.italic > 0 {
            InlineSpan::Italic(text.to_string())
        } else {
            InlineSpan::Text(text.to_string())
        };
        target.push(span);
    }

    fn flush_link(&mut self, target: &mut Vec<InlineSpan>) {
        if let Some(url) = self.link_url.take() {
            let txt = std::mem::take(&mut self.link_text);
            if !txt.trim().is_empty() {
                target.push(InlineSpan::Link { text: txt, url });
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ActiveBlock {
    None,
    Heading(u8),
    Paragraph,
    Pre,
    Blockquote,
    ListItem,
    TableCell(bool),
}

fn parse_xhtml_chapter(
    html: &str,
    chap_dir: &str,
    zip: &ZipReader<'_>,
    blocks: &mut Vec<MdBlock>,
    headings: &mut Vec<MdHeading>,
    embedded_images: &mut HashMap<String, Vec<u8>>,
) {
    let mut skip_depth = 0usize;
    let mut active = ActiveBlock::None;
    let mut spans: Vec<InlineSpan> = Vec::new();
    let mut style = HtmlStyle::default();
    let mut pre_buf = String::new();

    // List state
    let mut list_stack: Vec<(bool, Vec<MdListItem>)> = Vec::new();
    // Table state
    let mut in_table = false;
    let mut table_headers: Vec<Vec<InlineSpan>> = Vec::new();
    let mut table_rows: Vec<Vec<Vec<InlineSpan>>> = Vec::new();
    let mut cur_row: Vec<Vec<InlineSpan>> = Vec::new();
    let mut cur_row_is_header = false;

    let flush_active = |active: &mut ActiveBlock,
                        spans: &mut Vec<InlineSpan>,
                        style: &mut HtmlStyle,
                        pre_buf: &mut String,
                        list_stack: &mut Vec<(bool, Vec<MdListItem>)>,
                        cur_row: &mut Vec<Vec<InlineSpan>>,
                        cur_row_is_header: &mut bool,
                        blocks: &mut Vec<MdBlock>,
                        headings: &mut Vec<MdHeading>| {
        style.flush_link(spans);
        let taken_spans = std::mem::take(spans);
        match *active {
            ActiveBlock::Heading(lvl) => {
                if !taken_spans.is_empty() {
                    let title: String = taken_spans.iter().map(|s| s.as_plain_text()).collect();
                    let title = title.trim().to_string();
                    if !title.is_empty() {
                        let b_idx = blocks.len();
                        blocks.push(MdBlock::Heading {
                            level: lvl,
                            spans: taken_spans,
                        });
                        if lvl <= 3 {
                            headings.push(MdHeading {
                                level: lvl,
                                title,
                                block_idx: b_idx,
                            });
                        }
                    }
                }
            }
            ActiveBlock::Paragraph => {
                let has_content = taken_spans.iter().any(|s| !s.as_plain_text().trim().is_empty());
                if has_content {
                    blocks.push(MdBlock::Paragraph(taken_spans));
                }
            }
            ActiveBlock::Blockquote => {
                let has_content = taken_spans.iter().any(|s| !s.as_plain_text().trim().is_empty());
                if has_content {
                    blocks.push(MdBlock::Blockquote(vec![MdBlock::Paragraph(taken_spans)]));
                }
            }
            ActiveBlock::Pre => {
                let code = std::mem::take(pre_buf);
                if !code.trim().is_empty() {
                    blocks.push(MdBlock::CodeBlock { lang: None, code });
                }
            }
            ActiveBlock::ListItem => {
                if let Some((_, items)) = list_stack.last_mut() {
                    if !taken_spans.is_empty() {
                        items.push(MdListItem {
                            task_checked: None,
                            spans: taken_spans,
                            sublists: Vec::new(),
                        });
                    }
                }
            }
            ActiveBlock::TableCell(is_hdr) => {
                if is_hdr {
                    *cur_row_is_header = true;
                }
                cur_row.push(taken_spans);
            }
            ActiveBlock::None => {}
        }
        *active = ActiveBlock::None;
    };

    let bytes = html.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if html[i..].starts_with("<!--") {
                if let Some(end) = html[i + 4..].find("-->") {
                    i += 4 + end + 3;
                    continue;
                }
            }
            let Some(rel_end) = html[i + 1..].find('>') else {
                break;
            };
            let raw_tag = html[i + 1..i + 1 + rel_end].trim();
            i += 1 + rel_end + 1;
            if raw_tag.starts_with('?') || raw_tag.starts_with('!') {
                continue;
            }
            let is_close = raw_tag.starts_with('/');
            let self_closing = raw_tag.ends_with('/');
            let name_part = if is_close { &raw_tag[1..] } else { raw_tag }.trim_end_matches('/');
            let tag_name = name_part
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();

            if matches!(tag_name.as_str(), "head" | "script" | "style" | "title") {
                if is_close {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if !self_closing {
                    skip_depth += 1;
                }
                continue;
            }
            if skip_depth > 0 {
                continue;
            }

            if !is_close {
                match tag_name.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        let lvl = tag_name[1..].parse::<u8>().unwrap_or(2);
                        active = ActiveBlock::Heading(lvl);
                    }
                    "p" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        if list_stack.is_empty() && !in_table {
                            active = ActiveBlock::Paragraph;
                        }
                    }
                    "blockquote" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        active = ActiveBlock::Blockquote;
                    }
                    "pre" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        active = ActiveBlock::Pre;
                    }
                    "hr" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        blocks.push(MdBlock::HorizontalRule);
                    }
                    "br" => {
                        if active == ActiveBlock::Pre {
                            pre_buf.push('\n');
                        } else if active != ActiveBlock::None {
                            style.push_text(&mut spans, " ");
                        }
                    }
                    "ul" | "ol" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        list_stack.push((tag_name == "ol", Vec::new()));
                    }
                    "li" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        active = ActiveBlock::ListItem;
                    }
                    "table" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        in_table = true;
                        table_headers.clear();
                        table_rows.clear();
                    }
                    "tr" if in_table => {
                        cur_row.clear();
                        cur_row_is_header = false;
                    }
                    "th" | "td" if in_table => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        active = ActiveBlock::TableCell(tag_name == "th");
                    }
                    "img" | "image" => {
                        let src = get_attr(raw_tag, "src")
                            .or_else(|| get_attr(raw_tag, "xlink:href"))
                            .or_else(|| get_attr(raw_tag, "href"));
                        if let Some(src_val) = src {
                            let alt = get_attr(raw_tag, "alt").filter(|s| !s.trim().is_empty());
                            let zip_key = normalize_zip_path(chap_dir, &percent_decode(&src_val));
                            if let Ok(img_bytes) = zip.read_file(&zip_key) {
                                flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                                embedded_images.insert(zip_key.clone(), img_bytes);
                                blocks.push(MdBlock::Image {
                                    path: zip_key,
                                    caption: alt,
                                });
                            }
                        }
                    }
                    "strong" | "b" => style.bold += 1,
                    "em" | "i" => style.italic += 1,
                    "s" | "del" | "strike" => style.strike += 1,
                    "code" | "tt" | "kbd" => style.code += 1,
                    "a" => {
                        if let Some(href) = get_attr(raw_tag, "href") {
                            style.flush_link(&mut spans);
                            style.link_url = Some(href);
                        }
                    }
                    _ => {}
                }
            } else {
                match tag_name.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "blockquote" | "pre" | "li" | "th" | "td" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                    }
                    "ul" | "ol" => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        if let Some((ordered, items)) = list_stack.pop() {
                            if !items.is_empty() {
                                let list_blk = MdBlock::List {
                                    ordered,
                                    start: 1,
                                    items,
                                };
                                if let Some((_, parent_items)) = list_stack.last_mut() {
                                    if let Some(last_item) = parent_items.last_mut() {
                                        last_item.sublists.push(list_blk);
                                    } else {
                                        blocks.push(list_blk);
                                    }
                                } else {
                                    blocks.push(list_blk);
                                }
                            }
                        }
                    }
                    "tr" if in_table => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        if !cur_row.is_empty() {
                            if cur_row_is_header && table_headers.is_empty() {
                                table_headers = std::mem::take(&mut cur_row);
                            } else {
                                table_rows.push(std::mem::take(&mut cur_row));
                            }
                        }
                    }
                    "table" if in_table => {
                        flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
                        in_table = false;
                        if table_headers.is_empty() && !table_rows.is_empty() {
                            table_headers = table_rows.remove(0);
                        }
                        if !table_headers.is_empty() || !table_rows.is_empty() {
                            let ncols = table_headers.len().max(table_rows.iter().map(|r| r.len()).max().unwrap_or(0));
                            blocks.push(MdBlock::Table {
                                alignments: vec![MdAlign::Left; ncols],
                                headers: std::mem::take(&mut table_headers),
                                rows: std::mem::take(&mut table_rows),
                            });
                        }
                    }
                    "strong" | "b" => style.bold = style.bold.saturating_sub(1),
                    "em" | "i" => style.italic = style.italic.saturating_sub(1),
                    "s" | "del" | "strike" => style.strike = style.strike.saturating_sub(1),
                    "code" | "tt" | "kbd" => style.code = style.code.saturating_sub(1),
                    "a" => style.flush_link(&mut spans),
                    _ => {}
                }
            }
        } else {
            let next_lt = html[i..].find('<').map(|p| i + p).unwrap_or(html.len());
            let raw_txt = &html[i..next_lt];
            i = next_lt;
            if skip_depth > 0 {
                continue;
            }
            let decoded = decode_html_entities(raw_txt);
            if active == ActiveBlock::Pre {
                pre_buf.push_str(&decoded);
            } else {
                let collapsed = collapse_ws(&decoded);
                if !collapsed.is_empty() {
                    if active == ActiveBlock::None && !collapsed.trim().is_empty() {
                        active = ActiveBlock::Paragraph;
                    }
                    if active != ActiveBlock::None {
                        if !(spans.is_empty() && collapsed == " ") {
                            style.push_text(&mut spans, &collapsed);
                        }
                    }
                }
            }
        }
    }

    flush_active(&mut active, &mut spans, &mut style, &mut pre_buf, &mut list_stack, &mut cur_row, &mut cur_row_is_header, blocks, headings);
    while let Some((ordered, items)) = list_stack.pop() {
        if !items.is_empty() {
            blocks.push(MdBlock::List {
                ordered,
                start: 1,
                items,
            });
        }
    }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_ws {
                out.push(' ');
                last_ws = true;
            }
        } else {
            out.push(ch);
            last_ws = false;
        }
    }
    out
}

fn decode_html_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(semi) = s[i..].find(';') {
                if semi <= 12 {
                    let ent = &s[i + 1..i + semi];
                    let rep: Option<char> = match ent {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        "nbsp" => Some(' '),
                        "mdash" => Some('—'),
                        "ndash" => Some('–'),
                        "hellip" => Some('…'),
                        "copy" => Some('©'),
                        _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                            u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
                        }
                        _ if ent.starts_with('#') => {
                            ent[1..].parse::<u32>().ok().and_then(char::from_u32)
                        }
                        _ => None,
                    };
                    if let Some(ch) = rep {
                        out.push(ch);
                        i += semi + 1;
                        continue;
                    }
                }
            }
        }
        if let Some(ch) = s[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(pos) => path[..pos].to_string(),
        None => String::new(),
    }
}

fn normalize_zip_path(base_dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if rel.starts_with('/') || base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').filter(|p| !p.is_empty()).collect()
    };
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            seg => parts.push(seg),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_mini::ZipWriter;

    #[test]
    fn test_epub_reader_parses_chapters_toc_formatting_and_embedded_image() {
        let mut zw = ZipWriter::new();
        zw.add_stored("mimetype", b"application/epub+zip");
        zw.add_deflated(
            "META-INF/container.xml",
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        );
        zw.add_deflated(
            "OEBPS/content.opf",
            br#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Portable EPUB Test</dc:title>
    <dc:creator>MicroPDF Author</dc:creator>
  </metadata>
  <manifest>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="pic" href="img/cover.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
  </spine>
</package>"#,
        );
        let fake_png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        zw.add_stored("OEBPS/img/cover.png", &fake_png);
        zw.add_deflated(
            "OEBPS/ch1.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <h1>Chapter One</h1>
  <p>Hello <strong>bold</strong> and <em>italic</em> &amp; entities.</p>
  <ul><li>First</li><li>Second</li></ul>
  <table><tr><th>Col A</th><th>Col B</th></tr><tr><td>1</td><td>2</td></tr></table>
  <img src="img/cover.png" alt="Cover"/>
</body>
</html>"#,
        );

        let epub_bytes = zw.finish();
        let doc = parse_epub_bytes(&epub_bytes, "Fallback").expect("parse epub");
        assert_eq!(doc.title, "Portable EPUB Test");
        assert_eq!(doc.headings.len(), 1);
        assert_eq!(doc.headings[0].title, "Chapter One");
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::List { .. })));
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::Table { .. })));
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::Image { path, .. } if path == "OEBPS/img/cover.png")));
        assert_eq!(doc.embedded_images.get("OEBPS/img/cover.png"), Some(&fake_png));
    }
}
