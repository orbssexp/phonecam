use phonecam_core::VcamFrame;
use crate::font::render_centered_text_bgr;

/// Dark gray background color: RGB(36, 36, 36) -> BGR(36, 36, 36).
pub const PLACEHOLDER_BG: (u8, u8, u8) = (36, 36, 36);

/// Pure white text color: RGB(255, 255, 255) -> BGR(255, 255, 255).
pub const PLACEHOLDER_TEXT_COLOR: (u8, u8, u8) = (255, 255, 255);

/// Returns the status string associated with the given `VcamFrame`.
pub fn placeholder_text(frame: VcamFrame) -> &'static str {
    match frame {
        VcamFrame::Placeholder => "Scan the QR code in PhoneCam",
        VcamFrame::Reconnecting => "Reconnecting…",
        VcamFrame::Paused => "Paused on iPhone",
        VcamFrame::Camera => "Scan the QR code in PhoneCam",
    }
}

/// Renders a placeholder frame onto a BGR24 image buffer of size `(out_w, out_h)`.
pub fn render_placeholder(frame: VcamFrame, out_w: u32, out_h: u32, buf: &mut [u8]) {
    let text = placeholder_text(frame);
    render_centered_text_bgr(buf, out_w, out_h, text, PLACEHOLDER_TEXT_COLOR, PLACEHOLDER_BG);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_texts() {
        assert_eq!(placeholder_text(VcamFrame::Placeholder), "Scan the QR code in PhoneCam");
        assert_eq!(placeholder_text(VcamFrame::Reconnecting), "Reconnecting…");
        assert_eq!(placeholder_text(VcamFrame::Paused), "Paused on iPhone");
    }

    #[test]
    fn test_render_placeholder_buffer() {
        let w = 320;
        let h = 240;
        let mut buf = vec![0u8; (w * h * 3) as usize];
        render_placeholder(VcamFrame::Placeholder, w, h, &mut buf);

        // Verify dark gray background
        assert_eq!(buf[0..3], [PLACEHOLDER_BG.0, PLACEHOLDER_BG.1, PLACEHOLDER_BG.2]);
        // Verify text rendered
        let has_text = buf
            .as_chunks::<3>()
            .0
            .iter()
            .any(|&p| p == [PLACEHOLDER_TEXT_COLOR.0, PLACEHOLDER_TEXT_COLOR.1, PLACEHOLDER_TEXT_COLOR.2]);
        assert!(has_text);
    }
}
