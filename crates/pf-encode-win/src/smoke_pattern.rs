//! What the wave smokes share: the moving texture they encode, and the writer that stores a
//! stream the way `gpu_parity`'s field hashers read it. Rows band by luma and columns band the green,
//! so motion in either axis codes; `PF_WAVE_SCROLL=dx,dy` moves it that many pixels per
//! frame (default down 6, so motion vectors point up into rows a sweep already refreshed);
//! `PF_WAVE_NOISE=1` adds per-pixel noise that scrolls with the content, which starves the
//! encoder the way game content does. Test-only callers, in three crates.

/// BGRA, `w * h * 4` bytes, at `frame` frames of motion.
pub fn scroll_pattern(w: usize, h: usize, frame: usize) -> Vec<u8> {
    let (dx, dy) = std::env::var("PF_WAVE_SCROLL")
        .ok()
        .and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.trim().parse::<i64>().ok()?, y.trim().parse::<i64>().ok()?))
        })
        .unwrap_or((0, 6));
    let (sx, sy) = (dx * frame as i64, dy * frame as i64);
    let noise = std::env::var("PF_WAVE_NOISE").is_ok_and(|v| v == "1");
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        let cy = (y as i64 - sy).rem_euclid(h as i64);
        let band = cy as u8;
        for x in 0..w {
            let cx = (x as i64 - sx).rem_euclid(w as i64);
            let col = cx as u8;
            let o = (y * w + x) * 4;
            let n = if noise {
                // xorshift of the content coordinate: white noise, ±32 per channel.
                let mut v = (cx as u32).wrapping_mul(0x9E37_79B9)
                    ^ (cy as u32).wrapping_mul(0x85EB_CA6B)
                    ^ 0x5bd1_e995;
                v ^= v << 13;
                v ^= v >> 17;
                v ^= v << 5;
                (v & 63) as i16 - 32
            } else {
                0
            };
            let c = |b: u8| (i16::from(b) + n).clamp(0, 255) as u8;
            px[o] = c(band.wrapping_mul(3));
            px[o + 1] = c(band ^ col);
            px[o + 2] = c(255 - band);
            px[o + 3] = 255;
        }
    }
    px
}

/// Write `aus` to `path` with the `.idx` sidecar a `PUNKTFUNK_DUMP_VIDEO` capture carries
/// (`offset len flags complete` per access unit), so a field hasher splits any codec's stream
/// by access unit.
pub fn write_capture(path: &str, aus: &[&[u8]]) -> std::io::Result<()> {
    let mut data = Vec::new();
    let mut idx = String::new();
    for au in aus {
        idx.push_str(&format!("{} {} 0x0 1\n", data.len(), au.len()));
        data.extend_from_slice(au);
    }
    std::fs::write(path, &data)?;
    std::fs::write(format!("{path}.idx"), idx)
}
