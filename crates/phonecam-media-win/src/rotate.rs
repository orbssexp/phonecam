//! Clockwise rotation of BGR24 frames by a multiple of 90 degrees (applied before letterboxing).

/// Normalizes any angle to 0/90/180/270 (rounded to the nearest quarter turn).
pub fn normalize_deg(deg: i32) -> u16 {
    let q = ((deg as f64 / 90.0).round() as i32).rem_euclid(4);
    (q * 90) as u16
}

/// Size of a `w`x`h` frame after rotating by `deg` (0/90/180/270).
pub fn rotated_size(w: u32, h: u32, deg: u16) -> (u32, u32) {
    if deg == 90 || deg == 270 {
        (h, w)
    } else {
        (w, h)
    }
}

/// Rotates the BGR24 frame `src` (`w`x`h`, tightly packed) clockwise by `deg` into `dst`, which
/// must hold `w*h*3` bytes. Returns the new `(width, height)`. `deg` other than 90/180/270 copies.
pub fn rotate_bgr(src: &[u8], w: u32, h: u32, deg: u16, dst: &mut [u8]) -> (u32, u32) {
    let (w, h) = (w as usize, h as usize);
    let n = w * h * 3;
    assert!(src.len() >= n && dst.len() >= n, "rotate_bgr: buffers too small");
    match deg {
        90 => {
            // Output is h wide, w tall. src (x, y) -> dst (h-1-y, x).
            for y in 0..h {
                for x in 0..w {
                    let s = (y * w + x) * 3;
                    let d = (x * h + (h - 1 - y)) * 3;
                    dst[d..d + 3].copy_from_slice(&src[s..s + 3]);
                }
            }
            (h as u32, w as u32)
        }
        180 => {
            for i in 0..w * h {
                let s = i * 3;
                let d = (w * h - 1 - i) * 3;
                dst[d..d + 3].copy_from_slice(&src[s..s + 3]);
            }
            (w as u32, h as u32)
        }
        270 => {
            // Output is h wide, w tall. src (x, y) -> dst (y, w-1-x).
            for y in 0..h {
                for x in 0..w {
                    let s = (y * w + x) * 3;
                    let d = ((w - 1 - x) * h + y) * 3;
                    dst[d..d + 3].copy_from_slice(&src[s..s + 3]);
                }
            }
            (h as u32, w as u32)
        }
        _ => {
            dst[..n].copy_from_slice(&src[..n]);
            (w as u32, h as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 wide, 2 tall; pixel value = its index (B=G=R=index) so the layout is easy to read:
    ///   0 1 2
    ///   3 4 5
    fn sample() -> Vec<u8> {
        (0..6u8).flat_map(|i| [i, i, i]).collect()
    }

    fn plane(buf: &[u8]) -> Vec<u8> {
        buf.chunks(3).map(|p| p[0]).collect()
    }

    #[test]
    fn rotate_90_clockwise() {
        let mut dst = vec![0u8; 18];
        let (w, h) = rotate_bgr(&sample(), 3, 2, 90, &mut dst);
        assert_eq!((w, h), (2, 3));
        // Clockwise: left column (0,3) becomes the top row, read bottom-to-top.
        assert_eq!(plane(&dst), vec![3, 0, 4, 1, 5, 2]);
    }

    #[test]
    fn rotate_180() {
        let mut dst = vec![0u8; 18];
        let (w, h) = rotate_bgr(&sample(), 3, 2, 180, &mut dst);
        assert_eq!((w, h), (3, 2));
        assert_eq!(plane(&dst), vec![5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn rotate_270_clockwise() {
        let mut dst = vec![0u8; 18];
        let (w, h) = rotate_bgr(&sample(), 3, 2, 270, &mut dst);
        assert_eq!((w, h), (2, 3));
        assert_eq!(plane(&dst), vec![2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn four_quarter_turns_are_identity_and_zero_copies() {
        let src = sample();
        let mut a = vec![0u8; 18];
        let mut b = vec![0u8; 18];
        let (mut w, mut h) = (3, 2);
        let mut cur = src.clone();
        for _ in 0..4 {
            (w, h) = rotate_bgr(&cur, w, h, 90, &mut a);
            std::mem::swap(&mut cur, &mut a);
        }
        assert_eq!((w, h, &cur), (3, 2, &src));
        assert_eq!(rotate_bgr(&src, 3, 2, 0, &mut b), (3, 2));
        assert_eq!(b, src);
    }

    #[test]
    fn channels_stay_together() {
        let src = vec![1, 2, 3, 4, 5, 6]; // two pixels: (1,2,3) (4,5,6)
        let mut dst = vec![0u8; 6];
        assert_eq!(rotate_bgr(&src, 2, 1, 90, &mut dst), (1, 2));
        assert_eq!(dst, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(rotate_bgr(&src, 2, 1, 270, &mut dst), (1, 2));
        assert_eq!(dst, vec![4, 5, 6, 1, 2, 3]);
    }

    #[test]
    fn normalize_and_sizes() {
        assert_eq!(normalize_deg(450), 90);
        assert_eq!(normalize_deg(-90), 270);
        assert_eq!(normalize_deg(360), 0);
        assert_eq!(rotated_size(1920, 1080, 90), (1080, 1920));
        assert_eq!(rotated_size(1920, 1080, 180), (1920, 1080));
    }
}
