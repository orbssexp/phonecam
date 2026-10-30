use crate::letterbox::scale_bgr_bilinear;
use jpeg_encoder::{ColorType, Encoder};

/// Downscales a BGR24 frame to `max_w` (if larger than `max_w`), preserving aspect ratio,
/// and encodes it into JPEG bytes.
pub fn encode_preview_jpeg(frame_bgr: &[u8], width: u32, height: u32, max_w: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || max_w == 0 || frame_bgr.len() < (width * height * 3) as usize {
        return None;
    }

    let (target_w, target_h, pixels) = if width <= max_w {
        (width, height, frame_bgr)
    } else {
        let tw = max_w;
        let th = ((height as f64 * tw as f64) / width as f64).round() as u32;
        let th = th.max(1);

        // Temp buffer for scaled pixels
        let mut scaled = vec![0u8; (tw * th * 3) as usize];
        scale_bgr_bilinear(frame_bgr, width, height, &mut scaled, tw, th);
        return encode_jpeg_bgr(&scaled, tw, th);
    };

    encode_jpeg_bgr(pixels, target_w, target_h)
}

fn encode_jpeg_bgr(pixels: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = Encoder::new(&mut out, 75);
    encoder
        .encode(pixels, w as u16, h as u16, ColorType::Bgr)
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_preview_jpeg() {
        let w = 100;
        let h = 50;
        let mut bgr = vec![0u8; (w * h * 3) as usize];
        // Blue image
        for chunk in bgr.as_chunks_mut::<3>().0 {
            chunk[0] = 255;
        }

        // Test downscaling from 100 to 50
        let jpeg = encode_preview_jpeg(&bgr, w, h, 50).expect("JPEG encoding should succeed");
        // Verify JPEG SOI marker (0xFF, 0xD8)
        assert!(jpeg.len() > 4);
        assert_eq!(jpeg[0], 0xFF);
        assert_eq!(jpeg[1], 0xD8);
    }
}
