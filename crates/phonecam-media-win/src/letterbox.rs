/// Geometry for positioning a scaled source frame within a fixed output frame,
/// preserving aspect ratio with letterbox or pillarbox bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LetterboxRect {
    pub offset_x: u32,
    pub offset_y: u32,
    pub scaled_w: u32,
    pub scaled_h: u32,
}

/// Computes the centered subrectangle within `(out_w, out_h)` that fits `(src_w, src_h)`
/// while strictly preserving the source aspect ratio.
///
/// If `src_w == 0` or `src_h == 0` or `out_w == 0` or `out_h == 0`, returns zeros.
pub fn calculate_letterbox(src_w: u32, src_h: u32, out_w: u32, out_h: u32) -> LetterboxRect {
    if src_w == 0 || src_h == 0 || out_w == 0 || out_h == 0 {
        return LetterboxRect {
            offset_x: 0,
            offset_y: 0,
            scaled_w: 0,
            scaled_h: 0,
        };
    }

    let src_aspect = src_w as f64 / src_h as f64;
    let out_aspect = out_w as f64 / out_h as f64;

    let (mut scaled_w, mut scaled_h) = if src_aspect > out_aspect {
        // Source is wider than output -> letterbox (black bars top and bottom)
        let w = out_w;
        let h = ((out_w as f64) / src_aspect).round() as u32;
        (w, h.min(out_h))
    } else {
        // Source is taller than or equal to output -> pillarbox (black bars left and right)
        let h = out_h;
        let w = ((out_h as f64) * src_aspect).round() as u32;
        (w.min(out_w), h)
    };

    // Ensure dimensions are even to prevent chroma alignment or codec stride issues
    if scaled_w > 1 && scaled_w % 2 != 0 {
        scaled_w -= 1;
    }
    if scaled_h > 1 && scaled_h % 2 != 0 {
        scaled_h -= 1;
    }

    let offset_x = (out_w.saturating_sub(scaled_w)) / 2;
    let offset_y = (out_h.saturating_sub(scaled_h)) / 2;

    LetterboxRect {
        offset_x,
        offset_y,
        scaled_w,
        scaled_h,
    }
}

/// Bilinearly rescales a 24-bit BGR image from `(src_w, src_h)` into `(dst_w, dst_h)`.
pub fn scale_bgr_bilinear(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return;
    }
    if src.len() < (src_w * src_h * 3) as usize || dst.len() < (dst_w * dst_h * 3) as usize {
        return;
    }

    if src_w == dst_w && src_h == dst_h {
        let len = (src_w * src_h * 3) as usize;
        dst[..len].copy_from_slice(&src[..len]);
        return;
    }

    let x_ratio = (src_w as f32) / (dst_w as f32);
    let y_ratio = (src_h as f32) / (dst_h as f32);

    for y in 0..dst_h {
        let src_y = (y as f32 * y_ratio).clamp(0.0, (src_h - 1) as f32);
        let y0 = src_y.floor() as u32;
        let y1 = (y0 + 1).min(src_h - 1);
        let dy = src_y - y0 as f32;

        let dst_row_start = (y * dst_w * 3) as usize;

        for x in 0..dst_w {
            let src_x = (x as f32 * x_ratio).clamp(0.0, (src_w - 1) as f32);
            let x0 = src_x.floor() as u32;
            let x1 = (x0 + 1).min(src_w - 1);
            let dx = src_x - x0 as f32;

            let idx00 = ((y0 * src_w + x0) * 3) as usize;
            let idx10 = ((y0 * src_w + x1) * 3) as usize;
            let idx01 = ((y1 * src_w + x0) * 3) as usize;
            let idx11 = ((y1 * src_w + x1) * 3) as usize;

            let dst_idx = dst_row_start + (x * 3) as usize;

            for c in 0..3 {
                let p00 = src[idx00 + c] as f32;
                let p10 = src[idx10 + c] as f32;
                let p01 = src[idx01 + c] as f32;
                let p11 = src[idx11 + c] as f32;

                let top = p00 * (1.0 - dx) + p10 * dx;
                let bottom = p01 * (1.0 - dx) + p11 * dx;
                let val = top * (1.0 - dy) + bottom * dy;

                dst[dst_idx + c] = val.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Blits a BGR24 image `src` of dimensions `(rect.scaled_w, rect.scaled_h)`
/// into `dst` of dimensions `(out_w, out_h)` at `(rect.offset_x, rect.offset_y)`.
pub fn blit_subrectangle_bgr(
    src: &[u8],
    rect: &LetterboxRect,
    dst: &mut [u8],
    out_w: u32,
    out_h: u32,
) {
    if rect.scaled_w == 0 || rect.scaled_h == 0 {
        return;
    }
    let src_stride = (rect.scaled_w * 3) as usize;
    let dst_stride = (out_w * 3) as usize;

    for row in 0..rect.scaled_h {
        let src_start = (row as usize) * src_stride;
        let src_end = src_start + src_stride;
        let dst_y = (rect.offset_y + row) as usize;
        let dst_start = dst_y * dst_stride + (rect.offset_x as usize) * 3;
        let dst_end = dst_start + src_stride;

        if src_end <= src.len() && dst_end <= dst.len() && dst_y < out_h as usize {
            dst[dst_start..dst_end].copy_from_slice(&src[src_start..src_end]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_dimensions() {
        let rect = calculate_letterbox(0, 1080, 1920, 1080);
        assert_eq!(rect, LetterboxRect { offset_x: 0, offset_y: 0, scaled_w: 0, scaled_h: 0 });

        let rect = calculate_letterbox(1920, 1080, 0, 0);
        assert_eq!(rect, LetterboxRect { offset_x: 0, offset_y: 0, scaled_w: 0, scaled_h: 0 });
    }

    #[test]
    fn test_identical_aspect_ratio_16_9() {
        // 720p to 1080p (both 16:9)
        let rect = calculate_letterbox(1280, 720, 1920, 1080);
        assert_eq!(rect.scaled_w, 1920);
        assert_eq!(rect.scaled_h, 1080);
        assert_eq!(rect.offset_x, 0);
        assert_eq!(rect.offset_y, 0);

        // 4K to 1080p
        let rect = calculate_letterbox(3840, 2160, 1920, 1080);
        assert_eq!(rect.scaled_w, 1920);
        assert_eq!(rect.scaled_h, 1080);
        assert_eq!(rect.offset_x, 0);
        assert_eq!(rect.offset_y, 0);
    }

    #[test]
    fn test_pillarbox_4_3_in_16_9() {
        // 4:3 (e.g. 640x480) into 1920x1080 (16:9)
        // height fits 1080 -> width = 1080 * 4/3 = 1440
        // bars on left/right: (1920 - 1440)/2 = 240 each
        let rect = calculate_letterbox(640, 480, 1920, 1080);
        assert_eq!(rect.scaled_w, 1440);
        assert_eq!(rect.scaled_h, 1080);
        assert_eq!(rect.offset_x, 240);
        assert_eq!(rect.offset_y, 0);
    }

    #[test]
    fn test_letterbox_wider_aspect_ratio() {
        // 21:9 source into 16:9 target (1920x1080)
        // e.g. 2560x1080 into 1920x1080
        // width fits 1920 -> height = 1920 / (2560/1080) = 810
        let rect = calculate_letterbox(2560, 1080, 1920, 1080);
        assert_eq!(rect.scaled_w, 1920);
        assert_eq!(rect.scaled_h, 810);
        assert_eq!(rect.offset_x, 0);
        assert_eq!(rect.offset_y, (1080 - 810) / 2);
    }

    #[test]
    fn test_portrait_stream_in_landscape_out() {
        // iPhone portrait 1080x1920 into 1920x1080
        // height fits 1080 -> width = 1080 * (1080/1920) = 607.5 -> even = 606 or 608
        let rect = calculate_letterbox(1080, 1920, 1920, 1080);
        assert_eq!(rect.scaled_h, 1080);
        assert!(rect.scaled_w <= 1920);
        assert!(rect.offset_x > 0);
        assert_eq!(rect.offset_y, 0);
        assert_eq!(rect.scaled_w % 2, 0);
        assert_eq!(rect.offset_x, (1920 - rect.scaled_w) / 2);
    }

    #[test]
    fn test_even_dimensions_guarantee() {
        let rect = calculate_letterbox(1366, 768, 1920, 1080);
        assert_eq!(rect.scaled_w % 2, 0);
        assert_eq!(rect.scaled_h % 2, 0);
        assert!(rect.offset_x + rect.scaled_w <= 1920);
        assert!(rect.offset_y + rect.scaled_h <= 1080);
    }

    #[test]
    fn test_scaling_and_blitting() {
        // 2x2 red into 4x4
        let src = vec![
            0, 0, 255,   0, 0, 255,
            0, 0, 255,   0, 0, 255,
        ];
        let mut dst = vec![0u8; 4 * 4 * 3];
        scale_bgr_bilinear(&src, 2, 2, &mut dst, 4, 4);
        assert_eq!(dst.len(), 48);
        assert_eq!(dst[0..3], [0, 0, 255]);
        assert_eq!(dst[45..48], [0, 0, 255]);

        // Blit 2x2 into 4x4 centered with offset (1, 1)
        let mut target = vec![0u8; 4 * 4 * 3];
        let rect = LetterboxRect { offset_x: 1, offset_y: 1, scaled_w: 2, scaled_h: 2 };
        blit_subrectangle_bgr(&src, &rect, &mut target, 4, 4);

        // Outside rectangle must remain 0 (black bar)
        assert_eq!(target[0..3], [0, 0, 0]); // (0,0)
        // Inside rectangle at (1,1) should be red
        let idx_1_1 = (4 + 1) * 3;
        assert_eq!(target[idx_1_1..idx_1_1 + 3], [0, 0, 255]);
    }
}
