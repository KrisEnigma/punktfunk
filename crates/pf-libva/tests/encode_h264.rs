//! Does the native encoder produce bytes a decoder accepts?
//!
//! Ignored: needs a VAAPI encode device. `.25` (AMD 780M) and `.50` (Intel UHD 750)
//! both have one.
//!
//! ```text
//! cargo test -p pf-libva --test encode_h264 -- --ignored --nocapture
//! ```
//!
//! `PF_ENC_OUT=/tmp/out.h264` writes the stream out so `ffmpeg -i` can be the second
//! opinion — our own planner agreeing with our own encoder proves less than a decoder
//! that shares no code with either.

use pf_libva::encode::open;
use pf_vaapi::enc_params::SessionParams;

/// 320x240 keeps the surfaces small and is still off the macroblock grid in neither
/// dimension, so the crop path stays out of the way of a first-frame proof.
fn params() -> SessionParams {
    SessionParams {
        width: 320,
        height: 240,
        fps_num: 60,
        fps_den: 1,
        bitrate_bps: 4_000_000,
        max_num_ref_frames: 1,
        max_num_reorder_frames: 0,
        initial_qp: 26,
    }
}

/// A moving edge, so successive frames actually differ — an encoder fed identical
/// pictures can emit almost nothing and still look like it works.
fn frame(width: usize, height: usize, phase: usize) -> (Vec<u8>, Vec<u8>) {
    let mut y = vec![16u8; width * height];
    for (row, line) in y.chunks_mut(width).enumerate() {
        for (col, px) in line.iter_mut().enumerate() {
            if (col + phase * 8) % 64 < 32 || row % 48 < 8 {
                *px = 235;
            }
        }
    }
    (y, vec![128u8; width * height / 2])
}

/// **Known failing, deliberately.** The session encodes, the driver reports success
/// and the sizes are sane, but radeonsi emits slice payload with a zero NAL header:
/// the app has supplied packed parameter sets and no packed *slice* header. libav's
/// own VAAPI encoder requests `SEQUENCE | SLICE | MISC` and supplies all three.
///
/// Left failing rather than deleted or weakened. It names the exact remaining gap,
/// and a test that passed without slice headers would be asserting the wrong
/// contract — the stream would still not decode.
#[test]
#[ignore = "needs a VAAPI encode device; fails until the packed slice header exists"]
fn the_encoder_emits_a_decodable_stream() {
    let p = params();
    let mut enc = match open(p) {
        Ok(e) => e,
        Err(e) => panic!("no VAAPI encoder here: {e:#}"),
    };

    let mut stream = Vec::new();
    let mut sizes = Vec::new();
    for i in 0..30 {
        let (y, uv) = frame(p.width as usize, p.height as usize, i);
        enc.write_nv12(&y, &uv).expect("fill the input surface");
        let pic = enc.encode(i == 0).expect("encode");
        assert!(!pic.bytes.is_empty(), "frame {i} came back empty");
        assert_eq!(pic.is_idr, i == 0, "only the first frame opens the GOP");
        sizes.push(pic.bytes.len());
        stream.extend_from_slice(&pic.bytes);
    }

    println!("30 frames, {} bytes, sizes {:?}", stream.len(), &sizes[..5]);

    // Written before the assertions, so a failure still leaves the artefact to look at.
    if let Ok(path) = std::env::var("PF_ENC_OUT") {
        std::fs::write(&path, &stream).expect("write the stream out");
        println!("wrote {path} — check with: ffmpeg -v error -i {path} -f null -");
    }

    let starts: Vec<usize> = (0..stream.len().saturating_sub(4))
        .filter(|&i| stream[i..i + 4] == [0, 0, 0, 1])
        .collect();

    // One IDR and twenty-nine P slices. Counting types beats comparing sizes: a
    // moving edge makes P frames legitimately expensive, so size proves nothing,
    // but an encoder that ignores our slice_type emits thirty IDRs and this catches
    // it exactly.
    let nal_types: Vec<u8> = starts.iter().map(|&i| stream[i + 4] & 0x1f).collect();
    let idrs = nal_types.iter().filter(|&&t| t == 5).count();
    let ps = nal_types.iter().filter(|&&t| t == 1).count();
    assert_eq!(idrs, 1, "exactly one IDR: {nal_types:?}");
    assert_eq!(ps, 29, "the rest are P slices: {nal_types:?}");
    assert!(starts.len() >= 3, "expected SPS, PPS and slices");
    assert_eq!(stream[starts[0] + 4] & 0x1f, 7, "first NALU is the SPS");
    assert_eq!(stream[starts[1] + 4] & 0x1f, 8, "then the PPS");
    assert_eq!(stream[starts[2] + 4] & 0x1f, 5, "then an IDR slice");

    // Our own planner is the first reader: it is the client's, so a stream it
    // refuses is a stream the client refuses.
    let mut planner = pf_bitstream::h264::H264Planner::new();
    let aus = split_aus(&stream);
    let mut planned = 0;
    for (i, au) in aus.iter().enumerate() {
        match planner.plan_au(au) {
            Ok(_) => planned += 1,
            Err(e) => panic!("AU {i} did not plan: {e}"),
        }
    }
    assert_eq!(planned, 30, "every access unit should plan");
}

/// Split on the parameter-set/slice boundary: each AU here is the SPS+PPS+slice of
/// an IDR, or a single P slice.
fn split_aus(stream: &[u8]) -> Vec<&[u8]> {
    let starts: Vec<usize> = (0..stream.len().saturating_sub(4))
        .filter(|&i| stream[i..i + 4] == [0, 0, 0, 1])
        .collect();
    let mut aus = Vec::new();
    let mut au_start = 0;
    for (n, &s) in starts.iter().enumerate() {
        let nal_type = stream[s + 4] & 0x1f;
        // 7 = SPS opens a new AU; 1/5 = a slice ends one unless the SPS just began it.
        if n > 0 && (nal_type == 7 || (nal_type == 1 && stream[starts[n - 1] + 4] & 0x1f != 7)) {
            aus.push(&stream[au_start..s]);
            au_start = s;
        }
    }
    aus.push(&stream[au_start..]);
    aus
}
