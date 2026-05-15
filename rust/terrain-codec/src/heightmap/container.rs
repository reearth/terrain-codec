//! PNG / WebP / AVIF container helpers for heightmap RGB bytes.
//!
//! Each container is gated on its own cargo feature so callers only pay
//! the compile-time cost of what they actually use:
//!
//! | Feature | Provides            | Backend                            |
//! |---------|---------------------|------------------------------------|
//! | `png`   | [`rgb_to_png`]      | `image/png`                        |
//! | `webp`  | [`rgb_to_webp`]     | `image/webp` (lossless)            |
//! | `avif`  | [`rgb_to_avif`]     | `image/avif` (ravif, encode-only)  |
//!
//! [`decode_image`] auto-detects whichever formats are compiled in. WebP
//! encoding is **lossless** — lossy WebP would need `libwebp` which is
//! out of scope here.
//!
//! For runtime-chosen container format use the [`ContainerFormat`] enum
//! and the dispatching [`rgb_to_container`]; calling with a format whose
//! feature wasn't enabled returns [`ContainerError::Unsupported`] rather
//! than failing to compile.

use std::fmt;
use std::io::Cursor;
use std::str::FromStr;

#[cfg(feature = "avif")]
use image::codecs::avif::AvifEncoder;
#[cfg(feature = "png")]
use image::codecs::png::PngEncoder;
#[cfg(feature = "webp")]
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageReader};

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

/// Identifies one of the supported image container formats for the
/// runtime-dispatched [`rgb_to_container`] entry point.
///
/// All three variants are always present in the enum so callers can
/// parse user-supplied format names regardless of which cargo features
/// were enabled at compile time. Encoding into a format whose feature is
/// not enabled returns [`ContainerError::Unsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerFormat {
    /// PNG.
    Png,
    /// Lossless WebP.
    Webp,
    /// AVIF (encode-only).
    Avif,
}

impl ContainerFormat {
    /// All variants, in declaration order.
    pub const ALL: [Self; 3] = [Self::Png, Self::Webp, Self::Avif];

    /// Canonical lowercase name (`"png"` / `"webp"` / `"avif"`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Avif => "avif",
        }
    }

    /// IANA MIME type for the format.
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
        }
    }

    /// Whether the encoder for this format was compiled in at build time.
    pub const fn is_enabled(self) -> bool {
        match self {
            Self::Png => cfg!(feature = "png"),
            Self::Webp => cfg!(feature = "webp"),
            Self::Avif => cfg!(feature = "avif"),
        }
    }
}

impl fmt::Display for ContainerFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Error returned by [`ContainerFormat::from_str`] for an unrecognised name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseContainerFormatError {
    /// The input string that failed to parse.
    pub input: String,
}

impl fmt::Display for ParseContainerFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown container format `{}` (expected one of: png, webp, avif)",
            self.input
        )
    }
}

impl std::error::Error for ParseContainerFormatError {}

impl FromStr for ContainerFormat {
    type Err = ParseContainerFormatError;

    /// Parses case-insensitively. Accepts the canonical lowercase names
    /// as well as the `image/<name>` MIME shorthand.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "png" | "image/png" => Ok(Self::Png),
            "webp" | "image/webp" => Ok(Self::Webp),
            "avif" | "image/avif" => Ok(Self::Avif),
            _ => Err(ParseContainerFormatError {
                input: s.to_string(),
            }),
        }
    }
}

/// Error returned by [`rgb_to_container`].
#[derive(Debug)]
pub enum ContainerError {
    /// The underlying `image` encoder failed.
    Image(ImageError),
    /// The requested format's cargo feature was not enabled at build time.
    Unsupported(ContainerFormat),
}

impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image(e) => write!(f, "container encoding failed: {e}"),
            Self::Unsupported(fmt_) => write!(
                f,
                "container format `{fmt_}` is not supported in this build — enable the `{fmt_}` cargo feature"
            ),
        }
    }
}

impl std::error::Error for ContainerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Image(e) => Some(e),
            Self::Unsupported(_) => None,
        }
    }
}

impl From<ImageError> for ContainerError {
    fn from(value: ImageError) -> Self {
        Self::Image(value)
    }
}

/// Wrap raw `width × height × 3` RGB bytes in the chosen container format.
///
/// This is the runtime-dispatched counterpart of the per-format
/// [`rgb_to_png`] / [`rgb_to_webp`] / [`rgb_to_avif`] functions. Useful
/// when the format is determined at runtime (CLI flag, query param,
/// `Accept` header).
///
/// Returns [`ContainerError::Unsupported`] when the requested format's
/// cargo feature was not enabled.
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
pub fn rgb_to_container(
    format: ContainerFormat,
    rgb: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, ContainerError> {
    match format {
        ContainerFormat::Png => {
            #[cfg(feature = "png")]
            {
                Ok(rgb_to_png(rgb, width, height)?)
            }
            #[cfg(not(feature = "png"))]
            {
                let _ = (rgb, width, height);
                Err(ContainerError::Unsupported(ContainerFormat::Png))
            }
        }
        ContainerFormat::Webp => {
            #[cfg(feature = "webp")]
            {
                Ok(rgb_to_webp(rgb, width, height)?)
            }
            #[cfg(not(feature = "webp"))]
            {
                let _ = (rgb, width, height);
                Err(ContainerError::Unsupported(ContainerFormat::Webp))
            }
        }
        ContainerFormat::Avif => {
            #[cfg(feature = "avif")]
            {
                Ok(rgb_to_avif(rgb, width, height)?)
            }
            #[cfg(not(feature = "avif"))]
            {
                let _ = (rgb, width, height);
                Err(ContainerError::Unsupported(ContainerFormat::Avif))
            }
        }
    }
}

/// Wrap raw `width × height × 3` RGB bytes in a PNG container.
///
/// Available behind the `png` cargo feature.
///
/// # Errors
///
/// Returns [`ImageError`] if the underlying encoder fails (very rare for
/// valid RGB inputs — typically only OOM).
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
#[cfg(feature = "png")]
pub fn rgb_to_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ImageError> {
    assert_rgb_len(rgb, width, height);
    let mut out = Vec::with_capacity(rgb.len());
    PngEncoder::new(&mut out).write_image(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// Wrap raw `width × height × 3` RGB bytes in a lossless WebP container.
///
/// Available behind the `webp` cargo feature.
///
/// # Errors
///
/// Returns [`ImageError`] on encode failure.
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
#[cfg(feature = "webp")]
pub fn rgb_to_webp(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ImageError> {
    assert_rgb_len(rgb, width, height);
    let mut out = Vec::with_capacity(rgb.len() / 2);
    WebPEncoder::new_lossless(&mut out).write_image(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// Wrap raw `width × height × 3` RGB bytes in an AVIF container.
///
/// Available behind the `avif` cargo feature, which pulls in the pure-Rust
/// [`ravif`](https://docs.rs/ravif) encoder.
///
/// **Encode-only:** [`decode_image`] cannot decode AVIF without the
/// system `libdav1d` library. If you need to decode AVIF, enable
/// `image/avif-native` in your own dependency declaration and provide
/// libdav1d at link time.
///
/// # Errors
///
/// Returns [`ImageError`] on encode failure.
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
#[cfg(feature = "avif")]
pub fn rgb_to_avif(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ImageError> {
    assert_rgb_len(rgb, width, height);
    let mut out = Vec::with_capacity(rgb.len() / 4);
    AvifEncoder::new(&mut out).write_image(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// Decode container bytes to raw RGB. The format is auto-detected from
/// the bytes' header.
///
/// Only formats whose cargo features are enabled will be recognised —
/// e.g. with just `png` on, this can decode PNG but not WebP. AVIF
/// decoding additionally requires `image/avif-native` (libdav1d) which
/// is not enabled by our `avif` feature.
///
/// Pixels with alpha are dropped (the `image` crate decodes to RGBA
/// internally and we keep only the RGB channels).
///
/// # Errors
///
/// Returns [`ImageError`] if the bytes are not in a recognised format
/// or the decoder fails.
pub fn decode_image(bytes: &[u8]) -> Result<DecodedImage, ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let img = reader.decode()?;
    let width = img.width();
    let height = img.height();
    let rgb = img.into_rgb8().into_raw();
    Ok(DecodedImage { rgb, width, height })
}

#[track_caller]
fn assert_rgb_len(rgb: &[u8], width: u32, height: u32) {
    let expected = (width as usize) * (height as usize) * 3;
    assert_eq!(
        rgb.len(),
        expected,
        "rgb length mismatch: expected {expected}, got {}",
        rgb.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heightmap::{HeightmapFormat, decode, encode};

    fn sample_rgb(width: u32, height: u32) -> Vec<u8> {
        let elevations: Vec<f32> = (0..(width * height) as usize)
            .map(|i| i as f32 * 10.0)
            .collect();
        encode(HeightmapFormat::Terrarium, &elevations, width, height)
    }

    #[test]
    fn container_format_round_trips_through_from_str() {
        for fmt in ContainerFormat::ALL {
            let parsed: ContainerFormat = fmt.to_string().parse().unwrap();
            assert_eq!(parsed, fmt);
            // MIME alias also works.
            let mime: ContainerFormat = fmt.mime_type().parse().unwrap();
            assert_eq!(mime, fmt);
        }
        assert!("bogus".parse::<ContainerFormat>().is_err());
    }

    #[test]
    fn is_enabled_reflects_features() {
        assert_eq!(ContainerFormat::Png.is_enabled(), cfg!(feature = "png"));
        assert_eq!(ContainerFormat::Webp.is_enabled(), cfg!(feature = "webp"));
        assert_eq!(ContainerFormat::Avif.is_enabled(), cfg!(feature = "avif"));
    }

    #[test]
    fn dispatch_returns_unsupported_for_disabled_features() {
        let rgb = sample_rgb(4, 4);
        for fmt in ContainerFormat::ALL {
            let result = rgb_to_container(fmt, &rgb, 4, 4);
            match (fmt.is_enabled(), &result) {
                (true, Ok(_)) => {}
                (false, Err(ContainerError::Unsupported(f))) => assert_eq!(*f, fmt),
                other => panic!(
                    "unexpected combination: enabled={:?} {other:?}",
                    fmt.is_enabled()
                ),
            }
        }
    }

    #[cfg(feature = "png")]
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

    #[cfg(feature = "avif")]
    #[test]
    fn avif_encodes_to_valid_container() {
        let rgb = sample_rgb(8, 8);
        let avif = rgb_to_avif(&rgb, 8, 8).unwrap();
        // AVIF files have an `ftypavif` brand in the first ISO BMFF box.
        assert!(
            avif.windows(8).any(|w| w == b"ftypavif"),
            "expected AVIF brand in output"
        );
    }

    #[cfg(all(feature = "webp", feature = "png"))]
    #[test]
    fn webp_roundtrip_through_codec() {
        let rgb = sample_rgb(8, 8);
        let webp = rgb_to_webp(&rgb, 8, 8).unwrap();
        // WebP files start with "RIFF" .... "WEBP".
        assert_eq!(&webp[..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");
        let decoded = decode_image(&webp).unwrap();
        assert_eq!((decoded.width, decoded.height), (8, 8));
        assert_eq!(decoded.rgb, rgb);
    }
}
