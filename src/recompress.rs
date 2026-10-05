//! The pixel side of shrinking: how many pixels an image needs for the size
//! it's drawn at, resampling to that, and encoding.

use std::io::Write;

use image::imageops::{self, FilterType};
use image::{ImageBuffer, Luma, Rgb};

use crate::raster::Pixels;
use crate::scan::Placement;

/// Images within this factor of the target aren't resampled: a 10% resize
/// costs a generation of blur for almost no saving.
const RESIZE_THRESHOLD: f64 = 0.9;

/// The largest size an image is drawn at across all its placements, in
/// inches. An image drawn small on one page and full-width on another
/// needs the full-width resolution.
pub fn drawn_inches(placements: &[Placement]) -> Option<(f64, f64)> {
    let w = placements.iter().map(|p| p.width_pt).fold(0.0, f64::max);
    let h = placements.iter().map(|p| p.height_pt).fold(0.0, f64::max);
    // Images painted at under a hundredth of an inch are usually clipped
    // tricks or broken files; their size says nothing useful.
    (w > 0.72 && h > 0.72).then_some((w / 72.0, h / 72.0))
}

/// Pixels per inch along the less dense axis, which is the one a reader
/// would notice first.
pub fn effective_dpi(width: u32, height: u32, inches: (f64, f64)) -> f64 {
    (width as f64 / inches.0).min(height as f64 / inches.1)
}

/// The pixel size to resample to for a target dpi, or None if the image
/// is already at or near it. Aspect ratio is kept, and neither axis drops
/// below the target.
pub fn target_size(width: u32, height: u32, inches: (f64, f64), dpi: u32) -> Option<(u32, u32)> {
    let need_w = inches.0 * dpi as f64;
    let need_h = inches.1 * dpi as f64;
    let scale = (need_w / width as f64).max(need_h / height as f64);
    if scale >= RESIZE_THRESHOLD {
        return None;
    }
    let w = ((width as f64 * scale).round() as u32).max(1);
    let h = ((height as f64 * scale).round() as u32).max(1);
    Some((w, h))
}

/// Lanczos3: sharp edges on printed text inside scans, which is what people
/// check, at the cost of being slower than a box filter.
pub fn resize(px: &Pixels, w: u32, h: u32) -> Pixels {
    let data = match px.channels {
        1 => {
            let img: ImageBuffer<Luma<u8>, _> =
                ImageBuffer::from_raw(px.width, px.height, px.data.as_slice())
                    .expect("size checked at decode");
            imageops::resize(&img, w, h, FilterType::Lanczos3).into_raw()
        }
        _ => {
            let img: ImageBuffer<Rgb<u8>, _> =
                ImageBuffer::from_raw(px.width, px.height, px.data.as_slice())
                    .expect("size checked at decode");
            imageops::resize(&img, w, h, FilterType::Lanczos3).into_raw()
        }
    };
    Pixels {
        width: w,
        height: h,
        channels: px.channels,
        data,
    }
}

pub fn jpeg(px: &Pixels, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality);
    // Averaged chroma instead of the default top-left pick: thin coloured
    // lines (signatures in blue ink, stamps) keep their colour.
    enc.set_chroma_subsampling_method(jpeg_encoder::ChromaSubsamplingMethod::Average);
    enc.set_optimized_huffman_tables(true);
    let color = if px.channels == 1 {
        jpeg_encoder::ColorType::Luma
    } else {
        jpeg_encoder::ColorType::Rgb
    };
    // Images over 65535 px on a side can't be JPEG; the caller keeps the
    // original when encoding fails.
    if px.width > u16::MAX as u32 || px.height > u16::MAX as u32 {
        return Vec::new();
    }
    enc.encode(&px.data, px.width as u16, px.height as u16, color)
        .map(|_| out)
        .unwrap_or_default()
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).expect("writing to a Vec can't fail");
    e.finish().expect("writing to a Vec can't fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(page: u32, w: f64, h: f64) -> Placement {
        Placement {
            page,
            width_pt: w,
            height_pt: h,
        }
    }

    #[test]
    fn a_phone_scan_on_a4_is_far_above_document_resolution() {
        // 3000 x 4000 px photo placed full-page on A4 (595 x 842 pt).
        let inches = drawn_inches(&[at(1, 595.0, 842.0)]).unwrap();
        let dpi = effective_dpi(3000, 4000, inches);
        assert!((dpi - 342.0).abs() < 1.0, "{dpi}");
        let (w, h) = target_size(3000, 4000, inches, 150).unwrap();
        assert_eq!((w, h), (1316, 1754));
    }

    #[test]
    fn the_biggest_placement_wins() {
        let inches = drawn_inches(&[at(1, 72.0, 72.0), at(3, 288.0, 144.0)]).unwrap();
        assert_eq!(inches, (4.0, 2.0));
        // 4 x 2 inches at 150 dpi needs 600 x 300; 1200 x 600 halves.
        assert_eq!(target_size(1200, 600, inches, 150), Some((600, 300)));
    }

    #[test]
    fn near_target_images_are_left_alone() {
        assert_eq!(target_size(650, 325, (4.0, 2.0), 150), None);
        assert_eq!(target_size(400, 200, (4.0, 2.0), 150), None);
    }

    #[test]
    fn invisible_placements_give_no_size() {
        assert_eq!(drawn_inches(&[at(1, 0.1, 0.1)]), None);
        assert_eq!(drawn_inches(&[]), None);
    }

    #[test]
    fn resize_then_encode_gives_a_decodable_jpeg_of_that_size() {
        let px = Pixels {
            width: 300,
            height: 200,
            channels: 3,
            data: crate::testpdf::pattern(300, 200, 3),
        };
        let small = resize(&px, 150, 100);
        assert_eq!(small.data.len(), 150 * 100 * 3);
        let jpg = jpeg(&small, 75);
        let mut d = jpeg_decoder::Decoder::new(jpg.as_slice());
        d.decode().unwrap();
        let info = d.info().unwrap();
        assert_eq!((info.width, info.height), (150, 100));
    }
}
