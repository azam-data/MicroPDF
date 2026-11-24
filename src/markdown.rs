//! Lightweight GFM Markdown AST parser backed by `pulldown-cmark`.
//!
//! Converts `.md` / `.markdown` text into a compact typed `MarkdownDoc` AST
//! (zero pixel buffers, minimal heap footprint) shared by the CPU Markdown Reader,
//! Styled PDF Exporter, and Styled Word (`.docx`) Exporter.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MdAlign {
    None,
    Left,
    Center,
    Right,
}

impl From<Alignment> for MdAlign {
    fn from(a: Alignment) -> Self {
        match a {
            Alignment::None => MdAlign::None,
            Alignment::Left => MdAlign::Left,
            Alignment::Center => MdAlign::Center,
            Alignment::Right => MdAlign::Right,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineSpan {
    Text(String),
    Bold(String),
    Italic(String),
    BoldItalic(String),
    Strikethrough(String),
    Code(String),
    Link { text: String, url: String },
}

impl InlineSpan {
    pub fn as_plain_text(&self) -> &str {
        match self {
            InlineSpan::Text(s)
            | InlineSpan::Bold(s)
            | InlineSpan::Italic(s)
            | InlineSpan::BoldItalic(s)
            | InlineSpan::Strikethrough(s)
            | InlineSpan::Code(s) => s,
            InlineSpan::Link { text, .. } => text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdListItem {
    pub task_checked: Option<bool>,
    pub spans: Vec<InlineSpan>,
    pub sublists: Vec<MdBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdBlock {
    Heading {
        level: u8,
        spans: Vec<InlineSpan>,
    },
    Paragraph(Vec<InlineSpan>),
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    Blockquote(Vec<MdBlock>),
    Table {
        headers: Vec<Vec<InlineSpan>>,
        alignments: Vec<MdAlign>,
        rows: Vec<Vec<Vec<InlineSpan>>>,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<MdListItem>,
    },
    HorizontalRule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdHeading {
    pub level: u8,
    pub title: String,
    /// Index into `MarkdownDoc.blocks` (for top-level headings).
    pub block_idx: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownDoc {
    pub title: String,
    pub blocks: Vec<MdBlock>,
    pub headings: Vec<MdHeading>,
}

#[derive(Default)]
struct InlineCtx {
    bold: usize,
    italic: usize,
    strike: usize,
    link_url: Option<String>,
    link_text: String,
}

impl InlineCtx {
    fn push_text(&mut self, target: &mut Vec<InlineSpan>, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.link_url.is_some() {
            self.link_text.push_str(text);
            return;
        }
        let span = if self.strike > 0 {
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
        // Coalesce adjacent identical simple spans to keep AST compact
        match (target.last_mut(), &span) {
            (Some(InlineSpan::Text(prev)), InlineSpan::Text(cur)) => prev.push_str(cur),
            (Some(InlineSpan::Bold(prev)), InlineSpan::Bold(cur)) => prev.push_str(cur),
            (Some(InlineSpan::Italic(prev)), InlineSpan::Italic(cur)) => prev.push_str(cur),
            (Some(InlineSpan::BoldItalic(prev)), InlineSpan::BoldItalic(cur)) => prev.push_str(cur),
            (Some(InlineSpan::Strikethrough(prev)), InlineSpan::Strikethrough(cur)) => {
                prev.push_str(cur)
            }
            _ => target.push(span),
        }
    }
}

pub fn spans_to_plain_text(spans: &[InlineSpan]) -> String {
    let mut out = String::new();
    for s in spans {
        out.push_str(s.as_plain_text());
    }
    out
}

pub fn parse_markdown(src: &str, fallback_title: &str) -> MarkdownDoc {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(src, opts);

    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut headings: Vec<MdHeading> = Vec::new();
    let mut first_h1: Option<String> = None;

    let mut inline = InlineCtx::default();
    let mut cur_spans: Vec<InlineSpan> = Vec::new();

    // Active block states
    let mut in_heading: Option<u8> = None;
    let mut in_paragraph = false;
    let mut in_code_block: Option<(Option<String>, String)> = None;
    let mut blockquote_stack: Vec<Vec<MdBlock>> = Vec::new();

    // Table state
    struct TableState {
        alignments: Vec<MdAlign>,
        headers: Vec<Vec<InlineSpan>>,
        rows: Vec<Vec<Vec<InlineSpan>>>,
        cur_row: Vec<Vec<InlineSpan>>,
        in_head: bool,
        in_cell: bool,
    }
    let mut table_state: Option<TableState> = None;

    // List stack
    struct ListState {
        ordered: bool,
        start: u64,
        items: Vec<MdListItem>,
        cur_item: Option<MdListItem>,
    }
    let mut list_stack: Vec<ListState> = Vec::new();

    let push_block = |blk: MdBlock,
                      blocks: &mut Vec<MdBlock>,
                      bq_stack: &mut Vec<Vec<MdBlock>>,
                      headings: &mut Vec<MdHeading>,
                      first_h1: &mut Option<String>| {
        if let Some(top_bq) = bq_stack.last_mut() {
            top_bq.push(blk);
        } else {
            let block_idx = blocks.len();
            if let MdBlock::Heading { level, ref spans } = blk {
                let title = spans_to_plain_text(spans).trim().to_string();
                if !title.is_empty() {
                    if level == 1 && first_h1.is_none() {
                        *first_h1 = Some(title.clone());
                    }
                    if level <= 3 {
                        headings.push(MdHeading {
                            level,
                            title,
                            block_idx,
                        });
                    }
                }
            }
            blocks.push(blk);
        }
    };

    for ev in parser {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    in_heading = Some(level as u8);
                    cur_spans.clear();
                }
                Tag::Paragraph => {
                    in_paragraph = true;
                    if list_stack.is_empty() {
                        cur_spans.clear();
                    } else if let Some(top) = list_stack.last_mut() {
                        if let Some(ref mut item) = top.cur_item {
                            if !item.spans.is_empty() {
                                item.spans.push(InlineSpan::Text(" ".to_string()));
                            }
                        }
                    }
                }
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => {
                            let l = info.split_whitespace().next().unwrap_or("").trim();
                            if l.is_empty() {
                                None
                            } else {
                                Some(l.to_string())
                            }
                        }
                        CodeBlockKind::Indented => None,
                    };
                    in_code_block = Some((lang, String::new()));
                }
                Tag::BlockQuote(_) => {
                    blockquote_stack.push(Vec::new());
                }
                Tag::Table(aligns) => {
                    table_state = Some(TableState {
                        alignments: aligns.into_iter().map(MdAlign::from).collect(),
                        headers: Vec::new(),
                        rows: Vec::new(),
                        cur_row: Vec::new(),
                        in_head: false,
                        in_cell: false,
                    });
                }
                Tag::TableHead => {
                    if let Some(ref mut ts) = table_state {
                        ts.in_head = true;
                        ts.cur_row.clear();
                    }
                }
                Tag::TableRow => {
                    if let Some(ref mut ts) = table_state {
                        ts.cur_row.clear();
                    }
                }
                Tag::TableCell => {
                    if let Some(ref mut ts) = table_state {
                        ts.in_cell = true;
                        cur_spans.clear();
                    }
                }
                Tag::List(start_opt) => {
                    list_stack.push(ListState {
                        ordered: start_opt.is_some(),
                        start: start_opt.unwrap_or(1),
                        items: Vec::new(),
                        cur_item: None,
                    });
                }
                Tag::Item => {
                    if let Some(top) = list_stack.last_mut() {
                        top.cur_item = Some(MdListItem {
                            task_checked: None,
                            spans: Vec::new(),
                            sublists: Vec::new(),
                        });
                        cur_spans.clear();
                    }
                }
                Tag::Strong => inline.bold += 1,
                Tag::Emphasis => inline.italic += 1,
                Tag::Strikethrough => inline.strike += 1,
                Tag::Link { dest_url, .. } => {
                    inline.link_url = Some(dest_url.to_string());
                    inline.link_text.clear();
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    if let Some(level) = in_heading.take() {
                        let spans = std::mem::take(&mut cur_spans);
                        push_block(
                            MdBlock::Heading { level, spans },
                            &mut blocks,
                            &mut blockquote_stack,
                            &mut headings,
                            &mut first_h1,
                        );
                    }
                }
                TagEnd::Paragraph => {
                    in_paragraph = false;
                    if let Some(top) = list_stack.last_mut() {
                        if let Some(ref mut item) = top.cur_item {
                            item.spans.extend(std::mem::take(&mut cur_spans));
                        }
                    } else {
                        let spans = std::mem::take(&mut cur_spans);
                        if !spans.is_empty() {
                            push_block(
                                MdBlock::Paragraph(spans),
                                &mut blocks,
                                &mut blockquote_stack,
                                &mut headings,
                                &mut first_h1,
                            );
                        }
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, mut code)) = in_code_block.take() {
                        if code.ends_with('\n') {
                            code.pop();
                        }
                        push_block(
                            MdBlock::CodeBlock { lang, code },
                            &mut blocks,
                            &mut blockquote_stack,
                            &mut headings,
                            &mut first_h1,
                        );
                    }
                }
                TagEnd::BlockQuote(_) => {
                    if let Some(inner) = blockquote_stack.pop() {
                        push_block(
                            MdBlock::Blockquote(inner),
                            &mut blocks,
                            &mut blockquote_stack,
                            &mut headings,
                            &mut first_h1,
                        );
                    }
                }
                TagEnd::TableCell => {
                    if let Some(ref mut ts) = table_state {
                        ts.in_cell = false;
                        ts.cur_row.push(std::mem::take(&mut cur_spans));
                    }
                }
                TagEnd::TableHead => {
                    if let Some(ref mut ts) = table_state {
                        ts.in_head = false;
                        ts.headers = std::mem::take(&mut ts.cur_row);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(ref mut ts) = table_state {
                        if !ts.in_head {
                            ts.rows.push(std::mem::take(&mut ts.cur_row));
                        }
                    }
                }
                TagEnd::Table => {
                    if let Some(ts) = table_state.take() {
                        push_block(
                            MdBlock::Table {
                                headers: ts.headers,
                                alignments: ts.alignments,
                                rows: ts.rows,
                            },
                            &mut blocks,
                            &mut blockquote_stack,
                            &mut headings,
                            &mut first_h1,
                        );
                    }
                }
                TagEnd::Item => {
                    if let Some(top) = list_stack.last_mut() {
                        if let Some(mut item) = top.cur_item.take() {
                            if !cur_spans.is_empty() {
                                item.spans.extend(std::mem::take(&mut cur_spans));
                            }
                            top.items.push(item);
                        }
                    }
                }
                TagEnd::List(_) => {
                    if let Some(ls) = list_stack.pop() {
                        let list_blk = MdBlock::List {
                            ordered: ls.ordered,
                            start: ls.start,
                            items: ls.items,
                        };
                        if let Some(parent) = list_stack.last_mut() {
                            if let Some(ref mut pitem) = parent.cur_item {
                                pitem.sublists.push(list_blk);
                            } else {
                                push_block(
                                    list_blk,
                                    &mut blocks,
                                    &mut blockquote_stack,
                                    &mut headings,
                                    &mut first_h1,
                                );
                            }
                        } else {
                            push_block(
                                list_blk,
                                &mut blocks,
                                &mut blockquote_stack,
                                &mut headings,
                                &mut first_h1,
                            );
                        }
                    }
                }
                TagEnd::Strong => inline.bold = inline.bold.saturating_sub(1),
                TagEnd::Emphasis => inline.italic = inline.italic.saturating_sub(1),
                TagEnd::Strikethrough => inline.strike = inline.strike.saturating_sub(1),
                TagEnd::Link => {
                    if let Some(url) = inline.link_url.take() {
                        let text = if inline.link_text.is_empty() {
                            url.clone()
                        } else {
                            std::mem::take(&mut inline.link_text)
                        };
                        cur_spans.push(InlineSpan::Link { text, url });
                    }
                }
                _ => {}
            },
            Event::Text(txt) => {
                if let Some((_, ref mut code)) = in_code_block {
                    code.push_str(&txt);
                } else {
                    inline.push_text(&mut cur_spans, &txt);
                }
            }
            Event::Code(code) => {
                if inline.link_url.is_some() {
                    inline.link_text.push_str(&code);
                } else {
                    cur_spans.push(InlineSpan::Code(code.to_string()));
                }
            }
            Event::SoftBreak => {
                if in_code_block.is_none() {
                    inline.push_text(&mut cur_spans, " ");
                }
            }
            Event::HardBreak => {
                if in_code_block.is_none() {
                    inline.push_text(&mut cur_spans, "\n");
                }
            }
            Event::Rule => {
                push_block(
                    MdBlock::HorizontalRule,
                    &mut blocks,
                    &mut blockquote_stack,
                    &mut headings,
                    &mut first_h1,
                );
            }
            Event::TaskListMarker(checked) => {
                if let Some(top) = list_stack.last_mut() {
                    if let Some(ref mut item) = top.cur_item {
                        item.task_checked = Some(checked);
                    }
                }
            }
            _ => {}
        }
        let _ = in_paragraph;
    }

    MarkdownDoc {
        title: first_h1.unwrap_or_else(|| fallback_title.to_string()),
        blocks,
        headings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gfm_markdown_blocks_and_headings() {
        let md = "\
# Main Title

Intro with **bold**, *italic*, ~~strike~~, `code`, and [link](https://example.com).

## Tasks & Lists

- [x] Done item
- [ ] Todo item
  1. Nested one
  2. Nested two

### Data Table

| Name | Count | Notes |
| :--- | :---: | ---: |
| Alpha | 42 | OK |

```rust
fn main() { println!(\"hi\"); }
```

> Quoted text

---
";
        let doc = parse_markdown(md, "fallback.md");
        assert_eq!(doc.title, "Main Title");
        assert_eq!(doc.headings.len(), 3);
        assert_eq!(doc.headings[0].level, 1);
        assert_eq!(doc.headings[1].level, 2);
        assert_eq!(doc.headings[2].level, 3);
        assert!(doc.blocks.iter().any(|b| matches!(b, MdBlock::Table { .. })));
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(b, MdBlock::CodeBlock { lang: Some(l), .. } if l == "rust")));
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(b, MdBlock::Blockquote(_))));
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(b, MdBlock::HorizontalRule)));
    }
}
