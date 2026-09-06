//! Does ingest produce the NV12 the encoder expects? RGB in, BT.709 limited range
//! out, read straight back from the surface — no encoder, no decoder, just the
//! numbers. Then the same through a dmabuf, exported and re-imported on this
//! display: a capture buffer with no compositor in the way.
//!
//! Ignored: needs a VAAPI device. `.25` and `.50` both have one.

use std::os::fd::FromRawFd as _;
use std::os::fd::OwnedFd;

use pf_libva::encode::open;
use pf_libva::vpp::Vpp;
use pf_libva::Display;
use pf_libva::DmabufSource;
use pf_libva::Libva;
use pf_libva::VaSurfaceId;
use pf_vaapi::drm::flatten;
use pf_vaapi::drm::VaDrmPrimeSurfaceDescriptor;
use pf_vaapi::drm::VA_EXPORT_SURFACE_READ_ONLY;
use pf_vaapi::drm::VA_EXPORT_SURFACE_SEPARATE_LAYERS;
use pf_vaapi::drm::VA_FOURCC_NV12;
use pf_vaapi::enc_params::SessionParams;
use pf_vaapi::vpp::DRM_FORMAT_ARGB8888;
use pf_vaapi::vpp::DRM_FORMAT_XRGB8888;
use pf_vaapi::vpp::VA_FOURCC_BGRA;
use pf_vaapi::vpp::VA_RT_FORMAT_RGB32;
use pf_vaapi::vpp::VA_RT_FORMAT_YUV420;
use pf_vaapi::vpp::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2;

const W: u32 = 320;
const H: u32 = 240;

fn display() -> Display {
    Display::open(Libva::load().expect("libva")).expect("a VAAPI display")
}

/// A solid BGRA picture.
fn bgra(b: u8, g: u8, r: u8) -> Vec<u8> {
    [b, g, r, 255].repeat((W * H) as usize)
}

/// Y, Cb, Cr at the picture centre.
fn centre_yuv(display: &Display, nv12: VaSurfaceId) -> (u8, u8, u8) {
    display
        .map_image(nv12, |image, ptr| {
            let (x, y) = (W as usize / 2, H as usize / 2);
            // SAFETY: inside the mapped image, by the driver's own pitches and offsets.
            unsafe {
                let luma = *ptr.add(image.offsets[0] as usize + y * image.pitches[0] as usize + x);
                let uv = ptr.add(
                    image.offsets[1] as usize + (y / 2) * image.pitches[1] as usize + (x / 2) * 2,
                );
                Ok((luma, *uv, *uv.add(1)))
            }
        })
        .expect("read the NV12 back")
}

fn near(got: (u8, u8, u8), want: (u8, u8, u8), what: &str) {
    for ((g, w), name) in [got.0, got.1, got.2]
        .into_iter()
        .zip([want.0, want.1, want.2])
        .zip(["Y", "Cb", "Cr"])
    {
        assert!(
            (i32::from(g) - i32::from(w)).abs() <= 4,
            "{what}: {name} = {g}, want {w} (got {got:?}, want {want:?})"
        );
    }
}

/// BT.709, limited range: red is Y 63 / Cb 102 / Cr 240 and white is Y 235.
/// BT.601 would put red's Y at 82 and full range at 54 — a different, wrong picture
/// that decodes fine.
#[test]
#[ignore = "needs a VAAPI device"]
fn rgb_becomes_limited_range_bt709_nv12() {
    let display = display();
    let vpp = Vpp::new(&display, W, H).expect("VideoProc");
    let src = display
        .create_surface(VA_RT_FORMAT_RGB32, Some(VA_FOURCC_BGRA), W, H)
        .expect("a BGRA surface");
    let dst = display
        .create_surface(VA_RT_FORMAT_YUV420, Some(VA_FOURCC_NV12), W, H)
        .expect("an NV12 surface");
    let row = W as usize * 4;

    display
        .write_packed(src, &bgra(0, 0, 255), row)
        .expect("upload red");
    vpp.convert(&display, src, true, false, dst)
        .expect("convert red");
    near(centre_yuv(&display, dst), (63, 102, 240), "red");

    display
        .write_packed(src, &bgra(255, 255, 255), row)
        .expect("upload white");
    vpp.convert(&display, src, true, false, dst)
        .expect("convert white");
    near(centre_yuv(&display, dst), (235, 128, 128), "white");

    // The same red through a dmabuf, exported here and imported back as `XR24`
    // (the everyday capture format) and as `AR24`.
    display
        .write_packed(src, &bgra(0, 0, 255), row)
        .expect("upload red");
    let mut desc = VaDrmPrimeSurfaceDescriptor::zeroed();
    // SAFETY: live display and surface; `desc` is exactly the layout `DRM_PRIME_2`
    // writes and it outlives the call.
    display
        .va
        .check("vaExportSurfaceHandle", unsafe {
            (display.va.export_surface_handle)(
                display.display,
                src,
                VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
                VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_SEPARATE_LAYERS,
                (&raw mut desc).cast(),
            )
        })
        .expect("export the BGRA surface");
    let exported = flatten(&desc).expect("a flat plane list");
    // SAFETY: each fd came out of a successful export and is wrapped exactly once.
    let fds: Vec<OwnedFd> = exported
        .object_fds
        .iter()
        .map(|&fd| unsafe { OwnedFd::from_raw_fd(fd) })
        .collect();
    println!(
        "exported: {} plane(s), modifier {:#x}, stride {}",
        exported.planes.len(),
        exported.modifier,
        exported.planes[0].stride
    );
    for drm_fourcc in [DRM_FORMAT_XRGB8888, DRM_FORMAT_ARGB8888] {
        let source = DmabufSource {
            width: W,
            height: H,
            drm_fourcc,
            modifier: exported.modifier,
            planes: &exported.planes,
        };
        let imported = display.import_dmabuf(&source).expect("import the dmabuf");
        vpp.convert(&display, imported, true, false, dst)
            .expect("convert the import");
        near(centre_yuv(&display, dst), (63, 102, 240), "red via dmabuf");
        display.destroy_surface(imported);
    }
    drop(fds);
    display.destroy_surface(src);
    display.destroy_surface(dst);
    vpp.destroy(&display);
}

/// The whole path through the session's own ingest: RGB in, H.264 out.
/// `PF_ENC_OUT` writes the stream; frame 0 is pure red, so a decoder's first pixel
/// is the second opinion on the colour.
#[test]
#[ignore = "needs a VAAPI encode device"]
fn the_session_encodes_what_ingest_gives_it() {
    let mut enc = open(SessionParams {
        width: W,
        height: H,
        fps_num: 60,
        fps_den: 1,
        bitrate_bps: 4_000_000,
        slots: 1,
        max_num_reorder_frames: 0,
        initial_qp: 26,
        vbv_frames: 1.0,
    })
    .expect("an encoder");
    let mut stream = Vec::new();
    for i in 0..10u8 {
        // A colour that moves every frame, so the P frames carry something.
        enc.submit_packed(
            &bgra(i * 20, 0, 255 - i * 20),
            VA_FOURCC_BGRA,
            W as usize * 4,
        )
        .expect("ingest");
        let pic = enc.encode(i == 0).expect("encode");
        assert_eq!(pic.is_idr, i == 0);
        stream.extend_from_slice(&pic.bytes);
    }
    if let Ok(path) = std::env::var("PF_ENC_OUT") {
        std::fs::write(&path, &stream).expect("write the stream out");
        println!("wrote {path}");
    }
    assert!(stream.len() > 100, "{} bytes for ten frames", stream.len());
}
