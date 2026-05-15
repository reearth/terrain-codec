//! Heightmap codecs for elevation tile formats.
//!
//! Two flavours of heightmap encoding are supported:
//!
//! - **RGB tiles** (this module, top level): [`terrarium`], [`mapbox`],
//!   [`gsi`]. Elevations packed into 3-byte `(R, G, B)` pixels, usually
//!   served wrapped in a PNG or WebP container.
//! - **Cesium heightmap-1.0** ([`cesium`]): 16-bit little-endian heights
//!   plus child-tile mask. Cesium's legacy terrain format.
//!
//! The [`container`] submodule (behind the `image` cargo feature) wraps
//! the encoded RGB bytes in PNG or WebP for serving as image tiles.
//!
//! Three formats are supported, each as a `(encode_pixel, decode_pixel,
//! encode, decode)` quadruplet:
//!
//! - [`terrarium`] — Mapzen / Tilezen / Stadia "Terrarium" encoding.
//!   `elevation = (R * 256 + G + B / 256) - 32768`
//! - [`mapbox`] — Mapbox "Terrain-RGB" encoding.
//!   `elevation = -10000 + (R * 65536 + G * 256 + B) * 0.1`
//! - [`gsi`] — Geospatial Information Authority of Japan
//!   ([地理院](https://maps.gsi.go.jp/development/demtile.html))
//!   signed-integer DEM PNG. `(128, 0, 0)` is the no-data sentinel
//!   (decodes to `NaN`).
//!
//! Per-pixel functions (`encode_pixel` / `decode_pixel`) convert single
//! samples and are useful for streaming, hot loops, custom layouts, and
//! tests. Bulk `encode` / `decode` are thin wrappers that walk row-major
//! `width × height` buffers.
//!
//! ## Unified runtime-dispatch API
//!
//! When the format is only known at runtime (CLI flag, request param,
//! config file), use the [`HeightmapFormat`] enum and the top-level
//! [`encode_pixel`], [`decode_pixel`], [`encode`], [`decode`] functions:
//!
//! ```
//! use terrain_codec::heightmap::{HeightmapFormat, encode_pixel};
//! let fmt: HeightmapFormat = "terrarium".parse().unwrap();
//! let rgb = encode_pixel(fmt, 123.45);
//! ```
//!
//! The codecs operate on raw `(R, G, B)` byte triplets and produce flat
//! row-major buffers, so they're agnostic to the container format. Wrap
//! the encoded bytes in PNG/WebP yourself (e.g. via the `image` crate)
//! depending on your service.

use std::fmt;
use std::str::FromStr;

pub mod cesium;
#[cfg(feature = "image")]
pub mod container;

/// Identifies one of the supported RGB heightmap encodings, for
/// runtime-dispatched encode/decode via the top-level functions
/// ([`encode_pixel`], [`decode_pixel`], [`encode`], [`decode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeightmapFormat {
    /// Mapzen / Tilezen / Stadia "Terrarium" encoding.
    Terrarium,
    /// Mapbox "Terrain-RGB" encoding.
    Mapbox,
    /// GSI / 国土地理院 DEM tile encoding.
    Gsi,
}

impl HeightmapFormat {
    /// All supported formats, in declaration order. Useful for iterating
    /// in tests or building a format-picker UI.
    pub const ALL: [HeightmapFormat; 3] = [Self::Terrarium, Self::Mapbox, Self::Gsi];

    /// Canonical lowercase name (`"terrarium"` / `"mapbox"` / `"gsi"`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Terrarium => "terrarium",
            Self::Mapbox => "mapbox",
            Self::Gsi => "gsi",
        }
    }
}

impl fmt::Display for HeightmapFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Error returned by `HeightmapFormat::from_str` for an unrecognised name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseHeightmapFormatError {
    /// The input string that failed to parse.
    pub input: String,
}

impl fmt::Display for ParseHeightmapFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown heightmap format `{}` (expected one of: terrarium, mapbox, gsi)",
            self.input
        )
    }
}

impl std::error::Error for ParseHeightmapFormatError {}

impl FromStr for HeightmapFormat {
    type Err = ParseHeightmapFormatError;

    /// Parses case-insensitively. Accepts `"terrarium"`, `"mapbox"` (or
    /// the alias `"mapbox-rgb"` / `"terrain-rgb"`), and `"gsi"` (or
    /// `"gsi-dem"`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "terrarium" => Ok(Self::Terrarium),
            "mapbox" | "mapbox-rgb" | "terrain-rgb" => Ok(Self::Mapbox),
            "gsi" | "gsi-dem" => Ok(Self::Gsi),
            _ => Err(ParseHeightmapFormatError {
                input: s.to_string(),
            }),
        }
    }
}

/// Encode a single elevation sample (metres) into `(R, G, B)` using the
/// chosen format.
#[inline]
pub fn encode_pixel(format: HeightmapFormat, elevation: f32) -> [u8; 3] {
    match format {
        HeightmapFormat::Terrarium => terrarium::encode_pixel(elevation),
        HeightmapFormat::Mapbox => mapbox::encode_pixel(elevation),
        HeightmapFormat::Gsi => gsi::encode_pixel(elevation),
    }
}

/// Decode a single `(R, G, B)` pixel into elevation (metres) using the
/// chosen format.
#[inline]
pub fn decode_pixel(format: HeightmapFormat, rgb: [u8; 3]) -> f32 {
    match format {
        HeightmapFormat::Terrarium => terrarium::decode_pixel(rgb),
        HeightmapFormat::Mapbox => mapbox::decode_pixel(rgb),
        HeightmapFormat::Gsi => gsi::decode_pixel(rgb),
    }
}

/// Encode `elevations` (metres) into a flat `width × height × 3` RGB
/// buffer using the chosen format.
///
/// # Panics
///
/// Panics if `elevations.len() != (width * height) as usize`.
pub fn encode(format: HeightmapFormat, elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
    match format {
        HeightmapFormat::Terrarium => terrarium::encode(elevations, width, height),
        HeightmapFormat::Mapbox => mapbox::encode(elevations, width, height),
        HeightmapFormat::Gsi => gsi::encode(elevations, width, height),
    }
}

/// Decode a flat `width × height × 3` RGB buffer back to elevations
/// using the chosen format.
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
pub fn decode(format: HeightmapFormat, rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
    match format {
        HeightmapFormat::Terrarium => terrarium::decode(rgb, width, height),
        HeightmapFormat::Mapbox => mapbox::decode(rgb, width, height),
        HeightmapFormat::Gsi => gsi::decode(rgb, width, height),
    }
}

/// Mapzen / Tilezen / Stadia "Terrarium" elevation tile encoding.
///
/// Formula: `elevation = (R * 256 + G + B / 256) - 32768` (metres).
///
/// Reference: <https://github.com/tilezen/joerd/blob/master/docs/formats.md#terrarium>
pub mod terrarium {
    /// Encode a single elevation sample (metres) into Terrarium `(R, G, B)`.
    ///
    /// Values are clamped to the representable range
    /// `[-32768, 8388.99…]` metres. `NaN` encodes as zero metres
    /// (mid-range — Terrarium has no dedicated no-data sentinel).
    #[inline]
    pub fn encode_pixel(elevation: f32) -> [u8; 3] {
        let v = if elevation.is_nan() {
            0.0
        } else {
            (elevation + 32768.0) * 256.0
        };
        let v = v.clamp(0.0, (1u32 << 24) as f32 - 1.0) as u32;
        [
            ((v >> 16) & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
        ]
    }

    /// Decode a single Terrarium `(R, G, B)` pixel into elevation (metres).
    #[inline]
    pub fn decode_pixel(rgb: [u8; 3]) -> f32 {
        let r = rgb[0] as f32;
        let g = rgb[1] as f32;
        let b = rgb[2] as f32;
        r * 256.0 + g + b / 256.0 - 32768.0
    }

    /// Encode `elevations` (metres) into a flat `width × height × 3` RGB buffer.
    ///
    /// Walks `elevations` in row-major order and delegates to
    /// [`encode_pixel`] for each sample.
    ///
    /// # Panics
    ///
    /// Panics if `elevations.len() != (width * height) as usize`.
    pub fn encode(elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
        let expected = (width as usize) * (height as usize);
        assert_eq!(
            elevations.len(),
            expected,
            "elevations length mismatch: expected {expected}, got {}",
            elevations.len()
        );
        let mut out = Vec::with_capacity(expected * 3);
        for &e in elevations {
            out.extend_from_slice(&encode_pixel(e));
        }
        out
    }

    /// Decode a flat `width × height × 3` RGB buffer back to elevations.
    ///
    /// # Panics
    ///
    /// Panics if `rgb.len() != (width * height * 3) as usize`.
    pub fn decode(rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
        let pixels = (width as usize) * (height as usize);
        assert_eq!(
            rgb.len(),
            pixels * 3,
            "rgb length mismatch: expected {}, got {}",
            pixels * 3,
            rgb.len()
        );
        rgb.chunks_exact(3)
            .map(|c| decode_pixel([c[0], c[1], c[2]]))
            .collect()
    }
}

/// Mapbox "Terrain-RGB" elevation tile encoding.
///
/// Formula: `elevation = -10000 + (R * 65536 + G * 256 + B) * 0.1` (metres).
///
/// Reference: <https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-rgb-v1/>
pub mod mapbox {
    /// Encode a single elevation sample (metres) into Mapbox `(R, G, B)`.
    ///
    /// Values are clamped to the representable range
    /// `[-10000, +1667721.5]` metres. `NaN` encodes as zero metres
    /// (Mapbox Terrain-RGB has no dedicated no-data sentinel).
    #[inline]
    pub fn encode_pixel(elevation: f32) -> [u8; 3] {
        let v = if elevation.is_nan() {
            0.0
        } else {
            ((elevation + 10000.0) * 10.0).round()
        };
        let v = v.clamp(0.0, (1u32 << 24) as f32 - 1.0) as u32;
        [
            ((v >> 16) & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
        ]
    }

    /// Decode a single Mapbox Terrain-RGB `(R, G, B)` pixel into elevation
    /// (metres).
    #[inline]
    pub fn decode_pixel(rgb: [u8; 3]) -> f32 {
        let r = rgb[0] as f32;
        let g = rgb[1] as f32;
        let b = rgb[2] as f32;
        -10000.0 + (r * 65536.0 + g * 256.0 + b) * 0.1
    }

    /// Encode `elevations` (metres) into a flat `width × height × 3` RGB buffer.
    ///
    /// # Panics
    ///
    /// Panics if `elevations.len() != (width * height) as usize`.
    pub fn encode(elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
        let expected = (width as usize) * (height as usize);
        assert_eq!(
            elevations.len(),
            expected,
            "elevations length mismatch: expected {expected}, got {}",
            elevations.len()
        );
        let mut out = Vec::with_capacity(expected * 3);
        for &e in elevations {
            out.extend_from_slice(&encode_pixel(e));
        }
        out
    }

    /// Decode a flat `width × height × 3` RGB buffer back to elevations.
    ///
    /// # Panics
    ///
    /// Panics if `rgb.len() != (width * height * 3) as usize`.
    pub fn decode(rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
        let pixels = (width as usize) * (height as usize);
        assert_eq!(
            rgb.len(),
            pixels * 3,
            "rgb length mismatch: expected {}, got {}",
            pixels * 3,
            rgb.len()
        );
        rgb.chunks_exact(3)
            .map(|c| decode_pixel([c[0], c[1], c[2]]))
            .collect()
    }
}

/// Geospatial Information Authority of Japan (GSI / 国土地理院) DEM tile
/// encoding.
///
/// Each pixel packs a signed 24-bit integer of 0.01 m units:
///
/// ```text
/// x = (R << 16) | (G << 8) | B
/// if x == 0x800000  → NaN (no-data sentinel = RGB(128, 0, 0))
/// if x >= 0x800000  → elevation = (x - 0x1000000) * 0.01
/// else              → elevation = x * 0.01
/// ```
///
/// Reference: <https://maps.gsi.go.jp/development/demtile.html>
pub mod gsi {
    /// No-data sentinel `(R=128, G=0, B=0)`, decoded as `NaN`.
    pub const SENTINEL_RGB: [u8; 3] = [0x80, 0x00, 0x00];
    /// 2^23 — boundary between positive and negative values in the 24-bit
    /// two's complement representation. Also the bit pattern of the
    /// no-data sentinel.
    const SIGN_BIT: u32 = 1 << 23;
    /// 2^24 — modulus used to wrap into negative values.
    const RANGE: i64 = 1 << 24;

    /// Encode a single elevation sample (metres) into GSI `(R, G, B)`.
    ///
    /// `NaN` encodes as the no-data sentinel `(128, 0, 0)`. Finite values
    /// are quantised to 0.01 m and stored as a 24-bit signed integer,
    /// wrapping out-of-range values modulo 2²⁴ (matching the GSI spec).
    #[inline]
    pub fn encode_pixel(elevation: f32) -> [u8; 3] {
        if elevation.is_nan() {
            return SENTINEL_RGB;
        }
        let raw = (elevation as f64 * 100.0).round() as i64;
        let x = raw.rem_euclid(RANGE) as u32;
        [
            ((x >> 16) & 0xff) as u8,
            ((x >> 8) & 0xff) as u8,
            (x & 0xff) as u8,
        ]
    }

    /// Decode a single GSI `(R, G, B)` pixel into elevation (metres).
    ///
    /// The no-data sentinel `(128, 0, 0)` decodes to `NaN`.
    #[inline]
    pub fn decode_pixel(rgb: [u8; 3]) -> f32 {
        let r = rgb[0] as u32;
        let g = rgb[1] as u32;
        let b = rgb[2] as u32;
        let x = (r << 16) | (g << 8) | b;
        if x == SIGN_BIT {
            f32::NAN
        } else if x >= SIGN_BIT {
            (x as i64 - RANGE) as f32 * 0.01
        } else {
            x as f32 * 0.01
        }
    }

    /// Encode `elevations` (metres) into a flat `width × height × 3` RGB buffer.
    ///
    /// # Panics
    ///
    /// Panics if `elevations.len() != (width * height) as usize`.
    pub fn encode(elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
        let expected = (width as usize) * (height as usize);
        assert_eq!(
            elevations.len(),
            expected,
            "elevations length mismatch: expected {expected}, got {}",
            elevations.len()
        );
        let mut out = Vec::with_capacity(expected * 3);
        for &e in elevations {
            out.extend_from_slice(&encode_pixel(e));
        }
        out
    }

    /// Decode a flat `width × height × 3` RGB buffer back to elevations.
    ///
    /// The no-data sentinel `(128, 0, 0)` decodes to `NaN`.
    ///
    /// # Panics
    ///
    /// Panics if `rgb.len() != (width * height * 3) as usize`.
    pub fn decode(rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
        let pixels = (width as usize) * (height as usize);
        assert_eq!(
            rgb.len(),
            pixels * 3,
            "rgb length mismatch: expected {}, got {}",
            pixels * 3,
            rgb.len()
        );
        rgb.chunks_exact(3)
            .map(|c| decode_pixel([c[0], c[1], c[2]]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn terrarium_pixel_roundtrip() {
        for e in [-100.0_f32, 0.0, 100.0, 1234.5, 8000.0, -500.0] {
            let back = terrarium::decode_pixel(terrarium::encode_pixel(e));
            assert!(approx_eq(e, back, 0.01), "{e} → {back}");
        }
    }

    #[test]
    fn terrarium_zero_sea_level_is_8000() {
        // (R, G, B) = (128, 0, 0) ↔ elevation 0 m (sea level)
        assert_eq!(terrarium::encode_pixel(0.0), [0x80, 0x00, 0x00]);
    }

    #[test]
    fn terrarium_bulk_matches_pixel() {
        let elevations: Vec<f32> = vec![-100.0, 0.0, 100.0, 1234.5, 8000.0, -500.0];
        let bulk = terrarium::encode(&elevations, 6, 1);
        let from_pixels: Vec<u8> = elevations
            .iter()
            .flat_map(|&e| terrarium::encode_pixel(e))
            .collect();
        assert_eq!(bulk, from_pixels);
    }

    #[test]
    fn mapbox_pixel_roundtrip() {
        for e in [-100.0_f32, 0.0, 100.0, 1234.5, 8000.0, -500.0] {
            let back = mapbox::decode_pixel(mapbox::encode_pixel(e));
            assert!(approx_eq(e, back, 0.1), "{e} → {back}");
        }
    }

    #[test]
    fn mapbox_minimum_value_is_minus_10000() {
        assert_eq!(mapbox::encode_pixel(-10000.0), [0, 0, 0]);
        assert_eq!(mapbox::decode_pixel([0, 0, 0]), -10000.0);
    }

    #[test]
    fn gsi_sentinel_decodes_to_nan() {
        assert!(gsi::decode_pixel([0x80, 0x00, 0x00]).is_nan());
    }

    #[test]
    fn gsi_nan_encodes_to_sentinel() {
        assert_eq!(gsi::encode_pixel(f32::NAN), [0x80, 0x00, 0x00]);
    }

    #[test]
    fn gsi_pixel_roundtrip_positive_and_negative() {
        for e in [0.0_f32, 100.0, 3776.24, -10.5, -429.4] {
            let back = gsi::decode_pixel(gsi::encode_pixel(e));
            assert!(approx_eq(e, back, 0.01), "{e} → {back}");
        }
    }

    #[test]
    fn gsi_zero_is_all_zero_rgb() {
        assert_eq!(gsi::encode_pixel(0.0), [0, 0, 0]);
    }

    #[test]
    fn format_from_str_accepts_aliases() {
        assert_eq!("terrarium".parse(), Ok(HeightmapFormat::Terrarium));
        assert_eq!("TERRARIUM".parse(), Ok(HeightmapFormat::Terrarium));
        assert_eq!("mapbox".parse(), Ok(HeightmapFormat::Mapbox));
        assert_eq!("mapbox-rgb".parse(), Ok(HeightmapFormat::Mapbox));
        assert_eq!("terrain-rgb".parse(), Ok(HeightmapFormat::Mapbox));
        assert_eq!("gsi".parse(), Ok(HeightmapFormat::Gsi));
        assert_eq!("gsi-dem".parse(), Ok(HeightmapFormat::Gsi));
        assert!("bogus".parse::<HeightmapFormat>().is_err());
    }

    #[test]
    fn format_display_roundtrips_through_from_str() {
        for fmt in HeightmapFormat::ALL {
            let parsed: HeightmapFormat = fmt.to_string().parse().unwrap();
            assert_eq!(parsed, fmt);
        }
    }

    #[test]
    fn dispatch_matches_per_module_for_every_format() {
        let elevations: Vec<f32> = vec![-100.0, 0.0, 100.0, 1234.5, -500.0];
        for fmt in HeightmapFormat::ALL {
            // Bulk via dispatch == bulk via the module directly.
            let dispatched = encode(fmt, &elevations, elevations.len() as u32, 1);
            let direct = match fmt {
                HeightmapFormat::Terrarium => terrarium::encode(&elevations, 5, 1),
                HeightmapFormat::Mapbox => mapbox::encode(&elevations, 5, 1),
                HeightmapFormat::Gsi => gsi::encode(&elevations, 5, 1),
            };
            assert_eq!(dispatched, direct, "encode mismatch for {fmt}");

            // Per-pixel dispatch agrees too.
            for &e in &elevations {
                let px = encode_pixel(fmt, e);
                let back = decode_pixel(fmt, px);
                // Round-trip tolerance varies per format; pick the loosest.
                assert!((e - back).abs() <= 0.1, "[{fmt}] {e} → {px:?} → {back}");
            }
        }
    }

    #[test]
    fn gsi_bulk_matches_pixel() {
        let elevations: Vec<f32> = vec![0.0, 100.0, -10.5, f32::NAN, 3776.24];
        let bulk = gsi::encode(&elevations, elevations.len() as u32, 1);
        let from_pixels: Vec<u8> = elevations
            .iter()
            .flat_map(|&e| gsi::encode_pixel(e))
            .collect();
        assert_eq!(bulk, from_pixels);
    }
}
