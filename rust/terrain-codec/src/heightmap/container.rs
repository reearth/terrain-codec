//! PNG / WebP container helpers for heightmap RGB bytes.
//!
//! Available behind the `image` cargo feature. Uses the
//! [`image`](https://docs.rs/image) crate under the hood, with PNG and
//! WebP support compiled in. WebP encoding is **lossless** — lossy WebP
//! requires `libwebp` which is out of scope here.
//!
//! ```no_run
//! use terrain_codec::heightmap::{HeightmapFormat, encode};
//! use terrain_codec::heightmap::container;
//!
//! let elevations: Vec<f32> = vec![0.0; 256 * 256];
//! let rgb = encode(HeightmapFormat::Terrarium, &elevations, 256, 256);
//! let png_bytes = container::rgb_to_png(&rgb, 256, 256).unwrap();
//! ```

use std::io::Cursor;

use image::ImageReader;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder};

/// Re-export of [`image::ImageError`] for callers that don't want to
/// pull in the `image` crate directly.
pub type ImageError = image::ImageError;

/// A decoded image returned by [`decode_image`].
#[derive(Debug, Clone)]
pub struct DecodedImage {
    /// Flat row-major RGB bytes (3 bytes per pixel).
    pub rgb: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Wrap raw `width × height × 3` RGB bytes in a PNG container.
///
/// # Errors
///
/// Returns [`ImageError`] if the underlying encoder fails (very rare for
/// valid RGB inputs — typically only OOM).
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
pub fn rgb_to_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ImageError> {
    let expected = (width as usize) * (height as usize) * 3;
    assert_eq!(
        rgb.len(),
        expected,
        "rgb length mismatch: expected {expected}, got {}",
        rgb.len()
    );
    let mut out = Vec::with_capacity(expected);
    PngEncoder::new(&mut out).write_image(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// Wrap raw `width × height × 3` RGB bytes in a lossless WebP container.
///
/// # Errors
///
/// Returns [`ImageError`] on encode failure.
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
pub fn rgb_to_webp(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ImageError> {
    let expected = (width as usize) * (height as usize) * 3;
    assert_eq!(
        rgb.len(),
        expected,
        "rgb length mismatch: expected {expected}, got {}",
        rgb.len()
    );
    let mut out = Vec::with_capacity(expected / 2);
    WebPEncoder::new_lossless(&mut out).write_image(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// Decode PNG or WebP bytes to raw RGB. The format is auto-detected from
/// the bytes' header.
///
/// Pixels with alpha are dropped (the `image` crate decodes to RGBA
/// internally and we keep only the RGB channels).
///
/// # Errors
///
/// Returns [`ImageError`] if the bytes are not a recognised PNG/WebP, or
/// the decoder fails.
pub fn decode_image(bytes: &[u8]) -> Result<DecodedImage, ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let img = reader.decode()?;
    let width = img.width();
    let height = img.height();
    let rgb = img.into_rgb8().into_raw();
    Ok(DecodedImage { rgb, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heightmap::{HeightmapFormat, decode, encode};

    #[test]
    fn png_roundtrip_through_codec() {
        let width = 8u32;
        let height = 8u32;
        let elevations: Vec<f32> = (0..(width * height) as usize)
            .map(|i| i as f32 * 10.0)
            .collect();

        for fmt in [
            HeightmapFormat::Terrarium,
            HeightmapFormat::Mapbox,
            HeightmapFormat::Gsi,
        ] {
            let rgb = encode(fmt, &elevations, width, height);
            let png = rgb_to_png(&rgb, width, height).unwrap();
            assert_eq!(
                &png[..8],
                b"\x89PNG\r\n\x1a\n",
                "{fmt} should produce PNG magic"
            );
            let DecodedImage {
                rgb: rgb_back,
                width: w2,
                height: h2,
            } = decode_image(&png).unwrap();
            assert_eq!((w2, h2), (width, height));
            assert_eq!(rgb_back, rgb);
            let elev_back = decode(fmt, &rgb_back, width, height);
            for (a, b) in elevations.iter().zip(&elev_back) {
                assert!((a - b).abs() < 0.5, "{fmt}: {a} → {b}");
            }
        }
    }

    #[test]
    fn webp_roundtrip_through_codec() {
        let width = 8u32;
        let height = 8u32;
        let elevations: Vec<f32> = (0..(width * height) as usize)
            .map(|i| i as f32 * 10.0)
            .collect();
        let rgb = encode(HeightmapFormat::Terrarium, &elevations, width, height);
        let webp = rgb_to_webp(&rgb, width, height).unwrap();
        // WebP files start with "RIFF" .... "WEBP".
        assert_eq!(&webp[..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");
        let decoded = decode_image(&webp).unwrap();
        assert_eq!((decoded.width, decoded.height), (width, height));
        assert_eq!(decoded.rgb, rgb);
    }
}
