//! Pure-Rust Portable EPUB 2 & EPUB 3 Reader.
//!
//! Parses `.epub` archives directly from memory using `src/zip_mini.rs` (zero temp files)
//! and `roxmltree` (namespace-aware XML/XHTML DOM parsing with tolerant fallback) into
//! MicroPDF's shared `MarkdownDoc` AST + structured `BookMetadata`.

use anyhow::{Result, bail};
use std::collections::HashMap;

use crate::markdown::{
    BookMetadata, InlineSpan, MarkdownDoc, MdAlign, MdBlock, MdHeading, MdListItem, TocEntry,
};
use crate::zip_mini::ZipReader;

#[derive(Clone, Debug)]
struct RawTocNode {
    title: String,
    level: u8,
    zip_path: String,
    fragment: Option<String>,
    children: Vec<RawTocNode>,
}

/// Parses an `.epub` archive from raw bytes into `MarkdownDoc` with `BookMetadata`.
pub fn parse_epub_bytes(data: &[u8], fallback_title: &str) -> Result<MarkdownDoc> {
    let zip = ZipReader::from_bytes(data)?;
    let container_bytes = zip.read_file("META-INF/container.xml")?;
    let container_xml = String::from_utf8_lossy(&container_bytes);

    let opf_path = parse_container_opf_path(&container_xml)
        .or_else(|| {
            extract_attr_from_tag(&container_xml, "rootfile", "full-path")
                .map(|s| normalize_zip_path("", &s))
        })
        .ok_or_else(|| anyhow::anyhow!("EPUB missing rootfile full-path in META-INF/container.xml"))?;

    let opf_bytes = zip.read_file(&opf_path)?;
    let opf_xml = String::from_utf8_lossy(&opf_bytes);
    let opf_dir = parent_dir(&opf_path);

    let mut title = fallback_title.to_string();
    let mut author: Option<String> = None;
    let mut publisher: Option<String> = None;
    let mut language: Option<String> = None;
    let mut description: Option<String> = None;
    let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
    let mut nav_path: Option<String> = None;
    let mut ncx_path: Option<String> = None;
    let mut cover_path: Option<String> = None;
    let mut spine_hrefs: Vec<String> = Vec::new();
    let mut guide_toc_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut non_linear_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    if !parse_opf_with_roxmltree(
        &opf_xml,
        &opf_dir,
        fallback_title,
        &mut title,
        &mut author,
        &mut publisher,
        &mut language,
        &mut description,
        &mut manifest,
        &mut nav_path,
        &mut ncx_path,
        &mut cover_path,
        &mut spine_hrefs,
        &mut guide_toc_paths,
        &mut non_linear_paths,
    ) {
        // Tolerant fallback if .opf is malformed XML
        title = extract_tag_text(&opf_xml, "dc:title")
            .or_else(|| extract_tag_text(&opf_xml, "title"))
            .map(|t| decode_html_entities(t.trim()))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| fallback_title.to_string());
        author = extract_tag_text(&opf_xml, "dc:creator")
            .map(|a| decode_html_entities(a.trim()))
            .filter(|a| !a.is_empty());
        publisher = extract_tag_text(&opf_xml, "dc:publisher")
            .map(|p| decode_html_entities(p.trim()))
            .filter(|p| !p.is_empty());
        language = extract_tag_text(&opf_xml, "dc:language")
            .map(|l| decode_html_entities(l.trim()))
            .filter(|l| !l.is_empty());
        description = extract_tag_text(&opf_xml, "dc:description")
            .map(|d| collapse_ws(&decode_html_entities(&strip_all_tags(&d))).trim().to_string())
            .filter(|d| !d.is_empty());

        let mut epub2_cover_id: Option<String> = None;
        for tag in iter_tags(&opf_xml) {
            if tag.name.eq_ignore_ascii_case("meta") {
                if get_attr(&tag.raw, "name").is_some_and(|n| n.eq_ignore_ascii_case("cover")) {
                    if let Some(cid) = get_attr(&tag.raw, "content") {
                        if !cid.trim().is_empty() {
                            epub2_cover_id = Some(cid);
                        }
                    }
                }
            } else if tag.name.eq_ignore_ascii_case("item") {
                let id = get_attr(&tag.raw, "id").unwrap_or_default();
                let href = get_attr(&tag.raw, "href").unwrap_or_default();
                let media = get_attr(&tag.raw, "media-type").unwrap_or_default();
                let props = get_attr(&tag.raw, "properties").unwrap_or_default();
                if !id.is_empty() && !href.is_empty() {
                    let resolved = normalize_zip_path(&opf_dir, &percent_decode(&href));
                    if props.split_whitespace().any(|p| p == "nav") {
                        nav_path = Some(resolved.clone());
                    }
                    if props.split_whitespace().any(|p| p == "cover-image") {
                        cover_path = Some(resolved.clone());
                    }
                    if media.eq_ignore_ascii_case("application/x-dtbncx+xml") {
                        ncx_path = Some(resolved.clone());
                    }
                    manifest.insert(id, (resolved, media, props));
                }
            } else if tag.name.eq_ignore_ascii_case("reference") {
                let rtype = get_attr(&tag.raw, "type").unwrap_or_default();
                if rtype.eq_ignore_ascii_case("toc") {
                    if let Some(href) = get_attr(&tag.raw, "href") {
                        let file_part = href.split('#').next().unwrap_or(&href);
                        let resolved = normalize_zip_path(&opf_dir, &percent_decode(file_part));
                        if !resolved.is_empty() {
                            guide_toc_paths.insert(resolved);
                        }
                    }
                }
            }
        }
        if cover_path.is_none() {
            if let Some(cid) = epub2_cover_id {
                if let Some((href, _, _)) = manifest.get(&cid) {
                    cover_path = Some(href.clone());
                }
            }
        }
        for tag in iter_tags(&opf_xml) {
            if tag.name.eq_ignore_ascii_case("itemref") {
                if let Some(idref) = get_attr(&tag.raw, "idref") {
                    if let Some((href, media, _)) = manifest.get(&idref) {
                        if media.contains("html") || media.contains("xml") || media.is_empty() {
                            if get_attr(&tag.raw, "linear")
                                .is_some_and(|v| v.eq_ignore_ascii_case("no"))
                            {
                                non_linear_paths.insert(href.clone());
                            }
                            spine_hrefs.push(href.clone());
                        }
                    }
                }
            }
        }
    }

    if spine_hrefs.is_empty() {
        bail!("EPUB spine contains no readable chapters");
    }

    // Parse nested TOC (nav.xhtml preferred, toc.ncx fallback)
    let mut raw_toc: Vec<RawTocNode> = Vec::new();
    let mut toc_labels: HashMap<String, String> = HashMap::new();
    if let Some(ref np) = nav_path {
        if let Ok(nb) = zip.read_file(np) {
            let nav_xml = String::from_utf8_lossy(&nb);
            let nav_dir = parent_dir(np);
            raw_toc = parse_nav_toc_roxmltree(&nav_xml, &nav_dir);
            if raw_toc.is_empty() {
                collect_nav_labels(&nav_xml, &nav_dir, &mut toc_labels);
            } else {
                collect_raw_toc_labels(&raw_toc, &mut toc_labels);
            }
        }
    }
    if raw_toc.is_empty() {
        if let Some(ref ncx) = ncx_path {
            if let Ok(nb) = zip.read_file(ncx) {
                let ncx_xml = String::from_utf8_lossy(&nb);
                let ncx_dir = parent_dir(ncx);
                raw_toc = parse_ncx_toc_roxmltree(&ncx_xml, &ncx_dir);
                if raw_toc.is_empty() {
                    collect_ncx_labels(&ncx_xml, &ncx_dir, &mut toc_labels);
                } else {
                    collect_raw_toc_labels(&raw_toc, &mut toc_labels);
                }
            }
        }
    }

    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut headings: Vec<MdHeading> = Vec::new();
    let mut embedded_images: HashMap<String, Vec<u8>> = HashMap::new();
    let mut chapter_start_block: HashMap<String, usize> = HashMap::new();
    let mut anchor_blocks: HashMap<(String, String), usize> = HashMap::new();
    let mut skipped_toc_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Keep cover separate in embedded_images + BookMetadata.cover_image_key (not injected as a body block)
    let mut cover_image_key: Option<String> = None;
    if let Some(ref cp) = cover_path {
        if let Ok(cbytes) = zip.read_file(cp) {
            embedded_images.insert(cp.clone(), cbytes);
            cover_image_key = Some(cp.clone());
        }
    }

    for chap_path in &spine_hrefs {
        // Skip standalone EPUB3 nav file or OPF <guide type="toc"> file from the book body
        if spine_hrefs.len() > 1
            && (nav_path.as_deref() == Some(chap_path.as_str())
                || guide_toc_paths.contains(chap_path.as_str()))
        {
            if raw_toc.is_empty() {
                if let Ok(tb) = zip.read_file(chap_path) {
                    let thtml = String::from_utf8_lossy(&tb);
                    let tdir = parent_dir(chap_path);
                    let extracted = extract_toc_from_html_links(&thtml, &tdir);
                    if !extracted.is_empty() {
                        collect_raw_toc_labels(&extracted, &mut toc_labels);
                        raw_toc = extracted;
                    }
                }
            }
            skipped_toc_paths.insert(chap_path.clone());
            continue;
        }
        let Ok(chap_bytes) = zip.read_file(chap_path) else {
            continue;
        };
        let html = String::from_utf8_lossy(&chap_bytes);
        let chap_dir = parent_dir(chap_path);

        // Parse into temporary chapter buffers first so we can detect & omit standalone HTML TOC
        // pages before inserting a PageBreak or polluting the book body.
        let mut chap_blocks: Vec<MdBlock> = Vec::new();
        let mut chap_headings: Vec<MdHeading> = Vec::new();
        let mut chap_anchors: HashMap<(String, String), usize> = HashMap::new();

        let parsed_dom = parse_xhtml_chapter_roxmltree(
            &html,
            chap_path,
            &chap_dir,
            &zip,
            &mut chap_blocks,
            &mut chap_headings,
            &mut embedded_images,
            &mut chap_anchors,
        );
        if !parsed_dom {
            parse_xhtml_chapter(
                &html,
                &chap_dir,
                &zip,
                &mut chap_blocks,
                &mut chap_headings,
                &mut embedded_images,
            );
        }

        if spine_hrefs.len() > 1
            && is_standalone_toc_chapter(
                chap_path,
                &chap_blocks,
                non_linear_paths.contains(chap_path.as_str()),
            )
        {
            if raw_toc.is_empty() {
                let extracted = extract_toc_from_html_links(&html, &chap_dir);
                if !extracted.is_empty() {
                    collect_raw_toc_labels(&extracted, &mut toc_labels);
                    raw_toc = extracted;
                }
            }
            skipped_toc_paths.insert(chap_path.clone());
            continue;
        }

        if chap_blocks.is_empty() {
            continue;
        }

        // Skip redundant standalone cover-wrapper spine item when the exact same cover image
        // is already displayed on the full-page Cover (prevents duplicate cover on Page 1).
        if blocks.is_empty() && spine_hrefs.len() > 1 {
            if let Some(ref ck) = cover_image_key {
                let is_only_cover_img = chap_blocks.len() == 1
                    && matches!(&chap_blocks[0], MdBlock::Image { path, .. } if path == ck);
                if is_only_cover_img {
                    continue;
                }
            }
        }

        // Start every EPUB chapter on a clean new page (regardless of font or size)
        if !blocks.is_empty() {
            blocks.push(MdBlock::PageBreak);
        }

        let start_block_idx = blocks.len();
        chapter_start_block
            .entry(chap_path.clone())
            .or_insert(start_block_idx);

        // If chapter had no explicit <h1>-<h3>, inject TOC label as chapter heading
        let mut injected_heading = false;
        if chap_headings.is_empty() {
            if let Some(lbl) = toc_labels.get(chap_path) {
                blocks.push(MdBlock::Heading {
                    level: 2,
                    spans: vec![InlineSpan::Text(lbl.clone())],
                });
                headings.push(MdHeading {
                    level: 2,
                    title: lbl.clone(),
                    block_idx: start_block_idx,
                });
                injected_heading = true;
            }
        }

        let block_offset = blocks.len();
        blocks.extend(chap_blocks);
        for mut h in chap_headings {
            h.block_idx += block_offset;
            headings.push(h);
        }
        for (k, rel_idx) in chap_anchors {
            let abs_idx = if injected_heading && rel_idx == 0 {
                start_block_idx
            } else {
                block_offset + rel_idx
            };
            anchor_blocks.entry(k).or_insert(abs_idx);
        }
    }

    if blocks.is_empty() {
        blocks.push(MdBlock::Paragraph(vec![InlineSpan::Text(
            "(Empty EPUB document)".to_string(),
        )]));
    }

    if !skipped_toc_paths.is_empty() {
        raw_toc = filter_skipped_toc_nodes(raw_toc, &skipped_toc_paths);
    }

    let mut toc = resolve_raw_toc(&raw_toc, &chapter_start_block, &anchor_blocks, blocks.len());
    if toc.is_empty() {
        toc = build_toc_from_headings(&headings);
    }

    Ok(MarkdownDoc {
        title: title.clone(),
        blocks,
        headings,
        embedded_images,
        book_meta: Some(BookMetadata {
            title,
            author,
            publisher,
            language,
            description,
            cover_image_key,
            toc,
        }),
    })
}

fn parse_container_opf_path(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    for node in doc.descendants() {
        if node.is_element() && node.tag_name().name().eq_ignore_ascii_case("rootfile") {
            if let Some(fp) = node.attribute("full-path") {
                let norm = normalize_zip_path("", fp);
                if !norm.is_empty() {
                    return Some(norm);
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn parse_opf_with_roxmltree(
    opf_xml: &str,
    opf_dir: &str,
    fallback_title: &str,
    title: &mut String,
    author: &mut Option<String>,
    publisher: &mut Option<String>,
    language: &mut Option<String>,
    description: &mut Option<String>,
    manifest: &mut HashMap<String, (String, String, String)>,
    nav_path: &mut Option<String>,
    ncx_path: &mut Option<String>,
    cover_path: &mut Option<String>,
    spine_hrefs: &mut Vec<String>,
    guide_toc_paths: &mut std::collections::HashSet<String>,
    non_linear_paths: &mut std::collections::HashSet<String>,
) -> bool {
    let cleaned = prepare_xml_for_roxmltree(opf_xml);
    let Ok(doc) = roxmltree::Document::parse(&cleaned) else {
        return false;
    };

    let mut parsed_title: Option<String> = None;
    let mut epub2_cover_id: Option<String> = None;
    let mut spine_idrefs: Vec<(String, bool)> = Vec::new();

    for node in doc.descendants() {
        if !node.is_element() {
            continue;
        }
        let local = node.tag_name().name();
        let ns = node.tag_name().namespace().unwrap_or("");
        let is_dc = ns.contains("purl.org/dc");

        if (is_dc || local.eq_ignore_ascii_case("title")) && local.eq_ignore_ascii_case("title") {
            let txt = collect_node_text(node).trim().to_string();
            if !txt.is_empty() && parsed_title.is_none() {
                parsed_title = Some(txt);
            }
        } else if (is_dc || local.eq_ignore_ascii_case("creator"))
            && local.eq_ignore_ascii_case("creator")
        {
            let txt = collect_node_text(node).trim().to_string();
            if !txt.is_empty() && author.is_none() {
                *author = Some(txt);
            }
        } else if (is_dc || local.eq_ignore_ascii_case("publisher"))
            && local.eq_ignore_ascii_case("publisher")
        {
            let txt = collect_node_text(node).trim().to_string();
            if !txt.is_empty() && publisher.is_none() {
                *publisher = Some(txt);
            }
        } else if (is_dc || local.eq_ignore_ascii_case("language"))
            && local.eq_ignore_ascii_case("language")
        {
            let txt = collect_node_text(node).trim().to_string();
            if !txt.is_empty() && language.is_none() {
                *language = Some(txt);
            }
        } else if (is_dc || local.eq_ignore_ascii_case("description"))
            && local.eq_ignore_ascii_case("description")
        {
            let raw_desc = collect_node_text(node);
            let txt = collapse_ws(&strip_all_tags(&raw_desc)).trim().to_string();
            if !txt.is_empty() && description.is_none() {
                *description = Some(txt);
            }
        } else if local.eq_ignore_ascii_case("meta") {
            if node
                .attribute("name")
                .is_some_and(|n| n.eq_ignore_ascii_case("cover"))
            {
                if let Some(cid) = node.attribute("content") {
                    if !cid.trim().is_empty() {
                        epub2_cover_id = Some(cid.trim().to_string());
                    }
                }
            }
        } else if local.eq_ignore_ascii_case("item") {
            let id = node.attribute("id").unwrap_or("");
            let href = node.attribute("href").unwrap_or("");
            let media = node.attribute("media-type").unwrap_or("");
            let props = node.attribute("properties").unwrap_or("");
            if !id.is_empty() && !href.is_empty() {
                let resolved = normalize_zip_path(opf_dir, &percent_decode(href));
                if props.split_whitespace().any(|p| p == "nav") {
                    *nav_path = Some(resolved.clone());
                }
                if props.split_whitespace().any(|p| p == "cover-image") {
                    *cover_path = Some(resolved.clone());
                }
                if media.eq_ignore_ascii_case("application/x-dtbncx+xml") {
                    *ncx_path = Some(resolved.clone());
                }
                manifest.insert(
                    id.to_string(),
                    (resolved, media.to_string(), props.to_string()),
                );
            }
        } else if local.eq_ignore_ascii_case("reference") {
            if node
                .attribute("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("toc"))
            {
                if let Some(href) = node.attribute("href") {
                    let file_part = href.split('#').next().unwrap_or(href);
                    let resolved = normalize_zip_path(opf_dir, &percent_decode(file_part));
                    if !resolved.is_empty() {
                        guide_toc_paths.insert(resolved);
                    }
                }
            }
        } else if local.eq_ignore_ascii_case("itemref") {
            if let Some(idref) = node.attribute("idref") {
                let non_linear = node
                    .attribute("linear")
                    .is_some_and(|v| v.eq_ignore_ascii_case("no"));
                spine_idrefs.push((idref.to_string(), non_linear));
            }
        }
    }

    if cover_path.is_none() {
        if let Some(cid) = epub2_cover_id {
            if let Some((href, _, _)) = manifest.get(&cid) {
                *cover_path = Some(href.clone());
            }
        }
    }

    for (idref, non_linear) in spine_idrefs {
        if let Some((href, media, _)) = manifest.get(&idref) {
            if media.contains("html") || media.contains("xml") || media.is_empty() {
                if non_linear {
                    non_linear_paths.insert(href.clone());
                }
                spine_hrefs.push(href.clone());
            }
        }
    }

    *title = parsed_title.unwrap_or_else(|| fallback_title.to_string());
    true
}

fn is_toc_heading_title(raw: &str) -> bool {
    let norm = raw
        .trim()
        .to_ascii_lowercase()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != ' ')
        .to_string();
    matches!(
        norm.as_str(),
        "contents"
            | "table of contents"
            | "toc"
            | "brief contents"
            | "detailed contents"
            | "short contents"
            | "summary of contents"
            | "index of contents"
    )
}

fn is_toc_filename(zip_path: &str) -> bool {
    let fname = zip_path
        .rsplit('/')
        .next()
        .unwrap_or(zip_path)
        .to_ascii_lowercase();
    let stem = fname
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(&fname);
    matches!(
        stem,
        "toc" | "nav" | "contents" | "tableofcontents" | "table-of-contents" | "table_of_contents"
    ) || stem.ends_with("_toc")
        || stem.ends_with("-toc")
        || stem.starts_with("toc_")
        || stem.starts_with("toc-")
}

/// Detects dedicated HTML Table-of-Contents spine chapters so they live solely in the sidebar
/// Contents tree and never appear as an inline link-list attachment inside the book body.
fn is_standalone_toc_chapter(chap_path: &str, blocks: &[MdBlock], is_non_linear: bool) -> bool {
    if blocks.is_empty() {
        return false;
    }
    let mut heading_is_toc = false;
    let mut total_items = 0usize;
    let mut link_items = 0usize;
    let mut has_long_prose = false;

    fn inspect_list(items: &[MdListItem], total: &mut usize, links: &mut usize, long_prose: &mut bool) {
        for it in items {
            *total += 1;
            let text_len: usize = it.spans.iter().map(|s| s.as_plain_text().len()).sum();
            let has_link = it.spans.iter().any(|s| matches!(s, InlineSpan::Link { .. }));
            if has_link {
                *links += 1;
            }
            if text_len > 280 && !has_link {
                *long_prose = true;
            }
            for sub in &it.sublists {
                if let MdBlock::List { items: sub_items, .. } = sub {
                    inspect_list(sub_items, total, links, long_prose);
                }
            }
        }
    }

    for (idx, b) in blocks.iter().enumerate() {
        match b {
            MdBlock::Heading { spans, .. } => {
                let txt: String = spans.iter().map(|s| s.as_plain_text()).collect();
                if idx <= 1 && is_toc_heading_title(&txt) {
                    heading_is_toc = true;
                }
            }
            MdBlock::Paragraph(spans) => {
                let txt: String = spans.iter().map(|s| s.as_plain_text()).collect();
                let trimmed = txt.trim();
                if idx == 0 && is_toc_heading_title(trimmed) {
                    heading_is_toc = true;
                    continue;
                }
                if trimmed.is_empty() {
                    continue;
                }
                total_items += 1;
                let has_link = spans.iter().any(|s| matches!(s, InlineSpan::Link { .. }));
                if has_link {
                    link_items += 1;
                } else if trimmed.len() > 260 {
                    has_long_prose = true;
                }
            }
            MdBlock::List { items, .. } => {
                inspect_list(items, &mut total_items, &mut link_items, &mut has_long_prose);
            }
            MdBlock::Table { rows, .. } => {
                for row in rows {
                    total_items += 1;
                    let has_link = row
                        .iter()
                        .any(|cell| cell.iter().any(|s| matches!(s, InlineSpan::Link { .. })));
                    if has_link {
                        link_items += 1;
                    }
                }
            }
            MdBlock::CodeBlock { .. } | MdBlock::MathBlock(_) | MdBlock::Image { .. } => {
                has_long_prose = true;
            }
            _ => {}
        }
    }

    if has_long_prose {
        return false;
    }
    let name_is_toc = is_toc_filename(chap_path);
    if (name_is_toc || heading_is_toc) && link_items >= 2 && link_items * 100 >= total_items.max(1) * 55 {
        return true;
    }
    if is_non_linear && (name_is_toc || heading_is_toc) && total_items <= 40 {
        return true;
    }
    // Even without a standard filename, if a chapter's heading is explicitly "Table of Contents" /
    // "Contents" and it has short chapter-like lines with zero prose paragraphs, treat as TOC.
    if heading_is_toc && total_items >= 3 && link_items >= 2 && link_items * 100 >= total_items * 50 {
        return true;
    }
    false
}

/// Fallback TOC extractor from an HTML TOC page when neither `nav.xhtml` nor `toc.ncx` exists.
fn extract_toc_from_html_links(html: &str, chap_dir: &str) -> Vec<RawTocNode> {
    let cleaned = prepare_xml_for_roxmltree(html);
    let Ok(doc) = roxmltree::Document::parse(&cleaned) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in doc.descendants() {
        if node.is_element() && node.tag_name().name().eq_ignore_ascii_case("a") {
            let Some(raw_href) = node.attribute("href") else {
                continue;
            };
            let trimmed_href = raw_href.trim();
            if trimmed_href.is_empty()
                || trimmed_href.starts_with("http://")
                || trimmed_href.starts_with("https://")
                || trimmed_href.starts_with("mailto:")
            {
                continue;
            }
            let title = collapse_ws(&collect_node_text(node)).trim().to_string();
            if title.is_empty() || is_toc_heading_title(&title) {
                continue;
            }
            let mut parts = trimmed_href.splitn(2, '#');
            let file_part = parts.next().unwrap_or("");
            let frag = parts
                .next()
                .map(|f| percent_decode(f))
                .filter(|f| !f.is_empty());
            let zip_path = normalize_zip_path(chap_dir, &percent_decode(file_part));
            if !zip_path.is_empty() {
                out.push(RawTocNode {
                    title,
                    level: 0,
                    zip_path,
                    fragment: frag,
                    children: Vec::new(),
                });
            }
        }
    }
    out
}

fn filter_skipped_toc_nodes(
    nodes: Vec<RawTocNode>,
    skipped: &std::collections::HashSet<String>,
) -> Vec<RawTocNode> {
    let mut out = Vec::with_capacity(nodes.len());
    for mut n in nodes {
        n.children = filter_skipped_toc_nodes(n.children, skipped);
        if skipped.contains(&n.zip_path) && n.children.is_empty() {
            continue;
        }
        out.push(n);
    }
    out
}

fn collect_node_text(node: roxmltree::Node<'_, '_>) -> String {
    let mut out = String::new();
    for desc in node.descendants() {
        if desc.is_text() {
            if let Some(t) = desc.text() {
                out.push_str(t);
            }
        }
    }
    out
}

fn parse_nav_toc_roxmltree(nav_xml: &str, nav_dir: &str) -> Vec<RawTocNode> {
    let cleaned = prepare_xml_for_roxmltree(nav_xml);
    let Ok(doc) = roxmltree::Document::parse(&cleaned) else {
        return Vec::new();
    };
    let mut best_nav: Option<roxmltree::Node<'_, '_>> = None;
    for node in doc.descendants() {
        if node.is_element() && node.tag_name().name().eq_ignore_ascii_case("nav") {
            let is_toc = node
                .attributes()
                .any(|a| a.name().eq_ignore_ascii_case("type") && a.value().split_whitespace().any(|v| v.eq_ignore_ascii_case("toc")));
            if is_toc {
                best_nav = Some(node);
                break;
            }
            if best_nav.is_none() {
                best_nav = Some(node);
            }
        }
    }
    let Some(nav_node) = best_nav else {
        return Vec::new();
    };
    for child in nav_node.children() {
        if child.is_element() && child.tag_name().name().eq_ignore_ascii_case("ol") {
            return parse_nav_ol(child, nav_dir, 0);
        }
    }
    // Sometimes <ol> is nested inside a wrapper div inside <nav>
    for desc in nav_node.descendants() {
        if desc.is_element() && desc.tag_name().name().eq_ignore_ascii_case("ol") {
            return parse_nav_ol(desc, nav_dir, 0);
        }
    }
    Vec::new()
}

fn parse_nav_ol(ol_node: roxmltree::Node<'_, '_>, nav_dir: &str, level: u8) -> Vec<RawTocNode> {
    let mut out = Vec::new();
    for li in ol_node.children() {
        if !li.is_element() || !li.tag_name().name().eq_ignore_ascii_case("li") {
            continue;
        }
        let mut label: Option<String> = None;
        let mut href: Option<String> = None;
        let mut children: Vec<RawTocNode> = Vec::new();

        for ch in li.children() {
            if !ch.is_element() {
                continue;
            }
            let tag = ch.tag_name().name();
            if tag.eq_ignore_ascii_case("a") && label.is_none() {
                let txt = collapse_ws(&collect_node_text(ch)).trim().to_string();
                if !txt.is_empty() {
                    label = Some(txt);
                }
                href = ch.attribute("href").map(|s| s.to_string());
            } else if tag.eq_ignore_ascii_case("span") && label.is_none() {
                let txt = collapse_ws(&collect_node_text(ch)).trim().to_string();
                if !txt.is_empty() {
                    label = Some(txt);
                }
            } else if tag.eq_ignore_ascii_case("ol") {
                children.extend(parse_nav_ol(ch, nav_dir, level.saturating_add(1)));
            }
        }

        if let Some(title) = label {
            let (zip_path, fragment) = if let Some(raw_href) = href {
                let mut parts = raw_href.splitn(2, '#');
                let file_part = parts.next().unwrap_or("");
                let frag = parts.next().map(|f| percent_decode(f)).filter(|f| !f.is_empty());
                (normalize_zip_path(nav_dir, &percent_decode(file_part)), frag)
            } else {
                (String::new(), None)
            };
            out.push(RawTocNode {
                title,
                level,
                zip_path,
                fragment,
                children,
            });
        } else {
            out.extend(children);
        }
    }
    out
}

fn parse_ncx_toc_roxmltree(ncx_xml: &str, ncx_dir: &str) -> Vec<RawTocNode> {
    let cleaned = prepare_xml_for_roxmltree(ncx_xml);
    let Ok(doc) = roxmltree::Document::parse(&cleaned) else {
        return Vec::new();
    };
    let Some(nav_map) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("navMap"))
    else {
        return Vec::new();
    };
    parse_ncx_children(nav_map, ncx_dir, 0)
}

fn parse_ncx_children(parent: roxmltree::Node<'_, '_>, ncx_dir: &str, level: u8) -> Vec<RawTocNode> {
    let mut out = Vec::new();
    for ch in parent.children() {
        if !ch.is_element() || !ch.tag_name().name().eq_ignore_ascii_case("navPoint") {
            continue;
        }
        let mut title = String::new();
        let mut src = String::new();
        for sub in ch.children() {
            if !sub.is_element() {
                continue;
            }
            let name = sub.tag_name().name();
            if name.eq_ignore_ascii_case("navLabel") {
                title = collapse_ws(&collect_node_text(sub)).trim().to_string();
            } else if name.eq_ignore_ascii_case("content") {
                if let Some(s) = sub.attribute("src") {
                    src = s.to_string();
                }
            }
        }
        let children = parse_ncx_children(ch, ncx_dir, level.saturating_add(1));
        if !title.is_empty() {
            let mut parts = src.splitn(2, '#');
            let file_part = parts.next().unwrap_or("");
            let frag = parts.next().map(|f| percent_decode(f)).filter(|f| !f.is_empty());
            out.push(RawTocNode {
                title,
                level,
                zip_path: normalize_zip_path(ncx_dir, &percent_decode(file_part)),
                fragment: frag,
                children,
            });
        } else {
            out.extend(children);
        }
    }
    out
}

fn collect_raw_toc_labels(nodes: &[RawTocNode], out: &mut HashMap<String, String>) {
    for n in nodes {
        if !n.zip_path.is_empty() && !n.title.is_empty() {
            out.entry(n.zip_path.clone()).or_insert_with(|| n.title.clone());
        }
        collect_raw_toc_labels(&n.children, out);
    }
}

fn resolve_raw_toc(
    nodes: &[RawTocNode],
    chapter_start_block: &HashMap<String, usize>,
    anchor_blocks: &HashMap<(String, String), usize>,
    total_blocks: usize,
) -> Vec<TocEntry> {
    let max_b = total_blocks.saturating_sub(1);
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let block_idx = n
            .fragment
            .as_ref()
            .and_then(|frag| anchor_blocks.get(&(n.zip_path.clone(), frag.clone())).copied())
            .or_else(|| chapter_start_block.get(&n.zip_path).copied())
            .unwrap_or(0)
            .min(max_b);
        let children = resolve_raw_toc(&n.children, chapter_start_block, anchor_blocks, total_blocks);
        out.push(TocEntry {
            title: n.title.clone(),
            level: n.level,
            block_idx,
            children,
        });
    }
    out
}

fn build_toc_from_headings(headings: &[MdHeading]) -> Vec<TocEntry> {
    let mut root: Vec<TocEntry> = Vec::new();
    for h in headings {
        if h.level > 3 {
            continue;
        }
        let entry = TocEntry {
            title: h.title.clone(),
            level: h.level.saturating_sub(1),
            block_idx: h.block_idx,
            children: Vec::new(),
        };
        if h.level <= 1 || root.is_empty() {
            root.push(entry);
        } else if h.level == 2 {
            if let Some(last) = root.last_mut() {
                if last.level == 0 {
                    last.children.push(TocEntry { level: 1, ..entry });
                } else {
                    root.push(entry);
                }
            }
        } else if let Some(last) = root.last_mut() {
            if let Some(sub) = last.children.last_mut() {
                sub.children.push(TocEntry { level: 2, ..entry });
            } else {
                last.children.push(TocEntry { level: 1, ..entry });
            }
        }
    }
    root
}

/// Replaces common HTML4 named entities (which XML parsers reject without a DTD) with
/// numeric XML character references while leaving standard XML entities untouched.
fn prepare_xml_for_roxmltree(src: &str) -> String {
    // Strip DOCTYPE declarations referencing external HTML DTDs that aren't needed by roxmltree
    let mut out = String::with_capacity(src.len());
    let mut s = src;
    // Also strip UTF-8 BOM if present
    s = s.strip_prefix('\u{FEFF}').unwrap_or(s);

    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(semi) = s[i..].find(';') {
                if semi > 1 && semi <= 12 {
                    let ent = &s[i + 1..i + semi];
                    let replacement = match ent {
                        "amp" | "lt" | "gt" | "quot" | "apos" => None,
                        _ if ent.starts_with('#') => None,
                        "nbsp" => Some("&#160;"),
                        "iexcl" => Some("&#161;"),
                        "cent" => Some("&#162;"),
                        "pound" => Some("&#163;"),
                        "curren" => Some("&#164;"),
                        "yen" => Some("&#165;"),
                        "brvbar" => Some("&#166;"),
                        "sect" => Some("&#167;"),
                        "uml" => Some("&#168;"),
                        "copy" => Some("&#169;"),
                        "ordf" => Some("&#170;"),
                        "laquo" => Some("&#171;"),
                        "not" => Some("&#172;"),
                        "shy" => Some("&#173;"),
                        "reg" => Some("&#174;"),
                        "macr" => Some("&#175;"),
                        "deg" => Some("&#176;"),
                        "plusmn" => Some("&#177;"),
                        "sup2" => Some("&#178;"),
                        "sup3" => Some("&#179;"),
                        "acute" => Some("&#180;"),
                        "micro" => Some("&#181;"),
                        "para" => Some("&#182;"),
                        "middot" => Some("&#183;"),
                        "cedil" => Some("&#184;"),
                        "sup1" => Some("&#185;"),
                        "ordm" => Some("&#186;"),
                        "raquo" => Some("&#187;"),
                        "frac14" => Some("&#188;"),
                        "frac12" => Some("&#189;"),
                        "frac34" => Some("&#190;"),
                        "iquest" => Some("&#191;"),
                        "times" => Some("&#215;"),
                        "divide" => Some("&#247;"),
                        "ndash" => Some("&#8211;"),
                        "mdash" => Some("&#8212;"),
                        "lsquo" => Some("&#8216;"),
                        "rsquo" => Some("&#8217;"),
                        "sbquo" => Some("&#8218;"),
                        "ldquo" => Some("&#8220;"),
                        "rdquo" => Some("&#8221;"),
                        "bdquo" => Some("&#8222;"),
                        "dagger" => Some("&#8224;"),
                        "Dagger" => Some("&#8225;"),
                        "bull" => Some("&#8226;"),
                        "hellip" => Some("&#8230;"),
                        "permil" => Some("&#8240;"),
                        "prime" => Some("&#8242;"),
                        "Prime" => Some("&#8243;"),
                        "lsaquo" => Some("&#8249;"),
                        "rsaquo" => Some("&#8250;"),
                        "oline" => Some("&#8254;"),
                        "euro" => Some("&#8364;"),
                        "trade" => Some("&#8482;"),
                        "larr" => Some("&#8592;"),
                        "uarr" => Some("&#8593;"),
                        "rarr" => Some("&#8594;"),
                        "darr" => Some("&#8595;"),
                        "harr" => Some("&#8596;"),
                        "minus" => Some("&#8722;"),
                        "le" => Some("&#8804;"),
                        "ge" => Some("&#8805;"),
                        "ne" => Some("&#8800;"),
                        "asymp" => Some("&#8776;"),
                        _ => None,
                    };
                    if let Some(rep) = replacement {
                        out.push_str(rep);
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

#[allow(clippy::too_many_arguments)]
fn parse_xhtml_chapter_roxmltree(
    html: &str,
    chap_path: &str,
    chap_dir: &str,
    zip: &ZipReader<'_>,
    blocks: &mut Vec<MdBlock>,
    headings: &mut Vec<MdHeading>,
    embedded_images: &mut HashMap<String, Vec<u8>>,
    anchor_blocks: &mut HashMap<(String, String), usize>,
) -> bool {
    let cleaned = prepare_xml_for_roxmltree(html);
    let Ok(doc) = roxmltree::Document::parse(&cleaned) else {
        return false;
    };

    let body = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("body"))
        .unwrap_or_else(|| doc.root_element());

    struct DomCtx<'a> {
        chap_path: &'a str,
        chap_dir: &'a str,
        zip: &'a ZipReader<'a>,
        blocks: &'a mut Vec<MdBlock>,
        headings: &'a mut Vec<MdHeading>,
        embedded_images: &'a mut HashMap<String, Vec<u8>>,
        anchor_blocks: &'a mut HashMap<(String, String), usize>,
    }

    fn is_block_elem(tag: &str) -> bool {
        matches!(
            tag.to_ascii_lowercase().as_str(),
            "h1" | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "p"
                | "blockquote"
                | "pre"
                | "hr"
                | "ul"
                | "ol"
                | "table"
                | "div"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "aside"
                | "main"
                | "figure"
        )
    }

    fn record_id(ctx: &mut DomCtx<'_>, node: roxmltree::Node<'_, '_>, blk_idx: usize) {
        if let Some(id) = node.attribute("id") {
            let trimmed = id.trim();
            if !trimmed.is_empty() {
                ctx.anchor_blocks
                    .entry((ctx.chap_path.to_string(), trimmed.to_string()))
                    .or_insert(blk_idx);
            }
        }
    }

    fn collect_inline(
        ctx: &mut DomCtx<'_>,
        node: roxmltree::Node<'_, '_>,
        style: &mut HtmlStyle,
        spans: &mut Vec<InlineSpan>,
        anchor_target: usize,
    ) {
        if node.is_text() {
            if let Some(raw_txt) = node.text() {
                let collapsed = collapse_ws(raw_txt);
                if !collapsed.is_empty() && !(spans.is_empty() && collapsed == " ") {
                    style.push_text(spans, &collapsed);
                }
            }
            return;
        }
        if !node.is_element() {
            return;
        }
        record_id(ctx, node, anchor_target);
        let tag = node.tag_name().name().to_ascii_lowercase();
        match tag.as_str() {
            "head" | "script" | "style" | "title" => return,
            "br" => {
                style.push_text(spans, " ");
                return;
            }
            "img" | "image" => {
                let src = node
                    .attribute("src")
                    .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")))
                    .or_else(|| node.attribute("href"));
                if let Some(src_val) = src {
                    let zip_key = normalize_zip_path(ctx.chap_dir, &percent_decode(src_val));
                    if let Ok(img_bytes) = ctx.zip.read_file(&zip_key) {
                        let alt = node
                            .attribute("alt")
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        ctx.embedded_images.insert(zip_key.clone(), img_bytes);
                        ctx.blocks.push(MdBlock::Image {
                            path: zip_key,
                            caption: alt,
                        });
                    }
                }
                return;
            }
            "strong" | "b" => style.bold += 1,
            "em" | "i" => style.italic += 1,
            "s" | "del" | "strike" => style.strike += 1,
            "code" | "tt" | "kbd" => style.code += 1,
            "a" => {
                if let Some(href) = node.attribute("href") {
                    style.flush_link(spans);
                    style.link_url = Some(href.to_string());
                }
            }
            _ => {}
        }

        for ch in node.children() {
            collect_inline(ctx, ch, style, spans, anchor_target);
        }

        match tag.as_str() {
            "strong" | "b" => style.bold = style.bold.saturating_sub(1),
            "em" | "i" => style.italic = style.italic.saturating_sub(1),
            "s" | "del" | "strike" => style.strike = style.strike.saturating_sub(1),
            "code" | "tt" | "kbd" => style.code = style.code.saturating_sub(1),
            "a" => style.flush_link(spans),
            _ => {}
        }
    }

    fn parse_list(ctx: &mut DomCtx<'_>, list_node: roxmltree::Node<'_, '_>, ordered: bool) -> Option<MdBlock> {
        let mut items = Vec::new();
        for ch in list_node.children() {
            if !ch.is_element() || !ch.tag_name().name().eq_ignore_ascii_case("li") {
                continue;
            }
            let mut spans = Vec::new();
            let mut style = HtmlStyle::default();
            let mut sublists = Vec::new();
            let anchor_target = ctx.blocks.len();
            for sub in ch.children() {
                if sub.is_element()
                    && (sub.tag_name().name().eq_ignore_ascii_case("ul")
                        || sub.tag_name().name().eq_ignore_ascii_case("ol"))
                {
                    let sub_ord = sub.tag_name().name().eq_ignore_ascii_case("ol");
                    if let Some(sl) = parse_list(ctx, sub, sub_ord) {
                        sublists.push(sl);
                    }
                } else {
                    collect_inline(ctx, sub, &mut style, &mut spans, anchor_target);
                }
            }
            style.flush_link(&mut spans);
            if !spans.is_empty() || !sublists.is_empty() {
                items.push(MdListItem {
                    task_checked: None,
                    spans,
                    sublists,
                });
            }
        }
        if items.is_empty() {
            None
        } else {
            Some(MdBlock::List {
                ordered,
                start: 1,
                items,
            })
        }
    }

    fn parse_table(ctx: &mut DomCtx<'_>, tbl_node: roxmltree::Node<'_, '_>) -> Option<MdBlock> {
        let mut headers: Vec<Vec<InlineSpan>> = Vec::new();
        let mut rows: Vec<Vec<Vec<InlineSpan>>> = Vec::new();
        let anchor_target = ctx.blocks.len();
        for desc in tbl_node.descendants() {
            if !desc.is_element() || !desc.tag_name().name().eq_ignore_ascii_case("tr") {
                continue;
            }
            let mut row_cells = Vec::new();
            let mut is_hdr = false;
            for cell in desc.children() {
                if !cell.is_element() {
                    continue;
                }
                let cname = cell.tag_name().name();
                if cname.eq_ignore_ascii_case("th") || cname.eq_ignore_ascii_case("td") {
                    if cname.eq_ignore_ascii_case("th") {
                        is_hdr = true;
                    }
                    let mut spans = Vec::new();
                    let mut style = HtmlStyle::default();
                    for ch in cell.children() {
                        collect_inline(ctx, ch, &mut style, &mut spans, anchor_target);
                    }
                    style.flush_link(&mut spans);
                    row_cells.push(spans);
                }
            }
            if !row_cells.is_empty() {
                if is_hdr && headers.is_empty() {
                    headers = row_cells;
                } else {
                    rows.push(row_cells);
                }
            }
        }
        if headers.is_empty() && !rows.is_empty() {
            headers = rows.remove(0);
        }
        if headers.is_empty() && rows.is_empty() {
            return None;
        }
        let ncols = headers
            .len()
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
        Some(MdBlock::Table {
            alignments: vec![MdAlign::Left; ncols],
            headers,
            rows,
        })
    }

    fn walk_node(ctx: &mut DomCtx<'_>, node: roxmltree::Node<'_, '_>) {
        if node.is_text() {
            if let Some(txt) = node.text() {
                let collapsed = collapse_ws(txt);
                if !collapsed.trim().is_empty() {
                    ctx.blocks
                        .push(MdBlock::Paragraph(vec![InlineSpan::Text(collapsed.trim().to_string())]));
                }
            }
            return;
        }
        if !node.is_element() {
            return;
        }
        let tag = node.tag_name().name().to_ascii_lowercase();
        if tag == "nav" {
            let is_toc_nav = node.attributes().any(|a| {
                (a.name().eq_ignore_ascii_case("type")
                    && a.value().split_whitespace().any(|v| v.eq_ignore_ascii_case("toc")))
                    || (a.name().eq_ignore_ascii_case("role")
                        && a.value().eq_ignore_ascii_case("doc-toc"))
            });
            if is_toc_nav {
                return;
            }
        }
        if !ctx.blocks.is_empty() && !matches!(ctx.blocks.last(), Some(MdBlock::PageBreak)) {
            if let Some(style_attr) = node.attribute("style") {
                let lower = style_attr.to_ascii_lowercase();
                if lower.contains("page-break-before:always")
                    || lower.contains("page-break-before: always")
                    || lower.contains("break-before:page")
                    || lower.contains("break-before: page")
                {
                    ctx.blocks.push(MdBlock::PageBreak);
                }
            }
        }
        let b_idx_here = ctx.blocks.len();
        record_id(ctx, node, b_idx_here);
        match tag.as_str() {
            "head" | "script" | "style" | "title" => return,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let lvl = tag[1..].parse::<u8>().unwrap_or(2);
                let mut spans = Vec::new();
                let mut style = HtmlStyle::default();
                for ch in node.children() {
                    collect_inline(ctx, ch, &mut style, &mut spans, b_idx_here);
                }
                style.flush_link(&mut spans);
                let title: String = spans.iter().map(|s| s.as_plain_text()).collect();
                let title = title.trim().to_string();
                if !title.is_empty() {
                    let b_idx = ctx.blocks.len();
                    record_id(ctx, node, b_idx);
                    ctx.blocks.push(MdBlock::Heading {
                        level: lvl,
                        spans,
                    });
                    if lvl <= 3 {
                        ctx.headings.push(MdHeading {
                            level: lvl,
                            title,
                            block_idx: b_idx,
                        });
                    }
                }
            }
            "p" => {
                let mut spans = Vec::new();
                let mut style = HtmlStyle::default();
                for ch in node.children() {
                    collect_inline(ctx, ch, &mut style, &mut spans, b_idx_here);
                }
                style.flush_link(&mut spans);
                if spans.iter().any(|s| !s.as_plain_text().trim().is_empty()) {
                    let b_idx = ctx.blocks.len();
                    record_id(ctx, node, b_idx);
                    ctx.blocks.push(MdBlock::Paragraph(spans));
                }
            }
            "blockquote" => {
                let mut inner_blocks = Vec::new();
                let has_nested_blocks = node
                    .children()
                    .any(|c| c.is_element() && is_block_elem(c.tag_name().name()));
                if has_nested_blocks {
                    let prev_len = ctx.blocks.len();
                    for ch in node.children() {
                        walk_node(ctx, ch);
                    }
                    if ctx.blocks.len() > prev_len {
                        inner_blocks = ctx.blocks.drain(prev_len..).collect();
                    }
                } else {
                    let mut spans = Vec::new();
                    let mut style = HtmlStyle::default();
                    for ch in node.children() {
                        collect_inline(ctx, ch, &mut style, &mut spans, b_idx_here);
                    }
                    style.flush_link(&mut spans);
                    if spans.iter().any(|s| !s.as_plain_text().trim().is_empty()) {
                        inner_blocks.push(MdBlock::Paragraph(spans));
                    }
                }
                if !inner_blocks.is_empty() {
                    ctx.blocks.push(MdBlock::Blockquote(inner_blocks));
                }
            }
            "pre" => {
                let code = collect_node_text(node);
                if !code.trim().is_empty() {
                    ctx.blocks.push(MdBlock::CodeBlock { lang: None, code });
                }
            }
            "hr" => {
                ctx.blocks.push(MdBlock::HorizontalRule);
            }
            "ul" | "ol" => {
                let ordered = tag == "ol";
                if let Some(list_blk) = parse_list(ctx, node, ordered) {
                    ctx.blocks.push(list_blk);
                }
            }
            "table" => {
                if let Some(tbl_blk) = parse_table(ctx, node) {
                    ctx.blocks.push(tbl_blk);
                }
            }
            "img" | "image" => {
                let src = node
                    .attribute("src")
                    .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")))
                    .or_else(|| node.attribute("href"));
                if let Some(src_val) = src {
                    let zip_key = normalize_zip_path(ctx.chap_dir, &percent_decode(src_val));
                    if let Ok(img_bytes) = ctx.zip.read_file(&zip_key) {
                        let alt = node
                            .attribute("alt")
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        ctx.embedded_images.insert(zip_key.clone(), img_bytes);
                        ctx.blocks.push(MdBlock::Image {
                            path: zip_key,
                            caption: alt,
                        });
                    }
                }
            }
            "div" | "section" | "article" | "header" | "footer" | "aside" | "main" | "figure" | "figcaption" => {
                let has_child_blocks = node
                    .children()
                    .any(|c| c.is_element() && is_block_elem(c.tag_name().name()));
                if has_child_blocks {
                    for ch in node.children() {
                        walk_node(ctx, ch);
                    }
                } else {
                    let mut spans = Vec::new();
                    let mut style = HtmlStyle::default();
                    for ch in node.children() {
                        collect_inline(ctx, ch, &mut style, &mut spans, b_idx_here);
                    }
                    style.flush_link(&mut spans);
                    if spans.iter().any(|s| !s.as_plain_text().trim().is_empty()) {
                        ctx.blocks.push(MdBlock::Paragraph(spans));
                    }
                }
            }
            _ => {
                for ch in node.children() {
                    walk_node(ctx, ch);
                }
            }
        }
    }

    let mut ctx = DomCtx {
        chap_path,
        chap_dir,
        zip,
        blocks,
        headings,
        embedded_images,
        anchor_blocks,
    };
    for ch in body.children() {
        walk_node(&mut ctx, ch);
    }
    true
}

// ---------------------------------------------------------------------------
// XHTML / HTML Tolerant Fallback Scanner
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

    #[test]
    fn test_epub_roxmltree_metadata_nested_nav_and_ncx_toc() {
        let mut zw = ZipWriter::new();
        zw.add_stored("mimetype", b"application/epub+zip");
        zw.add_deflated(
            "META-INF/container.xml",
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        );
        zw.add_deflated(
            "EPUB/package.opf",
            br#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Moby Typography</dc:title>
    <dc:creator>Herman Melville</dc:creator>
    <dc:publisher>Pequod Press</dc:publisher>
    <dc:language>en-US</dc:language>
    <dc:description>A classic whaling tale with nested chapters.</dc:description>
    <meta name="cover" content="cover-img"/>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ch1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="cover-img" href="images/cover.png" media-type="image/png" properties="cover-image"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
  </spine>
</package>"#,
        );
        let fake_png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        zw.add_stored("EPUB/images/cover.png", &fake_png);
        zw.add_deflated(
            "EPUB/nav.xhtml",
            br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
  <nav epub:type="toc">
    <ol>
      <li>
        <a href="text/ch1.xhtml#part1">Part One: Loomings</a>
        <ol>
          <li><a href="text/ch1.xhtml#sec1">Section 1.1: Ishmael</a></li>
        </ol>
      </li>
    </ol>
  </nav>
</body>
</html>"#,
        );
        zw.add_deflated(
            "EPUB/text/ch1.xhtml",
            r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <h1 id="part1">Part One: Loomings</h1>
  <p>Call me <em>Ishmael</em>.</p>
  <h2 id="sec1">Section 1.1: Ishmael</h2>
  <p>Some years ago—never mind how long precisely.</p>
</body>
</html>"#
                .as_bytes(),
        );

        let epub_bytes = zw.finish();
        let doc = parse_epub_bytes(&epub_bytes, "Fallback").expect("parse epub3");
        let meta = doc.book_meta.as_ref().expect("book_meta populated");
        assert_eq!(meta.title, "Moby Typography");
        assert_eq!(meta.author.as_deref(), Some("Herman Melville"));
        assert_eq!(meta.publisher.as_deref(), Some("Pequod Press"));
        assert_eq!(meta.language.as_deref(), Some("en-US"));
        assert_eq!(
            meta.description.as_deref(),
            Some("A classic whaling tale with nested chapters.")
        );
        assert_eq!(meta.cover_image_key.as_deref(), Some("EPUB/images/cover.png"));
        assert_eq!(meta.toc.len(), 1);
        assert_eq!(meta.toc[0].title, "Part One: Loomings");
        assert_eq!(meta.toc[0].level, 0);
        assert_eq!(meta.toc[0].children.len(), 1);
        assert_eq!(meta.toc[0].children[0].title, "Section 1.1: Ishmael");
        assert_eq!(meta.toc[0].children[0].level, 1);
        assert!(meta.toc[0].children[0].block_idx > meta.toc[0].block_idx);

        // Must NOT inject "By Herman Melville" or synthetic cover image into body blocks
        let has_by_author = doc.blocks.iter().any(|b| match b {
            MdBlock::Paragraph(sp) => sp.iter().map(|s| s.as_plain_text()).collect::<String>().contains("By Herman Melville"),
            _ => false,
        });
        assert!(!has_by_author, "Distinct BookMetadata must not duplicate By Author in body blocks");
        assert!(!matches!(doc.blocks.first(), Some(MdBlock::Image { .. })), "Cover stays separate in BookMetadata");

        // Now verify EPUB 2 toc.ncx nested fallback when no nav.xhtml exists
        let mut zw2 = ZipWriter::new();
        zw2.add_stored("mimetype", b"application/epub+zip");
        zw2.add_deflated(
            "META-INF/container.xml",
            br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        );
        zw2.add_deflated(
            "content.opf",
            br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>NCX Book</dc:title><dc:creator>Ada</dc:creator></metadata>
  <manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine toc="ncx"><itemref idref="c1"/></spine>
</package>"#,
        );
        zw2.add_deflated(
            "toc.ncx",
            br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap>
    <navPoint id="np1" playOrder="1">
      <navLabel><text>Book I</text></navLabel>
      <content src="c1.xhtml#b1"/>
      <navPoint id="np2" playOrder="2">
        <navLabel><text>Chapter 1</text></navLabel>
        <content src="c1.xhtml#c1"/>
      </navPoint>
    </navPoint>
  </navMap>
</ncx>"#,
        );
        zw2.add_deflated(
            "c1.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1 id="b1">Book I</h1><h2 id="c1">Chapter 1</h2><p>Body.</p></body></html>"#,
        );
        let doc2 = parse_epub_bytes(&zw2.finish(), "NCX").expect("parse ncx epub2");
        let meta2 = doc2.book_meta.as_ref().expect("ncx meta");
        assert_eq!(meta2.toc.len(), 1);
        assert_eq!(meta2.toc[0].title, "Book I");
        assert_eq!(meta2.toc[0].children.len(), 1);
        assert_eq!(meta2.toc[0].children[0].title, "Chapter 1");
    }

    #[test]
    fn test_epub_chapter_page_breaks_and_toc_deduplication() {
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
    <dc:title>Clean Book</dc:title>
    <dc:creator>Author Name</dc:creator>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="htmltoc" href="toc.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="htmltoc"/>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
  </spine>
  <guide>
    <reference type="toc" title="Table of Contents" href="toc.xhtml"/>
  </guide>
</package>"#,
        );
        zw.add_deflated(
            "OEBPS/nav.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body><nav epub:type="toc"><ol>
  <li><a href="toc.xhtml">Table of Contents</a></li>
  <li><a href="ch1.xhtml">Chapter 1: Dawn</a></li>
  <li><a href="ch2.xhtml">Chapter 2: Noon</a></li>
</ol></nav></body></html>"#,
        );
        zw.add_deflated(
            "OEBPS/toc.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <h1>Table of Contents</h1>
  <ul>
    <li><a href="ch1.xhtml">Chapter 1: Dawn</a></li>
    <li><a href="ch2.xhtml">Chapter 2: Noon</a></li>
  </ul>
</body></html>"#,
        );
        zw.add_deflated(
            "OEBPS/ch1.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
  <nav epub:type="toc"><ol><li><a href="ch2.xhtml">Inline Toc Link</a></li></ol></nav>
  <h1>Chapter 1: Dawn</h1>
  <p>Morning light across the valley.</p>
</body></html>"#,
        );
        zw.add_deflated(
            "OEBPS/ch2.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <h1>Chapter 2: Noon</h1>
  <p>High sun above the harbor.</p>
</body></html>"#,
        );

        let doc = parse_epub_bytes(&zw.finish(), "Clean Book").expect("parse epub");
        let meta = doc.book_meta.as_ref().expect("book_meta");
        // Sidebar TOC keeps Chapter 1 & Chapter 2 and strips self-link to skipped toc.xhtml
        assert_eq!(meta.toc.len(), 2);
        assert_eq!(meta.toc[0].title, "Chapter 1: Dawn");
        assert_eq!(meta.toc[1].title, "Chapter 2: Noon");

        // Body must NOT contain standalone Table of Contents heading or inline <nav epub:type="toc">
        let all_body_text: String = doc
            .blocks
            .iter()
            .map(|b| match b {
                MdBlock::Heading { spans, .. } | MdBlock::Paragraph(spans) => {
                    spans.iter().map(|s| s.as_plain_text()).collect::<String>()
                }
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!all_body_text.contains("Table of Contents"), "TOC chapter must not appear in book body");
        assert!(!all_body_text.contains("Inline Toc Link"), "Inline nav toc must not appear in book body");
        assert!(all_body_text.contains("Morning light"));
        assert!(all_body_text.contains("High sun"));

        // Chapter 1 and Chapter 2 must be separated by an explicit MdBlock::PageBreak
        let ch1_idx = doc.blocks.iter().position(|b| matches!(b, MdBlock::Heading { spans, .. } if spans.iter().any(|s| s.as_plain_text().contains("Chapter 1")))).expect("ch1");
        let ch2_idx = doc.blocks.iter().position(|b| matches!(b, MdBlock::Heading { spans, .. } if spans.iter().any(|s| s.as_plain_text().contains("Chapter 2")))).expect("ch2");
        assert!(
            doc.blocks[ch1_idx..ch2_idx].iter().any(|b| matches!(b, MdBlock::PageBreak)),
            "Expected MdBlock::PageBreak between Chapter 1 and Chapter 2"
        );
    }
}
