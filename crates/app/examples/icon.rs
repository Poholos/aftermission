//! Paints the app icon and writes it to `assets/icon/`: a PNG per size
//! and the `.ico` that `build.rs` embeds into the Windows executable.
//! Run from the workspace root: `cargo run -p aftermission --example icon`.
//!
//! The picture is the app's plot in miniature: on a dark rounded square,
//! an amber flight profile that climbs, holds with a few wobbles and
//! comes down, over two faint flight-mode bands and a gray baseline.
//! Every size is painted on its own rather than scaled down, so at 32
//! pixels and below the bands and the baseline, which would smear, are
//! left out and the profile keeps a stroke of two whole pixels.

use std::fs;
use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};

use image::{ImageFormat, Rgba, RgbaImage};

const SIZES: [u16; 6] = [16, 32, 48, 64, 128, 256];
/// What Explorer, the taskbar and the Start menu ask the executable for.
const ICO_SIZES: [u16; 4] = [16, 32, 48, 256];
/// Subsamples along each pixel edge; the coverage antialiases the edges.
const SUBSAMPLES: u16 = 8;

/// The window's ground, as the web page's canvas background.
const BACKGROUND: [u8; 4] = [0x1b, 0x1b, 0x1b, 0xff];
/// A hairline around the square, so it stands out on a dark taskbar.
const EDGE: [u8; 4] = [0x3a, 0x3a, 0x3a, 0xff];
const BASELINE: [u8; 4] = [0x5a, 0x5a, 0x5a, 0xff];
const PROFILE: [u8; 4] = [0xf5, 0xa6, 0x23, 0xff];
/// The mode bands' hues, laid over the ground at `BAND_ALPHA`.
const BAND_BLUE: [u8; 3] = [0x4f, 0x8f, 0xf7];
const BAND_GREEN: [u8; 3] = [0x3f, 0xbf, 0x7f];
const BAND_ALPHA: f32 = 0.24;

/// The drawing's own units: a 256-unit square, whatever the size.
const UNITS: f32 = 256.0;
const MARGIN: f32 = 8.0;
const CORNER: f32 = 48.0;
const EDGE_WIDTH: f32 = 3.0;
/// Left, top, width, height.
const BANDS: [([f32; 4], [u8; 3]); 2] = [
    ([76.0, 36.0, 60.0, 184.0], BAND_BLUE),
    ([150.0, 36.0, 50.0, 184.0], BAND_GREEN),
];
const BASELINE_Y: f32 = 202.0;
const BASELINE_ENDS: [f32; 2] = [44.0, 212.0];
const BASELINE_WIDTH: f32 = 6.0;
/// The climb, the hold with its wobbles, and the descent.
const PROFILE_POINTS: [[f32; 2]; 9] = [
    [44.0, 196.0],
    [66.0, 190.0],
    [94.0, 96.0],
    [118.0, 86.0],
    [138.0, 92.0],
    [160.0, 84.0],
    [182.0, 90.0],
    [200.0, 150.0],
    [212.0, 190.0],
];
const PROFILE_WIDTH: f32 = 14.0;
/// The narrowest the profile may get, in pixels.
const PROFILE_MIN_PIXELS: f32 = 2.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args_os().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon"),
        PathBuf::from,
    );
    fs::create_dir_all(&out)?;
    for size in SIZES {
        let name = if size == 256 {
            "aftermission.png".to_string()
        } else {
            format!("aftermission-{size}.png")
        };
        Design::for_size(size).paint().save(out.join(name))?;
    }
    write_ico(&out.join("aftermission.ico"))?;
    println!("wrote the icon to {}", out.display());
    Ok(())
}

/// The drawing for one size.
struct Design {
    size: f32,
    /// Units per pixel.
    scale: f32,
    /// The bands and the baseline, left out at small sizes.
    detail: bool,
    /// The profile's stroke, in units.
    profile_width: f32,
}

impl Design {
    fn for_size(size: u16) -> Self {
        let px = f32::from(size);
        let scale = UNITS / px;
        Self {
            size: px,
            scale,
            detail: size > 32,
            profile_width: PROFILE_WIDTH.max(PROFILE_MIN_PIXELS * scale),
        }
    }

    /// The topmost shape covering the point, in units, or none outside
    /// the square.
    fn color_at(&self, p: [f32; 2]) -> Option<[u8; 4]> {
        let inside = rounded_square(p);
        if inside > 0.0 {
            return None;
        }
        let on_profile = PROFILE_POINTS
            .windows(2)
            .any(|w| to_segment(p, w[0], w[1]) < self.profile_width / 2.0);
        if on_profile {
            return Some(PROFILE);
        }
        if self.detail {
            let baseline = to_segment(
                p,
                [BASELINE_ENDS[0], BASELINE_Y],
                [BASELINE_ENDS[1], BASELINE_Y],
            );
            if baseline < BASELINE_WIDTH / 2.0 {
                return Some(BASELINE);
            }
            for ([left, top, width, height], hue) in BANDS {
                if (left..left + width).contains(&p[0]) && (top..top + height).contains(&p[1]) {
                    return Some(over_background(hue, BAND_ALPHA));
                }
            }
        }
        if inside > -EDGE_WIDTH {
            return Some(EDGE);
        }
        Some(BACKGROUND)
    }

    fn paint(&self) -> RgbaImage {
        let size = self.size as u32;
        let step = 1.0 / f32::from(SUBSAMPLES);
        let samples = f32::from(SUBSAMPLES) * f32::from(SUBSAMPLES);
        RgbaImage::from_fn(size, size, |x, y| {
            let mut sum = [0.0f32; 4];
            for j in 0..SUBSAMPLES {
                for i in 0..SUBSAMPLES {
                    let p = [
                        (coordinate(x) + (f32::from(i) + 0.5) * step) * self.scale,
                        (coordinate(y) + (f32::from(j) + 0.5) * step) * self.scale,
                    ];
                    if let Some(color) = self.color_at(p) {
                        for (acc, channel) in sum.iter_mut().zip(color) {
                            *acc += f32::from(channel);
                        }
                    }
                }
            }
            // Every shape is opaque: the color is the mean over the covered
            // samples, the alpha the covered share of the pixel.
            let covered = sum[3] / 255.0;
            if covered == 0.0 {
                return Rgba([0, 0, 0, 0]);
            }
            Rgba([
                channel(sum[0] / covered),
                channel(sum[1] / covered),
                channel(sum[2] / covered),
                channel(255.0 * covered / samples),
            ])
        })
    }
}

/// Signed distance from `p` to the rounded square's edge, in units:
/// negative inside.
fn rounded_square(p: [f32; 2]) -> f32 {
    let half = UNITS / 2.0 - MARGIN - CORNER;
    let q = [
        (p[0] - UNITS / 2.0).abs() - half,
        (p[1] - UNITS / 2.0).abs() - half,
    ];
    q[0].max(0.0).hypot(q[1].max(0.0)) + q[0].max(q[1]).min(0.0) - CORNER
}

/// Distance from `p` to the segment from `a` to `b`.
fn to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1])).clamp(0.0, 1.0);
    (ap[0] - t * ab[0]).hypot(ap[1] - t * ab[1])
}

/// `hue` laid over the ground at `alpha`, as an opaque color.
fn over_background(hue: [u8; 3], alpha: f32) -> [u8; 4] {
    let mix =
        |i: usize| channel(f32::from(hue[i]) * alpha + f32::from(BACKGROUND[i]) * (1.0 - alpha));
    [mix(0), mix(1), mix(2), 0xff]
}

/// The pixel coordinate as a float; the image is at most 256 wide.
fn coordinate(pixel: u32) -> f32 {
    u16::try_from(pixel).map_or(f32::MAX, f32::from)
}

/// Rounded and clamped to the channel's range.
fn channel(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// An `.ico` holding PNG-compressed images, which Windows has read since
/// Vista: a header, a directory entry per image, then the images.
fn write_ico(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut images = Vec::new();
    for size in ICO_SIZES {
        let mut png = Cursor::new(Vec::new());
        Design::for_size(size)
            .paint()
            .write_to(&mut png, ImageFormat::Png)?;
        images.push((size, png.into_inner()));
    }
    let mut file = fs::File::create(path)?;
    let count = u16::try_from(images.len())?;
    file.write_all(&[0, 0, 1, 0])?;
    file.write_all(&count.to_le_bytes())?;
    let mut offset = u32::try_from(6 + 16 * images.len())?;
    for (size, png) in &images {
        // A width or height of 256 is written as 0.
        let extent = u8::try_from(*size).unwrap_or(0);
        let length = u32::try_from(png.len())?;
        file.write_all(&[extent, extent, 0, 0])?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&32u16.to_le_bytes())?;
        file.write_all(&length.to_le_bytes())?;
        file.write_all(&offset.to_le_bytes())?;
        offset += length;
    }
    for (_, png) in &images {
        file.write_all(png)?;
    }
    Ok(())
}
