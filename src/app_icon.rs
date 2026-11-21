//! Pure-Rust supersampled vector renderer for the MicroPDF application icon
//! (used at runtime for the window titlebar & Windows taskbar icon, and to generate `assets/micropdf.ico`).

use winit::window::Icon;

/// Renders an anti-aliased (4×4 supersampled) RGBA8888 icon of `size × size` pixels.
/// Design: royal-blue rounded tile containing a crisp white document sheet with a folded
/// top-right corner, subtle content lines, and a bold red PDF accent bar at the bottom.
pub fn render_app_icon_rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let inv = 1.0 / (size as f32);

    let sample_at = |u: f32, v: f32| -> [f32; 4] {
        // Outer rounded-square tile: [0.05..0.95] x [0.05..0.95], corner radius 0.20
        let cx = (u - 0.5).abs();
        let cy = (v - 0.5).abs();
        let half = 0.45f32;
        let rad = 0.19f32;
        let qx = (cx - (half - rad)).max(0.0);
        let qy = (cy - (half - rad)).max(0.0);
        let tile_dist = (qx * qx + qy * qy).sqrt() - rad;
        if tile_dist > 0.0 {
            return [0.0, 0.0, 0.0, 0.0];
        }

        // Subtle vertical royal-blue gradient (#3B82F6 top -> #1D4ED8 bottom)
        let t = ((v - 0.05) / 0.90).clamp(0.0, 1.0);
        let mut r = 59.0 * (1.0 - t) + 29.0 * t;
        let mut g = 130.0 * (1.0 - t) + 78.0 * t;
        let mut b = 246.0 * (1.0 - t) + 216.0 * t;

        // Subtle crisp inner highlight rim along top/edges
        if tile_dist > -0.022 {
            r = (r + 22.0).min(255.0);
            g = (g + 26.0).min(255.0);
            b = (b + 18.0).min(255.0);
        }

        // Inner document sheet: [0.22..0.78] x [0.15..0.85] with folded top-right corner
        let dl = 0.22f32;
        let dr = 0.78f32;
        let dt = 0.15f32;
        let db = 0.85f32;
        let fold = 0.17f32;

        // Soft drop shadow beneath sheet
        let in_shadow = u >= dl + 0.02
            && u <= dr + 0.025
            && v >= dt + 0.03
            && v <= db + 0.03
            && (u - (dr - fold) + (dt + fold - v)) <= fold + 0.035;
        if in_shadow {
            r *= 0.78;
            g *= 0.78;
            b *= 0.78;
        }

        let in_doc_box = u >= dl && u <= dr && v >= dt && v <= db;
        let cut_corner = u > dr - fold && v < dt + fold && ((u - (dr - fold)) > (v - dt));
        if in_doc_box && !cut_corner {
            // Crisp white paper sheet
            r = 250.0;
            g = 252.0;
            b = 255.0;

            // Folded corner flap (soft sky-blue triangle)
            let in_fold_flap = u >= dr - fold && v <= dt + fold;
            if in_fold_flap {
                r = 191.0;
                g = 219.0;
                b = 254.0;
            }

            // Two crisp slate text bars in the upper body of the page
            let line1 = u >= 0.30 && u <= 0.56 && v >= 0.34 && v <= 0.395;
            let line2 = u >= 0.30 && u <= 0.68 && v >= 0.45 && v <= 0.505;
            if line1 || line2 {
                r = 148.0;
                g = 163.0;
                b = 184.0;
            }

            // Signature bold crimson PDF ribbon across lower sheet
            let in_ribbon = u >= 0.25 && u <= 0.75 && v >= 0.585 && v <= 0.785;
            if in_ribbon {
                // Rounded corners on the red badge
                let rcx = (u - 0.50).abs();
                let rcy = (v - 0.685).abs();
                let rqx = (rcx - (0.25 - 0.036)).max(0.0);
                let rqy = (rcy - (0.10 - 0.036)).max(0.0);
                if (rqx * rqx + rqy * rqy).sqrt() <= 0.036 {
                    // Rich vibrant red gradient
                    let rt = ((v - 0.585) / 0.20).clamp(0.0, 1.0);
                    r = 239.0 - 18.0 * rt;
                    g = 62.0 - 16.0 * rt;
                    b = 62.0 - 14.0 * rt;

                    // Crisp white geometric vector "P D F" lettering (v in [0.625, 0.745])
                    let in_p_stem = u >= 0.312 && u <= 0.337 && v >= 0.625 && v <= 0.745;
                    let p_dx = (u - 0.372).max(0.0);
                    let p_dy = v - 0.661;
                    let p_d = (p_dx * p_dx + p_dy * p_dy).sqrt();
                    let in_p_bowl = u >= 0.335 && p_d <= 0.036 && !(u >= 0.337 && p_d < 0.014);
                    let in_p = in_p_stem || in_p_bowl;

                    let in_d_stem = u >= 0.440 && u <= 0.465 && v >= 0.625 && v <= 0.745;
                    let d_dx = (u - 0.486).max(0.0);
                    let d_dy = v - 0.685;
                    let d_outer = (d_dx * d_dx) / (0.062 * 0.062) + (d_dy * d_dy) / (0.060 * 0.060) <= 1.0;
                    let d_inner = u >= 0.465
                        && ((d_dx * d_dx) / (0.037 * 0.037) + (d_dy * d_dy) / (0.036 * 0.036) < 1.0);
                    let in_d_arch = u >= 0.463 && d_outer && !d_inner;
                    let in_d = in_d_stem || in_d_arch;

                    let in_f_stem = u >= 0.580 && u <= 0.605 && v >= 0.625 && v <= 0.745;
                    let in_f_top = u >= 0.580 && u <= 0.674 && v >= 0.625 && v <= 0.649;
                    let in_f_mid = u >= 0.580 && u <= 0.656 && v >= 0.674 && v <= 0.697;
                    let in_f = in_f_stem || in_f_top || in_f_mid;

                    if in_p || in_d || in_f {
                        r = 255.0;
                        g = 255.0;
                        b = 255.0;
                    }
                }
            }
        }

        [r, g, b, 255.0]
    };

    for y in 0..size {
        for x in 0..size {
            let mut acc_r = 0.0f32;
            let mut acc_g = 0.0f32;
            let mut acc_b = 0.0f32;
            let mut acc_a = 0.0f32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let u = (x as f32 + (sx as f32 + 0.5) * 0.25) * inv;
                    let v = (y as f32 + (sy as f32 + 0.5) * 0.25) * inv;
                    let [r, g, b, a] = sample_at(u, v);
                    let w = a / 255.0;
                    acc_r += r * w;
                    acc_g += g * w;
                    acc_b += b * w;
                    acc_a += a;
                }
            }
            let a = (acc_a / 16.0).clamp(0.0, 255.0);
            let idx = ((y * size + x) * 4) as usize;
            if a > 0.5 {
                let w_sum = acc_a / 255.0;
                out[idx] = (acc_r / w_sum).round().clamp(0.0, 255.0) as u8;
                out[idx + 1] = (acc_g / w_sum).round().clamp(0.0, 255.0) as u8;
                out[idx + 2] = (acc_b / w_sum).round().clamp(0.0, 255.0) as u8;
                out[idx + 3] = a.round() as u8;
            }
        }
    }
    out
}

/// Builds a crisp 48×48 `winit::window::Icon` for the window titlebar and OS taskbar.
pub fn build_window_icon() -> Option<Icon> {
    let size = 48u32;
    let rgba = render_app_icon_rgba(size);
    Icon::from_rgba(rgba, size, size).ok()
}

/// Builds a multi-resolution Windows `.ico` file (16×16, 32×32, 48×48, 256×256 PNG-in-ICO).
#[allow(dead_code)]
pub fn build_windows_ico_bytes() -> Vec<u8> {
    let sizes: &[u32] = &[16, 32, 48, 256];
    let mut pngs: Vec<(u32, Vec<u8>)> = Vec::with_capacity(sizes.len());
    for &sz in sizes {
        let rgba = render_app_icon_rgba(sz);
        let img = image::RgbaImage::from_raw(sz, sz, rgba).expect("valid icon dimensions");
        let mut cursor = std::io::Cursor::new(Vec::new());
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode icon png");
        pngs.push((sz, cursor.into_inner()));
    }

    let count = pngs.len() as u16;
    let header_size = 6 + (pngs.len() * 16);
    let mut out = Vec::with_capacity(header_size + pngs.iter().map(|(_, p)| p.len()).sum::<usize>());

    // ICONDIR header
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = 1 (ICO)
    out.extend_from_slice(&count.to_le_bytes());

    let mut offset = header_size as u32;
    for (sz, png) in &pngs {
        let dim = if *sz >= 256 { 0u8 } else { *sz as u8 };
        out.push(dim); // width
        out.push(dim); // height
        out.push(0); // palette count
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in pngs {
        out.extend_from_slice(&png);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_icon_and_ico_generation() {
        let rgba = render_app_icon_rgba(48);
        assert_eq!(rgba.len(), 48 * 48 * 4);
        assert!(build_window_icon().is_some());
        let ico = build_windows_ico_bytes();
        assert!(ico.len() > 200);
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
    }
}
