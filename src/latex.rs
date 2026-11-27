//! Pure-Rust Portable LaTeX (`.tex`) Parser & Complex-Graph Failsafe Guard.
//!
//! Converts standard LaTeX documents (sections, formatted prose, math formulas,
//! tables, lists, code blocks, and `\includegraphics` images) into `MarkdownDoc`.
//! Any `.tex` file containing complex vector-graph environments (TikZ, PGFPlots,
//! CircuiTikZ, PSTricks, Asymptote) is rejected cleanly by `check_latex_complex_graphs`.

use crate::markdown::{spans_to_plain_text, InlineSpan, MarkdownDoc, MdAlign, MdBlock, MdHeading, MdListItem};
use std::path::Path;

const COMPLEX_GRAPH_ENVS: &[(&str, &str)] = &[
    ("\\begin{tikzpicture}", "TikZ"),
    ("\\begin{pgfpicture}", "PGF"),
    ("\\begin{circuitikz}", "CircuiTikZ"),
    ("\\begin{pspicture}", "PSTricks"),
    ("\\begin{asy}", "Asymptote"),
    ("\\begin{axis}", "PGFPlots"),
    ("\\begin{semilogxaxis}", "PGFPlots"),
    ("\\begin{semilogyaxis}", "PGFPlots"),
    ("\\begin{loglogaxis}", "PGFPlots"),
];

/// Returns `Some(kind)` if the `.tex` source contains complex vector-graph environments
/// that cannot be rendered without a full TeX engine.
pub fn check_latex_complex_graphs(src: &str) -> Option<&'static str> {
    let lower = src.to_ascii_lowercase();
    for (pat, kind) in COMPLEX_GRAPH_ENVS {
        if lower.contains(pat) {
            return Some(kind);
        }
    }
    None
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let mut escaped = false;
        let mut cut = line.len();
        for (idx, ch) in line.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == '%' {
                cut = idx;
                break;
            }
        }
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

fn extract_braced_at(s: &str, mut pos: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    if pos < bytes.len() && bytes[pos] == b'[' {
        let mut depth = 1usize;
        pos += 1;
        while pos < bytes.len() && depth > 0 {
            if bytes[pos] == b'[' {
                depth += 1;
            } else if bytes[pos] == b']' {
                depth -= 1;
            }
            pos += 1;
        }
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
    }
    if pos >= bytes.len() || bytes[pos] != b'{' {
        return None;
    }
    pos += 1;
    let start = pos;
    let mut depth = 1usize;
    while pos < bytes.len() && depth > 0 {
        if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
            pos += 2;
            continue;
        }
        if bytes[pos] == b'{' {
            depth += 1;
        } else if bytes[pos] == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some((s[start..pos].to_string(), pos + 1));
            }
        }
        pos += 1;
    }
    None
}

fn parse_inline_tex(src: &str) -> Vec<InlineSpan> {
    let mut out: Vec<InlineSpan> = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;

    let flush_buf = |buf: &mut String, out: &mut Vec<InlineSpan>| {
        if !buf.is_empty() {
            let clean = std::mem::take(buf)
                .replace("``", "\"")
                .replace("''", "\"")
                .replace('~', " ");
            if !clean.is_empty() {
                if let Some(InlineSpan::Text(prev)) = out.last_mut() {
                    prev.push_str(&clean);
                } else {
                    out.push(InlineSpan::Text(clean));
                }
            }
        }
    };

    while i < src.len() {
        let rest = &src[i..];
        if rest.starts_with("\\(") {
            if let Some(end) = rest[2..].find("\\)") {
                flush_buf(&mut buf, &mut out);
                let math = rest[2..2 + end].trim().to_string();
                if !math.is_empty() {
                    out.push(InlineSpan::Math(math));
                }
                i += 2 + end + 2;
                continue;
            }
        }
        if rest.starts_with('$') && !rest.starts_with("$$") {
            if let Some(end) = rest[1..].find('$') {
                flush_buf(&mut buf, &mut out);
                let math = rest[1..1 + end].trim().to_string();
                if !math.is_empty() {
                    out.push(InlineSpan::Math(math));
                }
                i += 1 + end + 1;
                continue;
            }
        }
        if rest.starts_with("\\\\") {
            buf.push('\n');
            i += 2;
            continue;
        }
        if rest.starts_with('\\') {
            if let Some(ch) = rest[1..].chars().next() {
                if matches!(ch, '%' | '$' | '&' | '#' | '_' | '{' | '}') {
                    buf.push(ch);
                    i += 1 + ch.len_utf8();
                    continue;
                }
            }
            let cmd_len = rest[1..]
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(rest.len() - 1);
            let cmd = &rest[1..1 + cmd_len];
            let after_cmd = i + 1 + cmd_len;
            match cmd {
                "textbf" | "mathbf" => {
                    if let Some((arg, next_i)) = extract_braced_at(src, after_cmd) {
                        flush_buf(&mut buf, &mut out);
                        out.push(InlineSpan::Bold(spans_to_plain_text(&parse_inline_tex(&arg))));
                        i = next_i;
                        continue;
                    }
                }
                "emph" | "textit" | "mathit" => {
                    if let Some((arg, next_i)) = extract_braced_at(src, after_cmd) {
                        flush_buf(&mut buf, &mut out);
                        out.push(InlineSpan::Italic(spans_to_plain_text(&parse_inline_tex(&arg))));
                        i = next_i;
                        continue;
                    }
                }
                "texttt" => {
                    if let Some((arg, next_i)) = extract_braced_at(src, after_cmd) {
                        flush_buf(&mut buf, &mut out);
                        out.push(InlineSpan::Code(arg));
                        i = next_i;
                        continue;
                    }
                }
                "sout" => {
                    if let Some((arg, next_i)) = extract_braced_at(src, after_cmd) {
                        flush_buf(&mut buf, &mut out);
                        out.push(InlineSpan::Strikethrough(arg));
                        i = next_i;
                        continue;
                    }
                }
                "href" => {
                    if let Some((url, next_i)) = extract_braced_at(src, after_cmd) {
                        if let Some((label, next2)) = extract_braced_at(src, next_i) {
                            flush_buf(&mut buf, &mut out);
                            out.push(InlineSpan::Link { text: label, url });
                            i = next2;
                            continue;
                        }
                    }
                }
                "url" => {
                    if let Some((url, next_i)) = extract_braced_at(src, after_cmd) {
                        flush_buf(&mut buf, &mut out);
                        out.push(InlineSpan::Link { text: url.clone(), url });
                        i = next_i;
                        continue;
                    }
                }
                "cite" | "ref" | "eqref" | "label" => {
                    if let Some((arg, next_i)) = extract_braced_at(src, after_cmd) {
                        if cmd == "eqref" {
                            buf.push_str(&format!("\u{27E8}eqref:{arg}\u{27E9}"));
                        } else if cmd == "ref" {
                            buf.push_str(&format!("\u{27E8}ref:{arg}\u{27E9}"));
                        } else if cmd != "label" {
                            buf.push('[');
                            buf.push_str(&arg);
                            buf.push(']');
                        }
                        i = next_i;
                        continue;
                    }
                }
                _ => {
                    i = after_cmd;
                    if i < src.len() && src[i..].starts_with(' ') {
                        i += 1;
                    }
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        if ch != '{' && ch != '}' {
            buf.push(ch);
        }
        i += ch.len_utf8();
    }
    flush_buf(&mut buf, &mut out);
    out
}

fn strip_and_record_eq_labels(
    body: &str,
    eq_num: usize,
    labels: &mut std::collections::HashMap<String, usize>,
) -> String {
    let mut out = String::with_capacity(body.len());
    let mut i = 0usize;
    while i < body.len() {
        if body[i..].starts_with("\\label") {
            if let Some((lbl, next_i)) = extract_braced_at(body, i + "\\label".len()) {
                let k = lbl.trim().to_string();
                if !k.is_empty() {
                    labels.insert(k, eq_num);
                }
                i = next_i;
                continue;
            }
        }
        let ch = body[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out.trim().to_string()
}

fn parse_env_block(
    env: &str,
    body: &str,
    base_dir: Option<&Path>,
    eq_counter: &mut usize,
    eq_labels: &mut std::collections::HashMap<String, usize>,
) -> Option<MdBlock> {
    match env {
        "equation" | "align" | "gather" => {
            *eq_counter += 1;
            let n = *eq_counter;
            let clean = strip_and_record_eq_labels(body, n, eq_labels);
            Some(MdBlock::MathBlock(format!("{clean}\\eqno({n})")))
        }
        "equation*" | "align*" | "gather*" | "displaymath" => {
            let clean = strip_and_record_eq_labels(body, 0, &mut std::collections::HashMap::new());
            Some(MdBlock::MathBlock(clean))
        }
        "verbatim" | "lstlisting" => Some(MdBlock::CodeBlock {
            lang: if env == "lstlisting" { Some("code".to_string()) } else { None },
            code: body.trim_matches('\n').to_string(),
        }),
        "quote" | "quotation" => {
            let inner = parse_latex_body(body, base_dir, eq_counter, eq_labels);
            Some(MdBlock::Blockquote(inner))
        }
        "itemize" | "enumerate" => {
            let ordered = env == "enumerate";
            let mut items = Vec::new();
            for chunk in body.split("\\item").skip(1) {
                let txt = chunk.trim();
                if !txt.is_empty() {
                    items.push(MdListItem {
                        task_checked: None,
                        spans: parse_inline_tex(txt),
                        sublists: Vec::new(),
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
        "tabular" => {
            let mut trimmed = body.trim();
            if trimmed.starts_with('{') {
                if let Some((_, after)) = extract_braced_at(trimmed, 0) {
                    trimmed = trimmed[after..].trim();
                }
            }
            let mut parsed_rows: Vec<Vec<Vec<InlineSpan>>> = Vec::new();
            for raw_row in trimmed.split("\\\\") {
                let cleaned = raw_row
                    .replace("\\hline", "")
                    .replace("\\toprule", "")
                    .replace("\\midrule", "")
                    .replace("\\bottomrule", "");
                let r = cleaned.trim();
                if r.is_empty() {
                    continue;
                }
                let cells = r
                    .split('&')
                    .map(|c| parse_inline_tex(c.trim()))
                    .collect::<Vec<_>>();
                parsed_rows.push(cells);
            }
            if parsed_rows.is_empty() {
                return None;
            }
            let headers = parsed_rows.remove(0);
            let alignments = vec![MdAlign::Left; headers.len()];
            Some(MdBlock::Table {
                headers,
                alignments,
                rows: parsed_rows,
            })
        }
        "figure" | "center" => {
            let inner = parse_latex_body(body, base_dir, eq_counter, eq_labels);
            inner.into_iter().next()
        }
        _ => None,
    }
}

fn parse_latex_body(
    src: &str,
    base_dir: Option<&Path>,
    eq_counter: &mut usize,
    eq_labels: &mut std::collections::HashMap<String, usize>,
) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut para_buf = String::new();
    let mut i = 0usize;

    let flush_para = |buf: &mut String, blocks: &mut Vec<MdBlock>| {
        let trimmed = buf.trim();
        if !trimmed.is_empty() {
            let collapsed = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
            let spans = parse_inline_tex(&collapsed);
            if !spans.is_empty() {
                blocks.push(MdBlock::Paragraph(spans));
            }
        }
        buf.clear();
    };

    while i < src.len() {
        let rest = &src[i..];
        if rest.starts_with("\n\n") || rest.starts_with("\r\n\r\n") {
            flush_para(&mut para_buf, &mut blocks);
            i += 2;
            continue;
        }
        if rest.starts_with("$$") {
            if let Some(end) = rest[2..].find("$$") {
                flush_para(&mut para_buf, &mut blocks);
                let math = rest[2..2 + end].trim().to_string();
                if !math.is_empty() {
                    blocks.push(MdBlock::MathBlock(math));
                }
                i += 2 + end + 2;
                continue;
            }
        }
        if rest.starts_with("\\[") {
            if let Some(end) = rest[2..].find("\\]") {
                flush_para(&mut para_buf, &mut blocks);
                let math = rest[2..2 + end].trim().to_string();
                if !math.is_empty() {
                    blocks.push(MdBlock::MathBlock(math));
                }
                i += 2 + end + 2;
                continue;
            }
        }
        if rest.starts_with("\\begin{") {
            if let Some((env, body_start)) = extract_braced_at(src, i + 6) {
                let end_tag = format!("\\end{{{env}}}");
                if let Some(rel_end) = src[body_start..].find(&end_tag) {
                    flush_para(&mut para_buf, &mut blocks);
                    let body = &src[body_start..body_start + rel_end];
                    if let Some(blk) = parse_env_block(&env, body, base_dir, eq_counter, eq_labels) {
                        blocks.push(blk);
                    }
                    i = body_start + rel_end + end_tag.len();
                    continue;
                }
            }
        }
        for (cmd, lvl) in [
            ("\\section*", 1u8),
            ("\\section", 1u8),
            ("\\subsection*", 2u8),
            ("\\subsection", 2u8),
            ("\\subsubsection*", 3u8),
            ("\\subsubsection", 3u8),
        ] {
            if rest.starts_with(cmd)
                && rest[cmd.len()..].chars().next().is_some_and(|c| c == '{' || c == '[' || c.is_whitespace())
            {
                if let Some((title, next_i)) = extract_braced_at(src, i + cmd.len()) {
                    flush_para(&mut para_buf, &mut blocks);
                    blocks.push(MdBlock::Heading {
                        level: lvl,
                        spans: parse_inline_tex(&title),
                    });
                    i = next_i;
                    continue;
                }
            }
        }
        if rest.starts_with("\\includegraphics") {
            if let Some((img_rel, next_i)) = extract_braced_at(src, i + "\\includegraphics".len()) {
                flush_para(&mut para_buf, &mut blocks);
                let resolved = if let Some(dir) = base_dir {
                    dir.join(img_rel.trim()).to_string_lossy().to_string()
                } else {
                    img_rel.trim().to_string()
                };
                blocks.push(MdBlock::Image {
                    path: resolved,
                    caption: None,
                });
                i = next_i;
                continue;
            }
        }
        if rest.starts_with("\\hrule") {
            flush_para(&mut para_buf, &mut blocks);
            blocks.push(MdBlock::HorizontalRule);
            i += "\\hrule".len();
            continue;
        }
        if rest.starts_with("\\maketitle") || rest.starts_with("\\tableofcontents") {
            i += if rest.starts_with("\\maketitle") { 10 } else { 16 };
            continue;
        }

        let ch = rest.chars().next().unwrap();
        para_buf.push(ch);
        i += ch.len_utf8();
    }
    flush_para(&mut para_buf, &mut blocks);
    blocks
}

pub fn parse_latex(src: &str, fallback_title: &str, base_dir: Option<&Path>) -> MarkdownDoc {
    let cleaned = strip_comments(src);
    let mut doc_title: Option<String> = None;
    let mut doc_author: Option<String> = None;

    if let Some(pos) = cleaned.find("\\title") {
        if let Some((t, _)) = extract_braced_at(&cleaned, pos + 6) {
            let plain = spans_to_plain_text(&parse_inline_tex(&t)).trim().to_string();
            if !plain.is_empty() {
                doc_title = Some(plain);
            }
        }
    }
    if let Some(pos) = cleaned.find("\\author") {
        if let Some((a, _)) = extract_braced_at(&cleaned, pos + 7) {
            let plain = spans_to_plain_text(&parse_inline_tex(&a)).trim().to_string();
            if !plain.is_empty() {
                doc_author = Some(plain);
            }
        }
    }

    let body = if let Some(start) = cleaned.find("\\begin{document}") {
        let after = start + "\\begin{document}".len();
        let end = cleaned[after..].find("\\end{document}").map(|e| after + e).unwrap_or(cleaned.len());
        &cleaned[after..end]
    } else {
        &cleaned
    };

    let mut blocks = Vec::new();
    if let Some(ref t) = doc_title {
        blocks.push(MdBlock::Heading {
            level: 1,
            spans: vec![InlineSpan::Text(t.clone())],
        });
    }
    if let Some(ref a) = doc_author {
        blocks.push(MdBlock::Paragraph(vec![InlineSpan::Italic(a.clone())]));
    }
    let mut eq_counter = 0usize;
    let mut eq_labels = std::collections::HashMap::new();
    blocks.extend(parse_latex_body(body, base_dir, &mut eq_counter, &mut eq_labels));
    resolve_eq_refs_in_blocks(&mut blocks, &eq_labels);

    let mut headings = Vec::new();
    for (idx, blk) in blocks.iter().enumerate() {
        if let MdBlock::Heading { level, spans } = blk {
            let title = spans_to_plain_text(spans).trim().to_string();
            if !title.is_empty() && *level <= 3 {
                headings.push(MdHeading {
                    level: *level,
                    title,
                    block_idx: idx,
                });
            }
        }
    }

    MarkdownDoc {
        title: doc_title.unwrap_or_else(|| fallback_title.to_string()),
        blocks,
        headings,
        embedded_images: std::collections::HashMap::new(),
        book_meta: None,
    }
}

fn resolve_eq_refs_in_text(s: &mut String, labels: &std::collections::HashMap<String, usize>) {
    if !s.contains('\u{27E8}') {
        return;
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(start) = rest.find('\u{27E8}') {
        out.push_str(&rest[..start]);
        let after = &rest[start + '\u{27E8}'.len_utf8()..];
        if let Some(end) = after.find('\u{27E9}') {
            let token = &after[..end];
            if let Some(lbl) = token.strip_prefix("eqref:") {
                if let Some(&n) = labels.get(lbl.trim()) {
                    out.push_str(&format!("({n})"));
                } else {
                    out.push_str(&format!("[{lbl}]"));
                }
            } else if let Some(lbl) = token.strip_prefix("ref:") {
                if let Some(&n) = labels.get(lbl.trim()) {
                    out.push_str(&format!("{n}"));
                } else {
                    out.push_str(&format!("[{lbl}]"));
                }
            } else {
                out.push('\u{27E8}');
                out.push_str(token);
                out.push('\u{27E9}');
            }
            rest = &after[end + '\u{27E9}'.len_utf8()..];
        } else {
            out.push('\u{27E8}');
            rest = after;
        }
    }
    out.push_str(rest);
    *s = out;
}

fn resolve_eq_refs_in_spans(spans: &mut [InlineSpan], labels: &std::collections::HashMap<String, usize>) {
    for sp in spans {
        match sp {
            InlineSpan::Text(s)
            | InlineSpan::Bold(s)
            | InlineSpan::Italic(s)
            | InlineSpan::BoldItalic(s)
            | InlineSpan::Strikethrough(s)
            | InlineSpan::Code(s)
            | InlineSpan::Math(s) => resolve_eq_refs_in_text(s, labels),
            InlineSpan::Link { text, .. } => resolve_eq_refs_in_text(text, labels),
        }
    }
}

fn resolve_eq_refs_in_blocks(blocks: &mut [MdBlock], labels: &std::collections::HashMap<String, usize>) {
    for b in blocks {
        match b {
            MdBlock::Heading { spans, .. } | MdBlock::Paragraph(spans) => {
                resolve_eq_refs_in_spans(spans, labels);
            }
            MdBlock::Blockquote(inner) => resolve_eq_refs_in_blocks(inner, labels),
            MdBlock::Table { headers, rows, .. } => {
                for h in headers {
                    resolve_eq_refs_in_spans(h, labels);
                }
                for row in rows {
                    for cell in row {
                        resolve_eq_refs_in_spans(cell, labels);
                    }
                }
            }
            MdBlock::List { items, .. } => {
                for item in items {
                    resolve_eq_refs_in_spans(&mut item.spans, labels);
                    resolve_eq_refs_in_blocks(&mut item.sublists, labels);
                }
            }
            MdBlock::CodeBlock { .. }
            | MdBlock::MathBlock(_)
            | MdBlock::Image { .. }
            | MdBlock::HorizontalRule => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latex_complex_graph_failsafe() {
        let tikz = r"\begin{document}\begin{tikzpicture}\draw (0,0) -- (1,1);\end{tikzpicture}\end{document}";
        assert_eq!(check_latex_complex_graphs(tikz), Some("TikZ"));

        let pgf = r"\begin{axis}\addplot {x^2};\end{axis}";
        assert_eq!(check_latex_complex_graphs(pgf), Some("PGFPlots"));

        let normal = r"\section{Intro} Hello $\frac{a}{b}$ and \includegraphics{fig.png}";
        assert_eq!(check_latex_complex_graphs(normal), None);
    }

    #[test]
    fn test_parse_latex_sections_math_table_and_image() {
        let tex = r#"
\title{Quantum Notes}
\author{MicroPDF}
\begin{document}
\section{Equations}
Energy is $E = mc^2$ and $\alpha + \beta$.

\[
\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}
\]

\begin{tabular}{ll}
Name & Value \\
\hline
Speed & Fast \\
\end{tabular}

\includegraphics[width=0.8\textwidth]{diagram.png}
\end{document}
"#;
        let doc = parse_latex(tex, "notes.tex", None);
        assert_eq!(doc.title, "Quantum Notes");
        assert!(doc.headings.len() >= 2);
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::MathBlock(_))));
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::Table { .. })));
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::Image { path, .. } if path == "diagram.png")));
    }

    #[test]
    fn test_latex_equation_numbers_and_eqref_resolution() {
        let tex = r#"
\begin{document}
See \eqref{eq:einstein} and \ref{eq:euler} below.

\begin{equation}\label{eq:einstein}
E = mc^2
\end{equation}

\begin{equation*}
a + b = c
\end{equation*}

\begin{align}\label{eq:euler}
e^{i\pi} + 1 = 0
\end{align}
\end{document}
"#;
        let doc = parse_latex(tex, "eq.tex", None);
        let para_txt = match &doc.blocks[0] {
            MdBlock::Paragraph(spans) => spans_to_plain_text(spans),
            other => panic!("expected paragraph, got {other:?}"),
        };
        assert!(para_txt.contains("(1)"), "expected (1) in '{para_txt}'");
        assert!(para_txt.contains("2"), "expected 2 in '{para_txt}'");
        assert!(matches!(&doc.blocks[1], MdBlock::MathBlock(t) if t == "E = mc^2\\eqno(1)"));
        assert!(matches!(&doc.blocks[2], MdBlock::MathBlock(t) if t == "a + b = c"));
        assert!(matches!(&doc.blocks[3], MdBlock::MathBlock(t) if t.ends_with("\\eqno(2)")));
    }
}
