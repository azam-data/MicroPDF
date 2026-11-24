//! Pure-Rust LaTeX Math Formula Parser & CPU `softbuffer` Renderer.
//!
//! Supports `\frac{a}{b}`, `\sqrt{x}`, superscripts/subscripts (`x^2_i`),
//! Greek letters, common math operators, and simple `pmatrix`/`bmatrix` matrices
//! with zero external dependencies.

use crate::viewer::render::fill_rect_clipped;
use crate::viewer::ui_draw::{draw_line_aa, draw_text_clipped, measure_text};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MathNode {
    Text(String),
    Frac {
        num: Vec<MathNode>,
        den: Vec<MathNode>,
    },
    Sqrt(Vec<MathNode>),
    Script {
        base: Box<MathNode>,
        sup: Option<Vec<MathNode>>,
        sub: Option<Vec<MathNode>>,
    },
    Matrix {
        rows: Vec<Vec<Vec<MathNode>>>,
        open: char,
        close: char,
    },
    Group(Vec<MathNode>),
}

fn latex_symbol_to_unicode(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" | "vartheta" => "θ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" | "varpi" => "π",
        "rho" => "ρ",
        "sigma" | "varsigma" => "σ",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" | "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "sum" => "∑",
        "prod" => "∏",
        "int" => "∫",
        "oint" => "∮",
        "pm" => "±",
        "mp" => "∓",
        "times" => "×",
        "div" => "÷",
        "cdot" => "·",
        "leq" | "le" => "≤",
        "geq" | "ge" => "≥",
        "neq" | "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "sim" => "∼",
        "infty" => "∞",
        "to" | "rightarrow" => "→",
        "leftarrow" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" => "⇔",
        "partial" => "∂",
        "nabla" => "∇",
        "in" => "∈",
        "notin" => "∉",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "cup" => "∪",
        "cap" => "∩",
        "forall" => "∀",
        "exists" => "∃",
        "land" => "∧",
        "lor" => "∨",
        "neg" => "¬",
        "cdots" | "dots" => "…",
        "quad" => "  ",
        "qquad" => "    ",
        "{" => "{",
        "}" => "}",
        "%" => "%",
        "&" => "&",
        " " | "," | ";" => " ",
        _ => return None,
    })
}

fn read_braced(chars: &[char], i: &mut usize) -> String {
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }
    if *i >= chars.len() {
        return String::new();
    }
    if chars[*i] == '{' {
        *i += 1;
        let start = *i;
        let mut depth = 1usize;
        while *i < chars.len() && depth > 0 {
            if chars[*i] == '\\' && *i + 1 < chars.len() {
                *i += 2;
                continue;
            }
            if chars[*i] == '{' {
                depth += 1;
            } else if chars[*i] == '}' {
                depth -= 1;
                if depth == 0 {
                    let s: String = chars[start..*i].iter().collect();
                    *i += 1;
                    return s;
                }
            }
            *i += 1;
        }
        chars[start..].iter().collect()
    } else if chars[*i] == '\\' {
        let start = *i;
        *i += 1;
        while *i < chars.len() && chars[*i].is_ascii_alphabetic() {
            *i += 1;
        }
        chars[start..*i].iter().collect()
    } else {
        let ch = chars[*i];
        *i += 1;
        ch.to_string()
    }
}

pub fn parse_math_nodes(tex: &str) -> Vec<MathNode> {
    let chars: Vec<char> = tex.trim().chars().collect();
    let mut i = 0usize;
    let mut out: Vec<MathNode> = Vec::new();

    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' {
            i += 1;
            if i >= chars.len() {
                break;
            }
            if !chars[i].is_ascii_alphabetic() {
                let sym = chars[i].to_string();
                i += 1;
                if let Some(u) = latex_symbol_to_unicode(&sym) {
                    out.push(MathNode::Text(u.to_string()));
                } else {
                    out.push(MathNode::Text(sym));
                }
                continue;
            }
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            let cmd: String = chars[start..i].iter().collect();
            match cmd.as_str() {
                "frac" | "dfrac" | "tfrac" => {
                    let num_s = read_braced(&chars, &mut i);
                    let den_s = read_braced(&chars, &mut i);
                    out.push(MathNode::Frac {
                        num: parse_math_nodes(&num_s),
                        den: parse_math_nodes(&den_s),
                    });
                }
                "sqrt" => {
                    while i < chars.len() && chars[i].is_whitespace() {
                        i += 1;
                    }
                    if i < chars.len() && chars[i] == '[' {
                        while i < chars.len() && chars[i] != ']' {
                            i += 1;
                        }
                        if i < chars.len() {
                            i += 1;
                        }
                    }
                    let inner = read_braced(&chars, &mut i);
                    out.push(MathNode::Sqrt(parse_math_nodes(&inner)));
                }
                "begin" => {
                    let env = read_braced(&chars, &mut i);
                    let end_tag = format!("\\end{{{env}}}");
                    let rest: String = chars[i..].iter().collect();
                    if let Some(pos) = rest.find(&end_tag) {
                        let body = &rest[..pos];
                        i += rest[..pos].chars().count() + end_tag.chars().count();
                        let (open, close) = match env.as_str() {
                            "bmatrix" => ('[', ']'),
                            "vmatrix" => ('|', '|'),
                            "Bmatrix" => ('{', '}'),
                            _ => ('(', ')'),
                        };
                        let rows = body
                            .split("\\\\")
                            .map(|r| {
                                r.split('&')
                                    .map(|c| parse_math_nodes(c.trim()))
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>();
                        out.push(MathNode::Matrix { rows, open, close });
                    }
                }
                "text" | "mathrm" | "mathbf" | "mathit" | "operatorname" => {
                    let inner = read_braced(&chars, &mut i);
                    out.push(MathNode::Text(inner));
                }
                "left" | "right" => {
                    if i < chars.len() {
                        let d = chars[i];
                        i += 1;
                        if d != '.' {
                            out.push(MathNode::Text(d.to_string()));
                        }
                    }
                }
                _ => {
                    if let Some(u) = latex_symbol_to_unicode(&cmd) {
                        out.push(MathNode::Text(u.to_string()));
                    } else {
                        out.push(MathNode::Text(cmd));
                    }
                }
            }
        } else if ch == '{' {
            let inner = read_braced(&chars, &mut i);
            out.push(MathNode::Group(parse_math_nodes(&inner)));
        } else if ch == '^' || ch == '_' {
            let is_sup = ch == '^';
            i += 1;
            let arg = read_braced(&chars, &mut i);
            let parsed_arg = parse_math_nodes(&arg);
            let base = out.pop().unwrap_or_else(|| MathNode::Text(String::new()));
            match base {
                MathNode::Script { base: inner, mut sup, mut sub } => {
                    if is_sup {
                        sup = Some(parsed_arg);
                    } else {
                        sub = Some(parsed_arg);
                    }
                    out.push(MathNode::Script { base: inner, sup, sub });
                }
                other => {
                    let (sup, sub) = if is_sup {
                        (Some(parsed_arg), None)
                    } else {
                        (None, Some(parsed_arg))
                    };
                    out.push(MathNode::Script {
                        base: Box::new(other),
                        sup,
                        sub,
                    });
                }
            }
        } else if ch.is_whitespace() {
            i += 1;
            if !matches!(out.last(), Some(MathNode::Text(t)) if t.ends_with(' ')) {
                out.push(MathNode::Text(" ".to_string()));
            }
        } else {
            let mut s = String::new();
            while i < chars.len()
                && !matches!(chars[i], '\\' | '{' | '}' | '^' | '_')
                && !chars[i].is_whitespace()
            {
                s.push(chars[i]);
                i += 1;
            }
            if !s.is_empty() {
                out.push(MathNode::Text(s));
            }
        }
    }
    out
}

pub fn math_to_readable_text(tex: &str) -> String {
    fn flatten(nodes: &[MathNode], out: &mut String) {
        for n in nodes {
            match n {
                MathNode::Text(t) => out.push_str(t),
                MathNode::Frac { num, den } => {
                    out.push('(');
                    flatten(num, out);
                    out.push_str(")/(");
                    flatten(den, out);
                    out.push(')');
                }
                MathNode::Sqrt(inner) => {
                    out.push_str("√(");
                    flatten(inner, out);
                    out.push(')');
                }
                MathNode::Script { base, sup, sub } => {
                    flatten(std::slice::from_ref(base.as_ref()), out);
                    if let Some(sb) = sub {
                        out.push('_');
                        flatten(sb, out);
                    }
                    if let Some(sp) = sup {
                        out.push('^');
                        flatten(sp, out);
                    }
                }
                MathNode::Matrix { rows, open, close } => {
                    out.push(*open);
                    for (ri, r) in rows.iter().enumerate() {
                        if ri > 0 {
                            out.push_str("; ");
                        }
                        for (ci, c) in r.iter().enumerate() {
                            if ci > 0 {
                                out.push_str(", ");
                            }
                            flatten(c, out);
                        }
                    }
                    out.push(*close);
                }
                MathNode::Group(g) => flatten(g, out),
            }
        }
    }
    let mut out = String::new();
    flatten(&parse_math_nodes(tex), &mut out);
    out
}

/// Returns `(width, ascent, descent)` in pixels for `nodes`.
pub fn measure_math_nodes(nodes: &[MathNode], font_sz: f32) -> (f32, f32, f32) {
    let mut w = 0.0f32;
    let mut asc = font_sz * 0.85;
    let mut desc = font_sz * 0.28;
    for n in nodes {
        let (nw, na, nd) = measure_one_node(n, font_sz);
        w += nw;
        asc = asc.max(na);
        desc = desc.max(nd);
    }
    (w.max(4.0), asc, desc)
}

fn measure_one_node(node: &MathNode, font_sz: f32) -> (f32, f32, f32) {
    match node {
        MathNode::Text(t) => (measure_text(t, font_sz, false), font_sz * 0.85, font_sz * 0.28),
        MathNode::Group(g) => measure_math_nodes(g, font_sz),
        MathNode::Frac { num, den } => {
            let sub_sz = (font_sz * 0.78).max(8.0);
            let (nw, na, nd) = measure_math_nodes(num, sub_sz);
            let (dw, da, dd) = measure_math_nodes(den, sub_sz);
            let w = nw.max(dw) + font_sz * 0.4;
            let asc = na + nd + font_sz * 0.30;
            let desc = da + dd + font_sz * 0.15;
            (w, asc, desc)
        }
        MathNode::Sqrt(inner) => {
            let (iw, ia, id) = measure_math_nodes(inner, font_sz);
            (iw + font_sz * 0.75, ia + font_sz * 0.25, id)
        }
        MathNode::Script { base, sup, sub } => {
            let (bw, ba, bd) = measure_one_node(base, font_sz);
            let sub_sz = (font_sz * 0.70).max(7.5);
            let mut extra_w = 0.0f32;
            let mut asc = ba;
            let mut desc = bd;
            if let Some(sp) = sup {
                let (sw, sa, sd) = measure_math_nodes(sp, sub_sz);
                extra_w = extra_w.max(sw);
                asc = asc.max(ba * 0.55 + sa + sd);
            }
            if let Some(sb) = sub {
                let (sw, sa, sd) = measure_math_nodes(sb, sub_sz);
                extra_w = extra_w.max(sw);
                desc = desc.max(bd + (sa + sd) * 0.55);
            }
            (bw + extra_w + 1.5, asc, desc)
        }
        MathNode::Matrix { rows, .. } => {
            let sub_sz = (font_sz * 0.85).max(8.5);
            let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
            let mut col_w = vec![0.0f32; ncols];
            let mut total_h = 0.0f32;
            for r in rows {
                let mut rh = sub_sz * 1.25;
                for (ci, cell) in r.iter().enumerate() {
                    let (cw, ca, cd) = measure_math_nodes(cell, sub_sz);
                    col_w[ci] = col_w[ci].max(cw);
                    rh = rh.max(ca + cd + 4.0);
                }
                total_h += rh;
            }
            let w: f32 = col_w.iter().sum::<f32>() + (ncols.saturating_sub(1) as f32 * font_sz * 0.7) + font_sz * 1.1;
            let half = total_h * 0.5;
            (w, half + font_sz * 0.3, half - font_sz * 0.1)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_math_nodes(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    mut x: f32,
    baseline_y: f32,
    nodes: &[MathNode],
    font_sz: f32,
    color: u32,
    clip_top: i32,
    clip_bot: i32,
) {
    for n in nodes {
        let (nw, _, _) = measure_one_node(n, font_sz);
        draw_one_node(buffer, buf_w, buf_h, x, baseline_y, n, font_sz, color, clip_top, clip_bot);
        x += nw;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_one_node(
    buffer: &mut [u32],
    buf_w: u32,
    buf_h: u32,
    x: f32,
    baseline_y: f32,
    node: &MathNode,
    font_sz: f32,
    color: u32,
    clip_top: i32,
    clip_bot: i32,
) {
    match node {
        MathNode::Text(t) => {
            let y_top = (baseline_y - font_sz * 0.85).round() as i32;
            if y_top >= clip_top && y_top + font_sz.round() as i32 <= clip_bot {
                draw_text_clipped(buffer, buf_w, buf_h, x.round() as i32, y_top, buf_w as i32, t, font_sz, color, false);
            }
        }
        MathNode::Group(g) => {
            draw_math_nodes(buffer, buf_w, buf_h, x, baseline_y, g, font_sz, color, clip_top, clip_bot);
        }
        MathNode::Frac { num, den } => {
            let sub_sz = (font_sz * 0.78).max(8.0);
            let (nw, _, nd) = measure_math_nodes(num, sub_sz);
            let (dw, da, _) = measure_math_nodes(den, sub_sz);
            let total_w = nw.max(dw) + font_sz * 0.4;
            let bar_y = baseline_y - font_sz * 0.30;
            if bar_y.round() as i32 >= clip_top && (bar_y.round() as i32) < clip_bot {
                fill_rect_clipped(buffer, buf_w, buf_h, x.round() as i32 + 2, bar_y.round() as i32, total_w.round().max(4.0) as u32 - 4, 1, color);
            }
            let num_x = x + (total_w - nw) * 0.5;
            let den_x = x + (total_w - dw) * 0.5;
            draw_math_nodes(buffer, buf_w, buf_h, num_x, bar_y - nd - 2.0, num, sub_sz, color, clip_top, clip_bot);
            draw_math_nodes(buffer, buf_w, buf_h, den_x, bar_y + da + 2.0, den, sub_sz, color, clip_top, clip_bot);
        }
        MathNode::Sqrt(inner) => {
            let (iw, ia, id) = measure_math_nodes(inner, font_sz);
            let top_y = baseline_y - ia - font_sz * 0.16;
            let bot_y = baseline_y + id * 0.6;
            let rx = x + font_sz * 0.55;
            if top_y.round() as i32 >= clip_top && bot_y.round() as i32 <= clip_bot {
                draw_line_aa(buffer, buf_w, buf_h, x + 1.0, baseline_y - font_sz * 0.25, x + font_sz * 0.28, bot_y, 1.3, color);
                draw_line_aa(buffer, buf_w, buf_h, x + font_sz * 0.28, bot_y, rx, top_y, 1.3, color);
                draw_line_aa(buffer, buf_w, buf_h, rx, top_y, rx + iw + 3.0, top_y, 1.3, color);
            }
            draw_math_nodes(buffer, buf_w, buf_h, rx + 2.0, baseline_y, inner, font_sz, color, clip_top, clip_bot);
        }
        MathNode::Script { base, sup, sub } => {
            let (bw, ba, bd) = measure_one_node(base, font_sz);
            draw_one_node(buffer, buf_w, buf_h, x, baseline_y, base, font_sz, color, clip_top, clip_bot);
            let sub_sz = (font_sz * 0.70).max(7.5);
            if let Some(sp) = sup {
                draw_math_nodes(buffer, buf_w, buf_h, x + bw + 1.0, baseline_y - ba * 0.52, sp, sub_sz, color, clip_top, clip_bot);
            }
            if let Some(sb) = sub {
                draw_math_nodes(buffer, buf_w, buf_h, x + bw + 1.0, baseline_y + bd * 0.95, sb, sub_sz, color, clip_top, clip_bot);
            }
        }
        MathNode::Matrix { rows, open, close } => {
            let sub_sz = (font_sz * 0.85).max(8.5);
            let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
            let mut col_w = vec![0.0f32; ncols];
            let mut row_h = vec![sub_sz * 1.25; rows.len()];
            let mut total_h = 0.0f32;
            for (ri, r) in rows.iter().enumerate() {
                for (ci, cell) in r.iter().enumerate() {
                    let (cw, ca, cd) = measure_math_nodes(cell, sub_sz);
                    col_w[ci] = col_w[ci].max(cw);
                    row_h[ri] = row_h[ri].max(ca + cd + 4.0);
                }
                total_h += row_h[ri];
            }
            let total_w: f32 = col_w.iter().sum::<f32>() + (ncols.saturating_sub(1) as f32 * font_sz * 0.7) + font_sz * 1.1;
            let top_y = baseline_y - total_h * 0.5 - font_sz * 0.25;
            let bot_y = top_y + total_h;
            if top_y.round() as i32 >= clip_top && bot_y.round() as i32 <= clip_bot {
                let lx = x + font_sz * 0.25;
                let rx = x + total_w - font_sz * 0.25;
                draw_line_aa(buffer, buf_w, buf_h, lx, top_y, lx, bot_y, 1.3, color);
                draw_line_aa(buffer, buf_w, buf_h, rx, top_y, rx, bot_y, 1.3, color);
                if *open == '[' {
                    draw_line_aa(buffer, buf_w, buf_h, lx, top_y, lx + 4.0, top_y, 1.3, color);
                    draw_line_aa(buffer, buf_w, buf_h, lx, bot_y, lx + 4.0, bot_y, 1.3, color);
                    draw_line_aa(buffer, buf_w, buf_h, rx - 4.0, top_y, rx, top_y, 1.3, color);
                    draw_line_aa(buffer, buf_w, buf_h, rx - 4.0, bot_y, rx, bot_y, 1.3, color);
                }
                let _ = close;
            }
            let mut cy = top_y;
            for (ri, r) in rows.iter().enumerate() {
                let mut cx = x + font_sz * 0.55;
                for (ci, cell) in r.iter().enumerate() {
                    let (cw, _, _) = measure_math_nodes(cell, sub_sz);
                    draw_math_nodes(buffer, buf_w, buf_h, cx + (col_w[ci] - cw) * 0.5, cy + row_h[ri] * 0.72, cell, sub_sz, color, clip_top, clip_bot);
                    cx += col_w[ci] + font_sz * 0.7;
                }
                cy += row_h[ri];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latex_math_parser_frac_sqrt_greek_scripts() {
        let nodes = parse_math_nodes(r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a} + \alpha_i");
        assert!(matches!(nodes.first(), Some(MathNode::Frac { .. })));
        let readable = math_to_readable_text(r"\frac{\alpha}{\beta} + x^2_i");
        assert!(readable.contains('α'));
        assert!(readable.contains('β'));
        assert!(readable.contains("^2"));
    }
}
