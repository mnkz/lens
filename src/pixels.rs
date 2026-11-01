#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PxRect {
    pub fn from_points(a: (i32, i32), b: (i32, i32)) -> Self {
        let x = a.0.min(b.0);
        let y = a.1.min(b.1);
        Self {
            x,
            y,
            w: (a.0 - b.0).abs().max(1),
            h: (a.1 - b.1).abs().max(1),
        }
    }

    pub fn square(anchor: (i32, i32), cursor: (i32, i32)) -> Self {
        let dx = cursor.0 - anchor.0;
        let dy = cursor.1 - anchor.1;
        let side = dx.abs().max(dy.abs()).max(1);
        Self {
            x: if dx >= 0 { anchor.0 } else { anchor.0 - side },
            y: if dy >= 0 { anchor.1 } else { anchor.1 - side },
            w: side,
            h: side,
        }
    }

    pub fn clamp_bounds(self, bw: i32, bh: i32) -> Option<Self> {
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x + self.w).min(bw);
        let y1 = (self.y + self.h).min(bh);
        if x1 - x0 < 2 || y1 - y0 < 2 {
            return None;
        }
        Some(Self {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

pub fn force_opaque(bgra: &mut [u8]) {
    for px in bgra.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
}

/// A faint cool wash. Sixteen percent toward a steel blue, so the freeze
/// stays readable and the cursor reveal still has something to open.
pub fn veiled(src: &[u8]) -> Vec<u8> {
    const TINT_B: u16 = 96;
    const TINT_G: u16 = 70;
    const TINT_R: u16 = 42;
    const MIX: u16 = 16;
    let mut out = src.to_vec();
    for px in out.as_chunks_mut::<4>().0 {
        px[0] = mix_channel(px[0], TINT_B, MIX);
        px[1] = mix_channel(px[1], TINT_G, MIX);
        px[2] = mix_channel(px[2], TINT_R, MIX);
        px[3] = 255;
    }
    out
}

fn mix_channel(src: u8, tint: u16, pct: u16) -> u8 {
    let src = src as u16;
    ((src * (100 - pct) + tint * pct) / 100) as u8
}

/// Soft disc of `src` blended into `back`. `t` is 1 at the center and 0 at `radius`.
pub fn reveal(back: &mut [u8], src: &[u8], w: i32, h: i32, cx: i32, cy: i32, radius: i32) {
    if radius < 2 || w < 1 || h < 1 {
        return;
    }
    let r = radius as i64;
    let r2 = r * r;
    let x0 = (cx - radius).max(0);
    let y0 = (cy - radius).max(0);
    let x1 = (cx + radius + 1).min(w);
    let y1 = (cy + radius + 1).min(h);
    let stride = w as usize * 4;
    for y in y0..y1 {
        let dy = (y - cy) as i64;
        for x in x0..x1 {
            let dx = (x - cx) as i64;
            let d2 = dx * dx + dy * dy;
            if d2 >= r2 {
                continue;
            }
            let dist = (d2 as f32).sqrt() / r as f32;
            let t = (1.0 - dist).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            let a = (t * 255.0) as u16;
            let i = y as usize * stride + x as usize * 4;
            if i + 3 >= back.len() || i + 3 >= src.len() {
                continue;
            }
            for c in 0..3 {
                let s = src[i + c] as u16;
                let b = back[i + c] as u16;
                back[i + c] = ((s * a + b * (255 - a)) / 255) as u8;
            }
            back[i + 3] = 255;
        }
    }
}

pub fn crop(src: &[u8], stride: i32, rect: PxRect) -> Option<Vec<u8>> {
    if rect.w < 1 || rect.h < 1 || stride < 1 {
        return None;
    }
    let need = (rect.y as i64 + rect.h as i64) * stride as i64 * 4;
    if need > src.len() as i64 || rect.x < 0 || rect.y < 0 {
        return None;
    }
    let mut out = Vec::with_capacity((rect.w * rect.h * 4) as usize);
    for y in rect.y..rect.y + rect.h {
        let start = ((y * stride + rect.x) * 4) as usize;
        let end = start + (rect.w * 4) as usize;
        if end > src.len() {
            return None;
        }
        out.extend_from_slice(&src[start..end]);
    }
    Some(out)
}

pub fn sample_hex(bgra: &[u8], w: i32, h: i32, x: i32, y: i32) -> Option<String> {
    if x < 0 || y < 0 || x >= w || y >= h {
        return None;
    }
    let i = ((y * w + x) * 4) as usize;
    if i + 3 >= bgra.len() {
        return None;
    }
    Some(format!(
        "#{:02X}{:02X}{:02X}",
        bgra[i + 2],
        bgra[i + 1],
        bgra[i]
    ))
}

pub fn to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bgra.len()];
    for (src, dst) in bgra
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0)
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = 255;
    }
    out
}

pub fn downscale(src: &[u8], sw: i32, sh: i32, dw: i32, dh: i32) -> Vec<u8> {
    let dw = dw.max(1);
    let dh = dh.max(1);
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        let y0 = y * sh / dh;
        let y1 = ((y + 1) * sh / dh).max(y0 + 1).min(sh);
        for x in 0..dw {
            let x0 = x * sw / dw;
            let x1 = ((x + 1) * sw / dw).max(x0 + 1).min(sw);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * sw + sx) * 4) as usize;
                    if i + 3 >= src.len() {
                        continue;
                    }
                    acc[0] += src[i] as u32;
                    acc[1] += src[i + 1] as u32;
                    acc[2] += src[i + 2] as u32;
                    acc[3] += src[i + 3] as u32;
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            let o = ((y * dw + x) * 4) as usize;
            out[o] = (acc[0] / n) as u8;
            out[o + 1] = (acc[1] / n) as u8;
            out[o + 2] = (acc[2] / n) as u8;
            out[o + 3] = 255;
        }
    }
    out
}

pub fn fit_max(src: &[u8], w: i32, h: i32, max_side: i32) -> (Vec<u8>, i32, i32, f32) {
    let max_side = max_side.max(32);
    let long = w.max(h);
    if long <= max_side || w < 1 || h < 1 {
        return (src.to_vec(), w.max(1), h.max(1), 1.0);
    }
    let scale = max_side as f32 / long as f32;
    let dw = (w as f32 * scale).round().max(1.0) as i32;
    let dh = (h as f32 * scale).round().max(1.0) as i32;
    let back = w as f32 / dw as f32;
    (downscale(src, w, h, dw, dh), dw, dh, back)
}

pub fn thumb_rgba(bgra: &[u8], w: i32, h: i32, max_w: i32) -> (Vec<u8>, u32, u32) {
    let max_w = max_w.max(1);
    let tw = w.min(max_w).max(1);
    let th = ((h as i64 * tw as i64) / w.max(1) as i64).clamp(1, 4000) as i32;
    let small = if tw == w && th == h {
        bgra.to_vec()
    } else {
        downscale(bgra, w, h, tw, th)
    };
    (to_rgba(&small), tw as u32, th as u32)
}

pub fn encode_png(bgra: &[u8], w: i32, h: i32) -> Result<Vec<u8>, String> {
    if w < 1 || h < 1 {
        return Err("image vide".into());
    }
    let rgba = to_rgba(bgra);
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|err| err.to_string())?;
    writer
        .write_image_data(&rgba)
        .map_err(|err| err.to_string())?;
    drop(writer);
    Ok(out)
}

pub fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, i32, i32), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|err| err.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|err| err.to_string())?;
    let w = info.width as i32;
    let h = info.height as i32;
    let data = &buf[..info.buffer_size()];
    let bgra = match info.color_type {
        png::ColorType::Rgba => {
            let mut out = vec![0u8; data.len()];
            for (src, dst) in data
                .as_chunks::<4>()
                .0
                .iter()
                .zip(out.as_chunks_mut::<4>().0)
            {
                dst[0] = src[2];
                dst[1] = src[1];
                dst[2] = src[0];
                dst[3] = 255;
            }
            out
        }
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(data.len() / 3 * 4);
            for src in data.as_chunks::<3>().0 {
                out.extend_from_slice(&[src[2], src[1], src[0], 255]);
            }
            out
        }
        _ => return Err("png non géré".into()),
    };
    if bgra.len() != (w * h * 4) as usize {
        return Err("png incomplet".into());
    }
    Ok((bgra, w, h))
}

pub fn dib_bytes(bgra: &[u8], w: i32, h: i32) -> Vec<u8> {
    let header = 40usize;
    let row = (w as usize) * 4;
    let mut out = vec![0u8; header + row * h as usize];
    out[0] = 40;
    out[4..8].copy_from_slice(&w.to_le_bytes());
    out[8..12].copy_from_slice(&h.to_le_bytes());
    out[12] = 1;
    out[14] = 32;
    for y in 0..h {
        let src_y = h - 1 - y;
        let src = ((src_y * w) * 4) as usize;
        let dst = header + (y as usize) * row;
        if src + row <= bgra.len() {
            out[dst..dst + row].copy_from_slice(&bgra[src..src + row]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_drops_a_sliver() {
        let rect = PxRect {
            x: -4,
            y: 2,
            w: 5,
            h: 20,
        };
        assert!(rect.clamp_bounds(100, 100).is_none());
    }

    #[test]
    fn clamp_keeps_the_visible_part() {
        let rect = PxRect {
            x: -10,
            y: 5,
            w: 40,
            h: 10,
        };
        let got = rect.clamp_bounds(100, 50).unwrap();
        assert_eq!(
            got,
            PxRect {
                x: 0,
                y: 5,
                w: 30,
                h: 10
            }
        );
    }

    #[test]
    fn square_grows_from_the_anchor() {
        let rect = PxRect::square((10, 10), (4, 20));
        assert_eq!(rect.w, rect.h);
        assert!(rect.contains(10, 10) || (rect.x + rect.w) == 10 || rect.y == 10);
        assert_eq!(rect.w, 10);
    }

    #[test]
    fn crop_and_roundtrip() {
        let mut px = vec![0u8; 8 * 4];
        px[0] = 10;
        px[1] = 20;
        px[2] = 30;
        px[3] = 0;
        force_opaque(&mut px);
        assert_eq!(px[3], 255);
        let rect = PxRect {
            x: 1,
            y: 0,
            w: 1,
            h: 2,
        };
        let cropped = crop(&px, 2, rect).unwrap();
        assert_eq!(cropped.len(), 8);
        let png = encode_png(&cropped, 1, 2).unwrap();
        let (back, w, h) = decode_png(&png).unwrap();
        assert_eq!((w, h), (1, 2));
        assert_eq!(back[0], cropped[0]);
        assert_eq!(back[2], cropped[2]);
    }

    #[test]
    fn hex_reads_bgra_as_rgb() {
        let px = [1u8, 2, 3, 255];
        assert_eq!(sample_hex(&px, 1, 1, 0, 0).as_deref(), Some("#030201"));
    }

    #[test]
    fn veil_leans_blue_and_stays_close() {
        let src = [40u8, 40, 40, 255];
        let out = veiled(&src);
        assert!(out[0] > out[2]);
        assert!(out[0] > src[0]);
        assert!((out[0] as i16 - src[0] as i16).abs() < 30);
        assert_eq!(out[3], 255);
    }

    #[test]
    fn reveal_opens_only_the_disc() {
        let w = 21;
        let h = 21;
        let src = vec![255u8; (w * h * 4) as usize];
        let mut back = vec![0u8; src.len()];
        reveal(&mut back, &src, w, h, 10, 10, 8);
        let center = ((10 * w + 10) * 4) as usize;
        assert!(back[center] > 200);
        assert_eq!(back[0], 0);
        let edge = ((10 * w + 18) * 4) as usize;
        assert_eq!(back[edge], 0);
        let inside = ((10 * w + 17) * 4) as usize;
        assert!(back[inside] > 0);
    }
}
