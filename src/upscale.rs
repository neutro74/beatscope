//! High-quality image resampling.
//!
//! ratatui-image's built-in scaling is fine, but small embedded thumbnails come
//! out soft when blown up. This module does a separable Lanczos-3 resample to the
//! exact target pixel size followed by a light unsharp mask, which keeps edges
//! crisp instead of mushy. It handles both up- and down-scaling correctly (the
//! filter widens to low-pass when shrinking).

use image::{DynamicImage, Rgba, RgbaImage};

const A: f32 = 3.0; // Lanczos lobes

fn lanczos(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        1.0
    } else if x < A {
        let px = std::f32::consts::PI * x;
        (px.sin() / px) * ((px / A).sin() / (px / A))
    } else {
        0.0
    }
}

/// For each destination index, the first source index and the normalized weights.
fn weights(src_len: u32, dst_len: u32) -> Vec<(i64, Vec<f32>)> {
    let scale = dst_len as f32 / src_len as f32;
    // When shrinking, stretch the kernel to act as a low-pass filter.
    let fscale = if scale < 1.0 { 1.0 / scale } else { 1.0 };
    let support = A * fscale;

    let mut out = Vec::with_capacity(dst_len as usize);
    for x in 0..dst_len {
        let center = (x as f32 + 0.5) / scale - 0.5;
        let left = (center - support).ceil() as i64;
        let right = (center + support).floor() as i64;
        let mut w = Vec::with_capacity((right - left + 1).max(1) as usize);
        let mut sum = 0.0;
        for i in left..=right {
            let val = lanczos((i as f32 - center) / fscale);
            w.push(val);
            sum += val;
        }
        if sum != 0.0 {
            for v in &mut w {
                *v /= sum;
            }
        }
        out.push((left, w));
    }
    out
}

#[inline]
fn clampi(v: i64, max: i64) -> usize {
    v.clamp(0, max) as usize
}

/// Resize `src` to exactly `dst_w` x `dst_h` with Lanczos-3 + unsharp.
pub fn resize_high_quality(src: &DynamicImage, dst_w: u32, dst_h: u32) -> DynamicImage {
    let src = src.to_rgba8();
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 || dst_w == 0 || dst_h == 0 {
        return DynamicImage::ImageRgba8(src);
    }

    // Source as planar f32 RGBA for fast accumulation.
    let src_f: Vec<[f32; 4]> = src
        .pixels()
        .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32])
        .collect();

    // --- horizontal pass: (sw x sh) -> (dst_w x sh) ---
    let xw = weights(sw, dst_w);
    let mut horiz = vec![[0.0f32; 4]; (dst_w * sh) as usize];
    let max_x = sw as i64 - 1;
    for y in 0..sh {
        let row = (y * sw) as usize;
        for x in 0..dst_w {
            let (left, ws) = &xw[x as usize];
            let mut acc = [0.0f32; 4];
            for (k, &wt) in ws.iter().enumerate() {
                let sx = clampi(left + k as i64, max_x);
                let px = &src_f[row + sx];
                acc[0] += px[0] * wt;
                acc[1] += px[1] * wt;
                acc[2] += px[2] * wt;
                acc[3] += px[3] * wt;
            }
            horiz[(y * dst_w + x) as usize] = acc;
        }
    }

    // --- vertical pass: (dst_w x sh) -> (dst_w x dst_h) ---
    let yw = weights(sh, dst_h);
    let mut out = vec![[0.0f32; 4]; (dst_w * dst_h) as usize];
    let max_y = sh as i64 - 1;
    for y in 0..dst_h {
        let (top, ws) = &yw[y as usize];
        for x in 0..dst_w {
            let mut acc = [0.0f32; 4];
            for (k, &wt) in ws.iter().enumerate() {
                let sy = clampi(top + k as i64, max_y);
                let px = &horiz[(sy as u32 * dst_w + x) as usize];
                acc[0] += px[0] * wt;
                acc[1] += px[1] * wt;
                acc[2] += px[2] * wt;
                acc[3] += px[3] * wt;
            }
            out[(y * dst_w + x) as usize] = acc;
        }
    }

    unsharp(&mut out, dst_w, dst_h, 0.45);

    let mut img = RgbaImage::new(dst_w, dst_h);
    for (i, p) in out.iter().enumerate() {
        let x = (i as u32) % dst_w;
        let y = (i as u32) / dst_w;
        img.put_pixel(
            x,
            y,
            Rgba([
                p[0].round().clamp(0.0, 255.0) as u8,
                p[1].round().clamp(0.0, 255.0) as u8,
                p[2].round().clamp(0.0, 255.0) as u8,
                p[3].round().clamp(0.0, 255.0) as u8,
            ]),
        );
    }
    DynamicImage::ImageRgba8(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};

    #[test]
    fn exact_target_dimensions() {
        let mut img = RgbaImage::new(8, 8);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = ((x + y) * 16) as u8;
            *p = Rgba([v, 255 - v, 128, 255]);
        }
        let out = resize_high_quality(&DynamicImage::ImageRgba8(img), 200, 150);
        assert_eq!(out.to_rgba8().dimensions(), (200, 150));
    }

    #[test]
    fn preserves_solid_color_and_alpha() {
        let img = RgbaImage::from_pixel(6, 6, Rgba([40, 160, 220, 255]));
        let out = resize_high_quality(&DynamicImage::ImageRgba8(img), 120, 90).to_rgba8();
        // A flat image upscaled should stay (nearly) flat with the same color.
        let c = out.get_pixel(60, 45);
        assert!((c[0] as i32 - 40).abs() <= 2, "r={}", c[0]);
        assert!((c[1] as i32 - 160).abs() <= 2, "g={}", c[1]);
        assert!((c[2] as i32 - 220).abs() <= 2, "b={}", c[2]);
        assert_eq!(c[3], 255);
    }

    #[test]
    fn sharp_edge_stays_sharp() {
        // Left half black, right half white. After upscale the transition should
        // remain steep (sharpening shouldn't smear it into a long ramp).
        let mut img = RgbaImage::new(8, 8);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let v = if x < 4 { 0 } else { 255 };
            *p = Rgba([v, v, v, 255]);
        }
        let out = resize_high_quality(&DynamicImage::ImageRgba8(img), 160, 16).to_rgba8();
        assert!(out.get_pixel(8, 8)[0] < 40, "left should be dark");
        assert!(out.get_pixel(150, 8)[0] > 215, "right should be bright");
    }
}

/// In-place unsharp mask on RGB (alpha untouched): out += amount * (out - blur),
/// where blur is a 3x3 Gaussian. Restores the crispness softened by upscaling.
fn unsharp(buf: &mut [[f32; 4]], w: u32, h: u32, amount: f32) {
    if w < 3 || h < 3 || amount <= 0.0 {
        return;
    }
    // 3x3 Gaussian kernel (1 2 1)/4 separable, applied as a copy.
    let idx = |x: u32, y: u32| (y * w + x) as usize;
    let orig: Vec<[f32; 4]> = buf.to_vec();

    let sample = |x: i64, y: i64| -> [f32; 4] {
        let xc = x.clamp(0, w as i64 - 1) as u32;
        let yc = y.clamp(0, h as i64 - 1) as u32;
        orig[idx(xc, yc)]
    };

    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut blur = [0.0f32; 3];
            // weights: corners 1, edges 2, center 4 -> /16
            let kern = [
                (-1i64, -1i64, 1.0),
                (0, -1, 2.0),
                (1, -1, 1.0),
                (-1, 0, 2.0),
                (0, 0, 4.0),
                (1, 0, 2.0),
                (-1, 1, 1.0),
                (0, 1, 2.0),
                (1, 1, 1.0),
            ];
            for (dx, dy, k) in kern {
                let s = sample(x + dx, y + dy);
                blur[0] += s[0] * k;
                blur[1] += s[1] * k;
                blur[2] += s[2] * k;
            }
            let o = orig[idx(x as u32, y as u32)];
            let cell = &mut buf[idx(x as u32, y as u32)];
            for c in 0..3 {
                let b = blur[c] / 16.0;
                cell[c] = (o[c] + amount * (o[c] - b)).clamp(0.0, 255.0);
            }
        }
    }
}
