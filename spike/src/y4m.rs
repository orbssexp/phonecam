//! Generates a noisy, moving 1080p30 Y4M so the fake-camera sender produces a realistic (~5-6 Mbps) H.264 stream.
use std::io::Write;

pub fn generate(path: &str, frames: u32) -> std::io::Result<()> {
    let (w, h) = (1920usize, 1080usize);
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(f, "YUV4MPEG2 W{w} H{h} F30:1 Ip A1:1 C420jpeg\n")?;
    let mut rng: u32 = 12345;
    let mut y = vec![0u8; w * h];
    let (cw, ch) = (w / 2, h / 2);
    let mut u = vec![128u8; cw * ch];
    let mut v = vec![128u8; cw * ch];
    for n in 0..frames as usize {
        for j in 0..h {
            for i in 0..w {
                rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((rng >> 24) as i32 % 7) - 3;
                let base = ((i + n * 7) * 255 / w + j * 60 / h) as i32 & 0xff;
                let block = if ((i / 160 + n / 6) + j / 135) % 2 == 0 { 30 } else { 0 };
                y[j * w + i] = (base / 2 + 60 + block + noise).clamp(16, 235) as u8;
            }
        }
        for j in 0..ch {
            for i in 0..cw {
                rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                u[j * cw + i] = (128 + ((i + n * 3) % 64) as i32 - 32 + ((rng >> 28) as i32 - 8)).clamp(16, 240) as u8;
                v[j * cw + i] = (128 + ((j + n * 2) % 64) as i32 - 32 + ((rng >> 27) as i32 & 7)).clamp(16, 240) as u8;
            }
        }
        f.write_all(b"FRAME\n")?;
        f.write_all(&y)?;
        f.write_all(&u)?;
        f.write_all(&v)?;
    }
    Ok(())
}
