//! Pure-Rust Word (`.docx`) OOXML Reader.
//!
//! Parses `.docx` archives directly from memory using `src/zip_mini.rs` (zero temp files)
//! and `roxmltree` into MicroPDF's shared `MarkdownDoc` AST + `BookMetadata`.

use anyhow::Result;
use std::collections::HashMap;

use crate::markdown::{
    BookMetadata, InlineSpan, MarkdownDoc, MdAlign, MdBlock, MdHeading, MdListItem, TocEntry,
};
use crate::zip_mini::ZipReader;

#[derive(Clone, Debug, Default)]
struct StyleInfo {
    name: String,
    based_on: Option<String>,
    heading_level: Option<u8>,
    outline_level: Option<u8>,
    is_quote: bool,
    is_code: bool,
    bold: bool,
    italic: bool,
    strike: bool,
}

#[derive(Clone, Debug)]
struct RelTarget {
    target: String,
    is_external: bool,
}

#[derive(Clone, Copy, Debug)]
struct NumberingLevel {
    ordered: bool,
    start: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct RunProps {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    font_size_half_pt: Option<u32>,
}

enum InlineOrBreak {
    Span(InlineSpan),
    PageBreak,
    Image { key: String, alt: Option<String> },
}

#[inline]
fn attr_local<'a>(node: roxmltree::Node<'a, 'a>, local: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == local)
        .map(|a| a.value())
}

#[inline]
fn is_tag(node: roxmltree::Node<'_, '_>, local: &str) -> bool {
    node.is_element() && node.tag_name().name() == local
}

fn find_child<'a>(node: roxmltree::Node<'a, 'a>, local: &str) -> Option<roxmltree::Node<'a, 'a>> {
    node.children().find(|c| is_tag(*c, local))
}

#[inline]
fn bool_prop_on(node: roxmltree::Node<'_, '_>) -> bool {
    match attr_local(node, "val") {
        None => true,
        Some(v) => !matches!(v.trim(), "0" | "false" | "FALSE" | "off" | "OFF"),
    }
}

fn is_monospace_font(font_name: &str) -> bool {
    let lower = font_name.to_ascii_lowercase();
    lower.contains("consolas")
        || lower.contains("courier")
        || lower.contains("mono")
        || lower.contains("menlo")
        || lower.contains("monaco")
        || lower.contains("fira code")
        || lower.contains("cascadia")
        || lower.contains("source code")
        || lower.contains("inconsolata")
}

fn normalize_zip_path(base_dir: &str, rel: &str) -> String {
    let raw = rel.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    if !raw.starts_with('/') && !base_dir.is_empty() {
        for seg in base_dir.split('/') {
            if !seg.is_empty() && seg != "." {
                parts.push(seg);
            }
        }
    }
    for seg in raw.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(idx) => &path[..idx],
        None => "",
    }
}

fn read_xml_string(zip: &ZipReader<'_>, path: &str) -> Option<String> {
    let bytes = zip.read_file(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(text.strip_prefix('\u{FEFF}').unwrap_or(&text).to_string())
}

fn resolve_main_document_path(zip: &ZipReader<'_>) -> String {
    if let Some(rels_xml) = read_xml_string(zip, "_rels/.rels") {
        if let Ok(doc) = roxmltree::Document::parse(&rels_xml) {
            for node in doc.descendants() {
                if is_tag(node, "Relationship") {
                    let rtype = attr_local(node, "Type").unwrap_or_default();
                    if rtype.ends_with("/officeDocument") {
                        if let Some(target) = attr_local(node, "Target") {
                            let norm = normalize_zip_path("", target);
                            if !norm.is_empty() {
                                return norm;
                            }
                        }
                    }
                }
            }
        }
    }
    "word/document.xml".to_string()
}

fn parse_core_props(zip: &ZipReader<'_>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(xml) = read_xml_string(zip, "docProps/core.xml") else {
        return (None, None, None);
    };
    let Ok(doc) = roxmltree::Document::parse(&xml) else {
        return (None, None, None);
    };
    let mut title = None;
    let mut creator = None;
    let mut description = None;
    let mut subject = None;
    for node in doc.descendants().filter(|n| n.is_element()) {
        let txt = node
            .text()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        match node.tag_name().name() {
            "title" if title.is_none() => title = txt,
            "creator" if creator.is_none() => creator = txt,
            "description" if description.is_none() => description = txt,
            "subject" if subject.is_none() => subject = txt,
            _ => {}
        }
    }
    (title, creator, description.or(subject))
}

fn parse_relationships(zip: &ZipReader<'_>, doc_path: &str) -> HashMap<String, RelTarget> {
    let mut map = HashMap::new();
    let doc_dir = parent_dir(doc_path);
    let doc_file = doc_path.rsplit('/').next().unwrap_or("document.xml");
    let rels_path = if doc_dir.is_empty() {
        format!("_rels/{doc_file}.rels")
    } else {
        format!("{doc_dir}/_rels/{doc_file}.rels")
    };
    let Some(xml) = read_xml_string(zip, &rels_path) else {
        return map;
    };
    let Ok(doc) = roxmltree::Document::parse(&xml) else {
        return map;
    };
    for node in doc.descendants() {
        if is_tag(node, "Relationship") {
            let Some(id) = attr_local(node, "Id") else { continue };
            let Some(target) = attr_local(node, "Target") else { continue };
            let is_external = attr_local(node, "TargetMode")
                .is_some_and(|m| m.eq_ignore_ascii_case("External"));
            let resolved = if is_external {
                target.to_string()
            } else {
                normalize_zip_path(doc_dir, target)
            };
            map.insert(
                id.to_string(),
                RelTarget {
                    target: resolved,
                    is_external,
                },
            );
        }
    }
    map
}

fn heading_level_from_name_or_id(s: &str) -> Option<u8> {
    let lower = s.trim().to_ascii_lowercase();
    if lower == "title" {
        return Some(1);
    }
    if lower == "subtitle" {
        return Some(2);
    }
    for prefix in ["heading ", "heading", "toc "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if let Ok(n) = rest.trim().parse::<u8>() {
                if (1..=9).contains(&n) {
                    return Some(n.min(6));
                }
            }
        }
    }
    if let Some(rest) = lower.strip_prefix('h') {
        if rest.len() == 1 {
            if let Ok(n) = rest.parse::<u8>() {
                if (1..=6).contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn parse_styles(zip: &ZipReader<'_>) -> HashMap<String, StyleInfo> {
    let mut styles = HashMap::new();
    let Some(xml) = read_xml_string(zip, "word/styles.xml") else {
        return styles;
    };
    let Ok(doc) = roxmltree::Document::parse(&xml) else {
        return styles;
    };
    for node in doc.descendants() {
        if !is_tag(node, "style") {
            continue;
        }
        let Some(style_id) = attr_local(node, "styleId") else {
            continue;
        };
        let mut info = StyleInfo::default();
        if let Some(name_node) = find_child(node, "name") {
            if let Some(v) = attr_local(name_node, "val") {
                info.name = v.to_string();
            }
        }
        if let Some(based) = find_child(node, "basedOn") {
            info.based_on = attr_local(based, "val").map(str::to_string);
        }
        info.heading_level = heading_level_from_name_or_id(&info.name)
            .or_else(|| heading_level_from_name_or_id(style_id));
        let lower_name = info.name.to_ascii_lowercase();
        let lower_id = style_id.to_ascii_lowercase();
        info.is_quote = lower_name.contains("quote") || lower_id.contains("quote");
        info.is_code = lower_name.contains("code")
            || lower_name.contains("preformatted")
            || lower_id.contains("code");

        if let Some(ppr) = find_child(node, "pPr") {
            if let Some(ol) = find_child(ppr, "outlineLvl") {
                if let Some(v) = attr_local(ol, "val").and_then(|s| s.parse::<u8>().ok()) {
                    info.outline_level = Some((v + 1).clamp(1, 6));
                }
            }
        }
        if let Some(rpr) = find_child(node, "rPr") {
            let rp = parse_rpr(rpr, None);
            info.bold = rp.bold;
            info.italic = rp.italic;
            info.strike = rp.strike;
            info.is_code |= rp.code;
        }
        styles.insert(style_id.to_string(), info);
    }

    // Resolve inherited heading / quote / code flags from basedOn chain (max depth 6)
    let keys: Vec<String> = styles.keys().cloned().collect();
    for key in keys {
        let mut cur_parent = styles.get(&key).and_then(|s| s.based_on.clone());
        let mut depth = 0usize;
        while let Some(pid) = cur_parent {
            if depth >= 6 {
                break;
            }
            depth += 1;
            let Some(pinfo) = styles.get(&pid).cloned() else {
                break;
            };
            if let Some(entry) = styles.get_mut(&key) {
                if entry.heading_level.is_none() {
                    entry.heading_level = pinfo.heading_level;
                }
                if entry.outline_level.is_none() {
                    entry.outline_level = pinfo.outline_level;
                }
                entry.is_quote |= pinfo.is_quote;
                entry.is_code |= pinfo.is_code;
            }
            cur_parent = pinfo.based_on;
        }
    }
    styles
}

fn parse_numbering(zip: &ZipReader<'_>) -> HashMap<(String, u8), NumberingLevel> {
    let mut out = HashMap::new();
    let Some(xml) = read_xml_string(zip, "word/numbering.xml") else {
        return out;
    };
    let Ok(doc) = roxmltree::Document::parse(&xml) else {
        return out;
    };

    let mut abstract_levels: HashMap<(String, u8), NumberingLevel> = HashMap::new();
    for node in doc.descendants() {
        if is_tag(node, "abstractNum") {
            let Some(abs_id) = attr_local(node, "abstractNumId") else {
                continue;
            };
            for lvl in node.children().filter(|c| is_tag(*c, "lvl")) {
                let ilvl = attr_local(lvl, "ilvl")
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(0);
                let num_fmt = find_child(lvl, "numFmt")
                    .and_then(|n| attr_local(n, "val"))
                    .unwrap_or("bullet");
                let ordered = !num_fmt.eq_ignore_ascii_case("bullet")
                    && !num_fmt.eq_ignore_ascii_case("none");
                let start = find_child(lvl, "start")
                    .and_then(|n| attr_local(n, "val"))
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1);
                abstract_levels.insert(
                    (abs_id.to_string(), ilvl),
                    NumberingLevel { ordered, start },
                );
            }
        }
    }

    for node in doc.descendants() {
        if is_tag(node, "num") {
            let Some(num_id) = attr_local(node, "numId") else {
                continue;
            };
            let Some(abs_id) = find_child(node, "abstractNumId").and_then(|n| attr_local(n, "val")) else {
                continue;
            };
            for ilvl in 0..=9u8 {
                if let Some(lvl_info) = abstract_levels.get(&(abs_id.to_string(), ilvl)) {
                    out.insert((num_id.to_string(), ilvl), *lvl_info);
                }
            }
            // Handle level overrides if present
            for ov in node.children().filter(|c| is_tag(*c, "lvlOverride")) {
                let ilvl = attr_local(ov, "ilvl")
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(0);
                if let Some(start_ov) = find_child(ov, "startOverride")
                    .and_then(|n| attr_local(n, "val"))
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    if let Some(existing) = out.get_mut(&(num_id.to_string(), ilvl)) {
                        existing.start = start_ov;
                    }
                }
            }
        }
    }
    out
}

fn parse_rpr(rpr: roxmltree::Node<'_, '_>, styles: Option<&HashMap<String, StyleInfo>>) -> RunProps {
    let mut props = RunProps::default();
    if let Some(styles_map) = styles {
        if let Some(rstyle) = find_child(rpr, "rStyle").and_then(|n| attr_local(n, "val")) {
            if let Some(st) = styles_map.get(rstyle) {
                props.bold |= st.bold;
                props.italic |= st.italic;
                props.strike |= st.strike;
                props.code |= st.is_code;
            }
        }
    }
    for child in rpr.children().filter(|c| c.is_element()) {
        match child.tag_name().name() {
            "b" => props.bold = bool_prop_on(child),
            "i" => props.italic = bool_prop_on(child),
            "strike" | "dstrike" => props.strike = bool_prop_on(child),
            "sz" => {
                if let Some(sz) = attr_local(child, "val").and_then(|s| s.parse::<u32>().ok()) {
                    props.font_size_half_pt = Some(sz);
                }
            }
            "rFonts" => {
                for attr_name in ["ascii", "hAnsi", "cs"] {
                    if let Some(f) = attr_local(child, attr_name) {
                        if is_monospace_font(f) {
                            props.code = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    props
}

fn push_inline_span(target: &mut Vec<InlineSpan>, span: InlineSpan) {
    match &span {
        InlineSpan::Text(s)
        | InlineSpan::Bold(s)
        | InlineSpan::Italic(s)
        | InlineSpan::BoldItalic(s)
        | InlineSpan::Strikethrough(s)
        | InlineSpan::Code(s)
        | InlineSpan::Math(s) if s.is_empty() => return,
        InlineSpan::Link { text, .. } if text.is_empty() => return,
        _ => {}
    }
    match (target.last_mut(), &span) {
        (Some(InlineSpan::Text(prev)), InlineSpan::Text(cur)) => prev.push_str(cur),
        (Some(InlineSpan::Bold(prev)), InlineSpan::Bold(cur)) => prev.push_str(cur),
        (Some(InlineSpan::Italic(prev)), InlineSpan::Italic(cur)) => prev.push_str(cur),
        (Some(InlineSpan::BoldItalic(prev)), InlineSpan::BoldItalic(cur)) => prev.push_str(cur),
        (Some(InlineSpan::Strikethrough(prev)), InlineSpan::Strikethrough(cur)) => prev.push_str(cur),
        (Some(InlineSpan::Code(prev)), InlineSpan::Code(cur)) => prev.push_str(cur),
        _ => target.push(span),
    }
}

fn make_styled_span(text: String, props: RunProps) -> InlineSpan {
    if props.code {
        InlineSpan::Code(text)
    } else if props.strike {
        InlineSpan::Strikethrough(text)
    } else if props.bold && props.italic {
        InlineSpan::BoldItalic(text)
    } else if props.bold {
        InlineSpan::Bold(text)
    } else if props.italic {
        InlineSpan::Italic(text)
    } else {
        InlineSpan::Text(text)
    }
}

fn is_supported_raster_image(zip_key: &str) -> bool {
    let lower = zip_key.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
}

fn extract_drawing_image<'a>(
    node: roxmltree::Node<'a, 'a>,
    rels: &HashMap<String, RelTarget>,
    zip: &ZipReader<'_>,
    embedded_images: &mut HashMap<String, Vec<u8>>,
) -> Option<(String, Option<String>)> {
    let mut rid: Option<&str> = None;
    let mut alt: Option<String> = None;
    for desc in node.descendants().filter(|n| n.is_element()) {
        let name = desc.tag_name().name();
        if name == "docPr" && alt.is_none() {
            alt = attr_local(desc, "descr")
                .or_else(|| attr_local(desc, "title"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        } else if name == "blip" && rid.is_none() {
            rid = attr_local(desc, "embed").or_else(|| attr_local(desc, "link"));
        } else if name == "imagedata" && rid.is_none() {
            rid = attr_local(desc, "id");
        }
    }
    let rel = rels.get(rid?)?;
    if rel.is_external || !is_supported_raster_image(&rel.target) {
        return None;
    }
    if !embedded_images.contains_key(&rel.target) {
        let bytes = zip.read_file(&rel.target).ok()?;
        embedded_images.insert(rel.target.clone(), bytes);
    }
    Some((rel.target.clone(), alt))
}

fn collect_run_items<'a>(
    run: roxmltree::Node<'a, 'a>,
    styles: &HashMap<String, StyleInfo>,
    rels: &HashMap<String, RelTarget>,
    zip: &ZipReader<'_>,
    embedded_images: &mut HashMap<String, Vec<u8>>,
    para_is_code: bool,
    out: &mut Vec<InlineOrBreak>,
    max_sz_half_pt: &mut Option<u32>,
    all_runs_bold: &mut bool,
    saw_text_run: &mut bool,
) {
    let mut props = find_child(run, "rPr")
        .map(|rpr| parse_rpr(rpr, Some(styles)))
        .unwrap_or_default();
    if para_is_code {
        props.code = true;
    }

    for child in run.children().filter(|c| c.is_element()) {
        match child.tag_name().name() {
            "t" | "delText" if child.tag_name().name() == "t" => {
                if let Some(txt) = child.text() {
                    if !txt.is_empty() {
                        if !txt.trim().is_empty() {
                            *saw_text_run = true;
                            if !props.bold {
                                *all_runs_bold = false;
                            }
                            if let Some(sz) = props.font_size_half_pt {
                                *max_sz_half_pt = Some(max_sz_half_pt.map_or(sz, |m| m.max(sz)));
                            }
                        }
                        out.push(InlineOrBreak::Span(make_styled_span(txt.to_string(), props)));
                    }
                }
            }
            "tab" => {
                out.push(InlineOrBreak::Span(make_styled_span("    ".to_string(), props)));
            }
            "br" => {
                if attr_local(child, "type").is_some_and(|t| t.eq_ignore_ascii_case("page")) {
                    out.push(InlineOrBreak::PageBreak);
                } else {
                    out.push(InlineOrBreak::Span(InlineSpan::Text("\n".to_string())));
                }
            }
            "cr" => {
                out.push(InlineOrBreak::Span(InlineSpan::Text("\n".to_string())));
            }
            "drawing" | "pict" => {
                if let Some((key, alt)) = extract_drawing_image(child, rels, zip, embedded_images) {
                    out.push(InlineOrBreak::Image { key, alt });
                }
            }
            _ => {}
        }
    }
}

fn collect_inline_from_container<'a>(
    container: roxmltree::Node<'a, 'a>,
    styles: &HashMap<String, StyleInfo>,
    rels: &HashMap<String, RelTarget>,
    zip: &ZipReader<'_>,
    embedded_images: &mut HashMap<String, Vec<u8>>,
    para_is_code: bool,
    out: &mut Vec<InlineOrBreak>,
    max_sz_half_pt: &mut Option<u32>,
    all_runs_bold: &mut bool,
    saw_text_run: &mut bool,
) {
    for child in container.children().filter(|c| c.is_element()) {
        match child.tag_name().name() {
            "r" => collect_run_items(
                child,
                styles,
                rels,
                zip,
                embedded_images,
                para_is_code,
                out,
                max_sz_half_pt,
                all_runs_bold,
                saw_text_run,
            ),
            "hyperlink" => {
                let mut sub = Vec::new();
                collect_inline_from_container(
                    child,
                    styles,
                    rels,
                    zip,
                    embedded_images,
                    para_is_code,
                    &mut sub,
                    max_sz_half_pt,
                    all_runs_bold,
                    saw_text_run,
                );
                let mut label = String::new();
                for item in &sub {
                    if let InlineOrBreak::Span(sp) = item {
                        label.push_str(sp.as_plain_text());
                    }
                }
                let url = attr_local(child, "id")
                    .and_then(|rid| rels.get(rid))
                    .map(|r| r.target.clone())
                    .or_else(|| attr_local(child, "anchor").map(|a| format!("#{a}")))
                    .unwrap_or_default();
                if !label.is_empty() {
                    if url.is_empty() {
                        out.push(InlineOrBreak::Span(InlineSpan::Text(label)));
                    } else {
                        out.push(InlineOrBreak::Span(InlineSpan::Link { text: label, url }));
                    }
                }
            }
            "ins" | "smartTag" | "sdt" | "sdtContent" => {
                collect_inline_from_container(
                    child,
                    styles,
                    rels,
                    zip,
                    embedded_images,
                    para_is_code,
                    out,
                    max_sz_half_pt,
                    all_runs_bold,
                    saw_text_run,
                );
            }
            // Gracefully ignore deleted track changes, comments, bookmarks, and proof errors
            _ => {}
        }
    }
}

fn spans_plain_text(spans: &[InlineSpan]) -> String {
    let mut s = String::new();
    for sp in spans {
        s.push_str(sp.as_plain_text());
    }
    s
}

#[derive(Clone, Debug)]
struct PendingListItem {
    num_id: String,
    ordered: bool,
    start: u64,
    ilvl: u8,
    spans: Vec<InlineSpan>,
}

fn build_list_tree(items: &[PendingListItem], base_ilvl: u8) -> (Vec<MdListItem>, usize) {
    let mut out: Vec<MdListItem> = Vec::new();
    let mut idx = 0usize;
    while idx < items.len() {
        let cur = &items[idx];
        if cur.ilvl < base_ilvl {
            break;
        }
        if cur.ilvl > base_ilvl {
            let sub_ordered = cur.ordered;
            let sub_start = cur.start;
            let (sub_items, consumed) = build_list_tree(&items[idx..], cur.ilvl);
            if let Some(last) = out.last_mut() {
                last.sublists.push(MdBlock::List {
                    ordered: sub_ordered,
                    start: sub_start,
                    items: sub_items,
                });
            } else {
                for si in sub_items {
                    out.push(si);
                }
            }
            idx += consumed;
            continue;
        }
        out.push(MdListItem {
            task_checked: None,
            spans: cur.spans.clone(),
            sublists: Vec::new(),
        });
        idx += 1;
    }
    (out, idx)
}

fn flush_pending_list(
    pending: &mut Vec<PendingListItem>,
    blocks: &mut Vec<MdBlock>,
) {
    if pending.is_empty() {
        return;
    }
    let min_ilvl = pending.iter().map(|i| i.ilvl).min().unwrap_or(0);
    let ordered = pending[0].ordered;
    let start = pending[0].start;
    let (items, _) = build_list_tree(pending, min_ilvl);
    if !items.is_empty() {
        blocks.push(MdBlock::List {
            ordered,
            start,
            items,
        });
    }
    pending.clear();
}

fn infer_heading_from_formatting(
    keep_next: bool,
    saw_text_run: bool,
    all_runs_bold: bool,
    max_sz_half_pt: Option<u32>,
) -> Option<u8> {
    if !keep_next || !saw_text_run || !all_runs_bold {
        return None;
    }
    let sz = max_sz_half_pt?;
    if sz >= 38 {
        Some(1)
    } else if sz >= 30 {
        Some(2)
    } else if sz >= 26 {
        Some(3)
    } else if sz >= 24 {
        Some(4)
    } else {
        None
    }
}

fn parse_jc_align(ppr: roxmltree::Node<'_, '_>) -> MdAlign {
    let Some(jc) = find_child(ppr, "jc").and_then(|n| attr_local(n, "val")) else {
        return MdAlign::None;
    };
    if jc.eq_ignore_ascii_case("center") {
        MdAlign::Center
    } else if jc.eq_ignore_ascii_case("right") || jc.eq_ignore_ascii_case("end") {
        MdAlign::Right
    } else if jc.eq_ignore_ascii_case("left") || jc.eq_ignore_ascii_case("start") {
        MdAlign::Left
    } else {
        MdAlign::None
    }
}

fn parse_table_block<'a>(
    tbl: roxmltree::Node<'a, 'a>,
    styles: &HashMap<String, StyleInfo>,
    rels: &HashMap<String, RelTarget>,
    zip: &ZipReader<'_>,
    embedded_images: &mut HashMap<String, Vec<u8>>,
) -> Option<MdBlock> {
    let mut parsed_rows: Vec<(Vec<Vec<InlineSpan>>, Vec<MdAlign>)> = Vec::new();
    for tr in tbl.children().filter(|c| is_tag(*c, "tr")) {
        let mut cells: Vec<Vec<InlineSpan>> = Vec::new();
        let mut aligns: Vec<MdAlign> = Vec::new();
        for tc in tr.children().filter(|c| is_tag(*c, "tc")) {
            let mut cell_spans: Vec<InlineSpan> = Vec::new();
            let mut cell_align = MdAlign::None;
            let mut first_para = true;
            for p in tc.children().filter(|c| is_tag(*c, "p")) {
                let ppr = find_child(p, "pPr");
                if first_para {
                    if let Some(ppr_node) = ppr {
                        cell_align = parse_jc_align(ppr_node);
                    }
                }
                let pstyle = ppr
                    .and_then(|n| find_child(n, "pStyle"))
                    .and_then(|n| attr_local(n, "val"));
                let para_is_code = pstyle
                    .and_then(|id| styles.get(id))
                    .is_some_and(|st| st.is_code);

                let mut raw_items = Vec::new();
                let mut max_sz = None;
                let mut all_bold = true;
                let mut saw_text = false;
                collect_inline_from_container(
                    p,
                    styles,
                    rels,
                    zip,
                    embedded_images,
                    para_is_code,
                    &mut raw_items,
                    &mut max_sz,
                    &mut all_bold,
                    &mut saw_text,
                );
                let mut para_spans = Vec::new();
                for item in raw_items {
                    if let InlineOrBreak::Span(sp) = item {
                        push_inline_span(&mut para_spans, sp);
                    }
                }
                if !para_spans.is_empty() {
                    if !first_para && !cell_spans.is_empty() {
                        push_inline_span(&mut cell_spans, InlineSpan::Text(" ".to_string()));
                    }
                    for sp in para_spans {
                        push_inline_span(&mut cell_spans, sp);
                    }
                }
                first_para = false;
            }
            cells.push(cell_spans);
            aligns.push(cell_align);
        }
        if !cells.is_empty() {
            parsed_rows.push((cells, aligns));
        }
    }
    if parsed_rows.is_empty() {
        return None;
    }
    let (headers, alignments) = parsed_rows.remove(0);
    let rows: Vec<Vec<Vec<InlineSpan>>> = parsed_rows.into_iter().map(|(c, _)| c).collect();
    Some(MdBlock::Table {
        headers,
        alignments,
        rows,
    })
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

/// Parses a `.docx` archive from raw bytes into MicroPDF's shared `MarkdownDoc` AST + `BookMetadata`.
pub fn parse_docx_bytes(data: &[u8], fallback_title: &str) -> Result<MarkdownDoc> {
    let zip = ZipReader::from_bytes(data)?;
    let doc_path = resolve_main_document_path(&zip);
    let doc_xml = read_xml_string(&zip, &doc_path)
        .ok_or_else(|| anyhow::anyhow!("Invalid Word document: missing {doc_path}"))?;
    let xml_doc = roxmltree::Document::parse(&doc_xml)
        .map_err(|e| anyhow::anyhow!("Failed to parse Word document XML: {e}"))?;

    let (core_title, core_author, core_desc) = parse_core_props(&zip);
    let rels = parse_relationships(&zip, &doc_path);
    let styles = parse_styles(&zip);
    let numbering = parse_numbering(&zip);

    let body = xml_doc
        .descendants()
        .find(|n| is_tag(*n, "body"))
        .ok_or_else(|| anyhow::anyhow!("Invalid Word document: missing <w:body>"))?;

    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut headings: Vec<MdHeading> = Vec::new();
    let mut embedded_images: HashMap<String, Vec<u8>> = HashMap::new();
    let mut pending_list: Vec<PendingListItem> = Vec::new();
    let mut first_h1: Option<String> = None;

    for child in body.children().filter(|c| c.is_element()) {
        match child.tag_name().name() {
            "p" => {
                let ppr = find_child(child, "pPr");
                let page_break_before = ppr
                    .and_then(|n| find_child(n, "pageBreakBefore"))
                    .is_some_and(bool_prop_on);
                let keep_next = ppr
                    .and_then(|n| find_child(n, "keepNext"))
                    .is_some_and(bool_prop_on);
                let pstyle_id = ppr
                    .and_then(|n| find_child(n, "pStyle"))
                    .and_then(|n| attr_local(n, "val"));
                let style_info = pstyle_id.and_then(|id| styles.get(id));

                let mut heading_level = style_info
                    .and_then(|st| st.heading_level.or(st.outline_level))
                    .or_else(|| pstyle_id.and_then(heading_level_from_name_or_id));
                if heading_level.is_none() {
                    if let Some(ol) = ppr.and_then(|n| find_child(n, "outlineLvl")) {
                        if let Some(v) = attr_local(ol, "val").and_then(|s| s.parse::<u8>().ok()) {
                            if v < 6 {
                                heading_level = Some(v + 1);
                            }
                        }
                    }
                }

                let num_pr = ppr.and_then(|n| find_child(n, "numPr"));
                let num_id = num_pr
                    .and_then(|n| find_child(n, "numId"))
                    .and_then(|n| attr_local(n, "val"))
                    .filter(|id| *id != "0");
                let ilvl = num_pr
                    .and_then(|n| find_child(n, "ilvl"))
                    .and_then(|n| attr_local(n, "val"))
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(0);

                let para_is_quote = style_info.is_some_and(|st| st.is_quote);
                let para_is_code = style_info.is_some_and(|st| st.is_code);
                let has_bottom_border = ppr
                    .and_then(|n| find_child(n, "pBdr"))
                    .and_then(|n| find_child(n, "bottom"))
                    .is_some();

                let mut items: Vec<InlineOrBreak> = Vec::new();
                let mut max_sz_half_pt: Option<u32> = None;
                let mut all_runs_bold = true;
                let mut saw_text_run = false;
                collect_inline_from_container(
                    child,
                    &styles,
                    &rels,
                    &zip,
                    &mut embedded_images,
                    para_is_code,
                    &mut items,
                    &mut max_sz_half_pt,
                    &mut all_runs_bold,
                    &mut saw_text_run,
                );

                if page_break_before && !blocks.is_empty() {
                    flush_pending_list(&mut pending_list, &mut blocks);
                    blocks.push(MdBlock::PageBreak);
                }

                let has_any_content = items.iter().any(|it| match it {
                    InlineOrBreak::Span(sp) => !sp.as_plain_text().trim().is_empty(),
                    InlineOrBreak::PageBreak | InlineOrBreak::Image { .. } => true,
                });

                if !has_any_content {
                    if has_bottom_border {
                        flush_pending_list(&mut pending_list, &mut blocks);
                        blocks.push(MdBlock::HorizontalRule);
                    }
                    continue;
                }

                if heading_level.is_none() && num_id.is_none() {
                    heading_level = infer_heading_from_formatting(
                        keep_next,
                        saw_text_run,
                        all_runs_bold,
                        max_sz_half_pt,
                    );
                }

                let mut cur_spans: Vec<InlineSpan> = Vec::new();
                let push_paragraph_spans = |spans: Vec<InlineSpan>,
                                            blocks: &mut Vec<MdBlock>,
                                            headings: &mut Vec<MdHeading>,
                                            pending_list: &mut Vec<PendingListItem>,
                                            first_h1: &mut Option<String>| {
                    if spans.is_empty() {
                        return;
                    }
                    if let Some(nid) = num_id {
                        let lvl = numbering
                            .get(&(nid.to_string(), ilvl))
                            .copied()
                            .unwrap_or(NumberingLevel {
                                ordered: false,
                                start: 1,
                            });
                        if ilvl == 0
                            && pending_list
                                .first()
                                .is_some_and(|first| first.num_id != nid || first.ordered != lvl.ordered)
                        {
                            flush_pending_list(pending_list, blocks);
                        }
                        pending_list.push(PendingListItem {
                            num_id: nid.to_string(),
                            ordered: lvl.ordered,
                            start: lvl.start,
                            ilvl,
                            spans,
                        });
                        return;
                    }

                    flush_pending_list(pending_list, blocks);

                    if let Some(level) = heading_level {
                        let title_text = spans_plain_text(&spans).trim().to_string();
                        if !title_text.is_empty() {
                            if level == 1 && first_h1.is_none() {
                                *first_h1 = Some(title_text.clone());
                            }
                            let block_idx = blocks.len();
                            headings.push(MdHeading {
                                level,
                                title: title_text,
                                block_idx,
                            });
                        }
                        blocks.push(MdBlock::Heading { level, spans });
                    } else if para_is_quote {
                        blocks.push(MdBlock::Blockquote(vec![MdBlock::Paragraph(spans)]));
                    } else if para_is_code {
                        blocks.push(MdBlock::CodeBlock {
                            lang: None,
                            code: spans_plain_text(&spans),
                        });
                    } else {
                        blocks.push(MdBlock::Paragraph(spans));
                    }
                };

                for item in items {
                    match item {
                        InlineOrBreak::Span(sp) => push_inline_span(&mut cur_spans, sp),
                        InlineOrBreak::PageBreak => {
                            let flushed = std::mem::take(&mut cur_spans);
                            push_paragraph_spans(
                                flushed,
                                &mut blocks,
                                &mut headings,
                                &mut pending_list,
                                &mut first_h1,
                            );
                            flush_pending_list(&mut pending_list, &mut blocks);
                            blocks.push(MdBlock::PageBreak);
                        }
                        InlineOrBreak::Image { key, alt } => {
                            let flushed = std::mem::take(&mut cur_spans);
                            push_paragraph_spans(
                                flushed,
                                &mut blocks,
                                &mut headings,
                                &mut pending_list,
                                &mut first_h1,
                            );
                            flush_pending_list(&mut pending_list, &mut blocks);
                            blocks.push(MdBlock::Image {
                                path: key,
                                caption: alt,
                            });
                        }
                    }
                }
                push_paragraph_spans(
                    cur_spans,
                    &mut blocks,
                    &mut headings,
                    &mut pending_list,
                    &mut first_h1,
                );
            }
            "tbl" => {
                flush_pending_list(&mut pending_list, &mut blocks);
                if let Some(tbl_block) =
                    parse_table_block(child, &styles, &rels, &zip, &mut embedded_images)
                {
                    blocks.push(tbl_block);
                }
            }
            "sdt" => {
                // Unwrap structured document tag blocks if present
                flush_pending_list(&mut pending_list, &mut blocks);
                if let Some(content) = find_child(child, "sdtContent") {
                    for inner in content.children().filter(|c| is_tag(*c, "p")) {
                        let mut items = Vec::new();
                        let mut max_sz = None;
                        let mut all_bold = true;
                        let mut saw_text = false;
                        collect_inline_from_container(
                            inner,
                            &styles,
                            &rels,
                            &zip,
                            &mut embedded_images,
                            false,
                            &mut items,
                            &mut max_sz,
                            &mut all_bold,
                            &mut saw_text,
                        );
                        let mut spans = Vec::new();
                        for it in items {
                            if let InlineOrBreak::Span(sp) = it {
                                push_inline_span(&mut spans, sp);
                            }
                        }
                        if !spans.is_empty() {
                            blocks.push(MdBlock::Paragraph(spans));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    flush_pending_list(&mut pending_list, &mut blocks);

    if blocks.is_empty() {
        blocks.push(MdBlock::Paragraph(vec![InlineSpan::Text(
            "(Empty Word document)".to_string(),
        )]));
    }

    let title = core_title
        .or(first_h1)
        .unwrap_or_else(|| fallback_title.to_string());
    let toc = build_toc_from_headings(&headings);

    Ok(MarkdownDoc {
        title: title.clone(),
        blocks,
        headings,
        embedded_images,
        book_meta: Some(BookMetadata {
            title,
            author: core_author,
            publisher: None,
            language: None,
            description: core_desc,
            cover_image_key: None,
            toc,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse_markdown;
    use crate::zip_mini::ZipWriter;

    fn build_synthetic_docx(include_wmf: bool) -> Vec<u8> {
        let mut zw = ZipWriter::new();
        zw.add_deflated(
            "[Content_Types].xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
</Types>"#,
        );
        zw.add_deflated(
            "_rels/.rels",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
        );
        zw.add_deflated(
            "docProps/core.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>Quarterly Spec</dc:title>
  <dc:creator>Ada Lovelace</dc:creator>
  <dc:description>Architecture overview</dc:description>
</cp:coreProperties>"#,
        );
        zw.add_deflated(
            "word/styles.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
</w:styles>"#,
        );
        zw.add_deflated(
            "word/numbering.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="10">
    <w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl>
    <w:lvl w:ilvl="1"><w:numFmt w:val="decimal"/><w:start w:val="1"/></w:lvl>
  </w:abstractNum>
  <w:abstractNum w:abstractNumId="20">
    <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:start w:val="3"/></w:lvl>
  </w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>
  <w:num w:numId="2"><w:abstractNumId w:val="20"/></w:num>
</w:numbering>"#,
        );
        zw.add_deflated(
            "word/_rels/document.xml.rels",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdLink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.org/docs" TargetMode="External"/>
  <Relationship Id="rIdImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/logo.png"/>
  <Relationship Id="rIdWmf" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/legacy.wmf"/>
</Relationships>"#,
        );
        // Tiny 1x1 valid PNG
        let tiny_png: [u8; 67] = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        zw.add_stored("word/media/logo.png", &tiny_png);
        if include_wmf {
            zw.add_stored("word/media/legacy.wmf", b"WMFBLOB");
        }

        let wmf_drawing = if include_wmf {
            r#"<w:p><w:r><w:drawing><a:blip xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:embed="rIdWmf"/></w:drawing></w:r></w:p>"#
        } else {
            ""
        };

        let doc_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Executive Summary</w:t></w:r></w:p>
    <w:p>
      <w:r><w:t xml:space="preserve">Normal </w:t></w:r>
      <w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r>
      <w:r><w:t xml:space="preserve"> and </w:t></w:r>
      <w:r><w:rPr><w:i/></w:rPr><w:t>italic</w:t></w:r>
      <w:r><w:t xml:space="preserve"> and </w:t></w:r>
      <w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>both</w:t></w:r>
      <w:r><w:t xml:space="preserve"> and </w:t></w:r>
      <w:r><w:rPr><w:strike/></w:rPr><w:t>old</w:t></w:r>
      <w:r><w:t xml:space="preserve"> and </w:t></w:r>
      <w:r><w:rPr><w:rFonts w:ascii="Consolas"/></w:rPr><w:t>fn()</w:t></w:r>
      <w:r><w:t xml:space="preserve"> see </w:t></w:r>
      <w:hyperlink r:id="rIdLink"><w:r><w:t>Docs</w:t></w:r></w:hyperlink>
    </w:p>
    <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Checklist</w:t></w:r></w:p>
    <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Bullet A</w:t></w:r></w:p>
    <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Sub 1</w:t></w:r></w:p>
    <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>Step Three</w:t></w:r></w:p>
    <w:tbl>
      <w:tr>
        <w:tc><w:p><w:pPr><w:jc w:val="left"/></w:pPr><w:r><w:t>Col A</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>Col B</w:t></w:r></w:p></w:tc>
      </w:tr>
      <w:tr>
        <w:tc><w:p><w:r><w:t>1</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>2</w:t></w:r></w:p></w:tc>
      </w:tr>
    </w:tbl>
    <w:p><w:r><w:br w:type="page"/></w:r></w:p>
    <w:p><w:r><w:drawing><wp:docPr id="1" name="Logo" descr="Brand Logo"/><a:blip r:embed="rIdImg"/></w:drawing></w:r></w:p>
    {wmf_drawing}
  </w:body>
</w:document>"#
        );
        zw.add_deflated("word/document.xml", doc_xml.as_bytes());
        zw.finish()
    }

    #[test]
    fn test_docx_reader_parses_full_features_and_skips_unsupported_wmf() {
        let bytes = build_synthetic_docx(true);
        let doc = parse_docx_bytes(&bytes, "fallback.docx").expect("parse synthetic docx");

        assert_eq!(doc.title, "Quarterly Spec");
        let meta = doc.book_meta.as_ref().expect("book_meta present");
        assert_eq!(meta.author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(meta.description.as_deref(), Some("Architecture overview"));
        assert_eq!(doc.headings.len(), 2);
        assert_eq!(doc.headings[0].title, "Executive Summary");
        assert_eq!(doc.headings[0].level, 1);
        assert_eq!(doc.headings[1].title, "Checklist");
        assert_eq!(doc.headings[1].level, 2);
        assert_eq!(meta.toc.len(), 1);
        assert_eq!(meta.toc[0].children.len(), 1);

        // Verify inline formatting + hyperlink
        let has_rich_para = doc.blocks.iter().any(|b| match b {
            MdBlock::Paragraph(spans) => {
                spans.iter().any(|s| matches!(s, InlineSpan::Bold(t) if t == "bold"))
                    && spans.iter().any(|s| matches!(s, InlineSpan::Italic(t) if t == "italic"))
                    && spans.iter().any(|s| matches!(s, InlineSpan::BoldItalic(t) if t == "both"))
                    && spans.iter().any(|s| matches!(s, InlineSpan::Strikethrough(t) if t == "old"))
                    && spans.iter().any(|s| matches!(s, InlineSpan::Code(t) if t == "fn()"))
                    && spans.iter().any(|s| matches!(s, InlineSpan::Link { text, url } if text == "Docs" && url == "https://example.org/docs"))
            }
            _ => false,
        });
        assert!(has_rich_para, "expected rich inline formatting + link");

        // Verify bullet + nested ordered sublist, and separate ordered list starting at 3
        let lists: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                MdBlock::List { ordered, start, items } => Some((*ordered, *start, items)),
                _ => None,
            })
            .collect();
        assert_eq!(lists.len(), 2);
        assert!(!lists[0].0);
        assert_eq!(lists[0].2[0].sublists.len(), 1);
        assert!(lists[1].0);
        assert_eq!(lists[1].1, 3);

        // Verify table with alignments
        let has_table = doc.blocks.iter().any(|b| match b {
            MdBlock::Table { headers, alignments, rows } => {
                headers.len() == 2
                    && alignments == &[MdAlign::Left, MdAlign::Right]
                    && rows.len() == 1
            }
            _ => false,
        });
        assert!(has_table, "expected table with alignments");

        // Verify page break and PNG image (WMF skipped)
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::PageBreak)));
        assert!(doc.blocks.iter().any(|b| matches!(
            b,
            MdBlock::Image { path, caption } if path == "word/media/logo.png" && caption.as_deref() == Some("Brand Logo")
        )));
        assert!(doc.embedded_images.contains_key("word/media/logo.png"));
        assert!(!doc.embedded_images.contains_key("word/media/legacy.wmf"));
    }

    #[test]
    fn test_docx_reader_roundtrip_from_markdown_docx_exporter() {
        let md = "\
# Annual Report

This report covers **performance**, *reliability*, ~~legacy~~, `rust`, and [Site](https://example.com).

## Key Metrics

| Metric | Status |
| :--- | ---: |
| Latency | Fast |
| Memory | Zero Idle |
";
        let source = parse_markdown(md, "report.md");
        let tmp = std::env::temp_dir().join(format!(
            "micropdf_docx_roundtrip_{}.docx",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        crate::markdown_docx::export_markdown_to_docx(&source, &tmp)
            .expect("export_markdown_to_docx");

        let bytes = std::fs::read(&tmp).expect("read exported docx");
        let parsed = parse_docx_bytes(&bytes, "report.docx").expect("parse exported docx");
        let _ = std::fs::remove_file(&tmp);

        assert_eq!(parsed.title, "Annual Report");
        assert!(parsed.headings.iter().any(|h| h.title == "Annual Report" && h.level == 1));
        assert!(parsed.headings.iter().any(|h| h.title == "Key Metrics" && h.level == 2));
        assert!(parsed.blocks.iter().any(|b| matches!(b, MdBlock::Table { .. })));
        assert!(parsed.blocks.iter().any(|b| matches!(
            b,
            MdBlock::Paragraph(spans) if spans.iter().any(|s| matches!(s, InlineSpan::Bold(t) if t == "performance"))
        )));
    }
}
