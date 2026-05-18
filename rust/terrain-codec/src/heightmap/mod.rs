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
//! The [`container`] submodule (behind the `png` / `webp` / `avif` cargo
//! features) wraps the encoded RGB bytes in PNG, WebP, or AVIF for serving
//! as image tiles.
//!
//! ## API shape: allocation-free by default
//!
//! Every codec ships in three flavours so callers can pick the right
//! allocation profile:
//!
//! | Function           | Output                | Allocation |
//! |--------------------|-----------------------|------------|
//! | `encode_pixel`     | `[u8; 3]`             | none       |
//! | `encode_into`      | caller-owned `&mut [u8]` | none    |
//! | `encode_to<W>`     | `impl Write`          | none (4 KiB stack buffer) |
//! | `encode`           | `Vec<u8>`             | one Vec    |
//!
//! Decoding mirrors this: `decode_pixel`, `decode_into`, [`HeightmapView`]
//! (zero-copy borrowed iterator), and `decode` (`Vec<f32>`).
//!
//! ## Unified runtime-dispatch API
//!
//! When the format is only known at runtime, use the [`HeightmapFormat`]
//! enum and the top-level [`encode_pixel`], [`decode_pixel`],
//! [`encode_into`], [`decode_into`], [`encode_to`], [`encode`], [`decode`]
//! functions:
//!
//! ```
//! use terrain_codec::heightmap::{HeightmapFormat, encode_pixel};
//! let fmt: HeightmapFormat = "terrarium".parse().unwrap();
//! let rgb = encode_pixel(fmt, 123.45);
//! ```

use std::fmt;
use std::io::{self, Write};
use std::str::FromStr;

pub mod cesium;
#[cfg(any(feature = "png", feature = "webp", feature = "avif"))]
pub mod container;

/// Identifies one of the supported RGB heightmap encodings, for
/// runtime-dispatched encode/decode via the top-level functions.
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
    /// All supported formats, in declaration order.
    pub const ALL: [HeightmapFormat; 3] = [Self::Terrarium, Self::Mapbox, Self::Gsi];

    /// Canonical lowercase name.
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

/// Encode `elevations` into a caller-owned RGB buffer.
///
/// `out.len()` must equal `elevations.len() * 3`. No allocation occurs.
pub fn encode_into(format: HeightmapFormat, elevations: &[f32], out: &mut [u8]) {
    encode_into_with(elevations, out, |e| encode_pixel(format, e))
}

/// Decode RGB bytes into a caller-owned `f32` buffer.
///
/// `rgb.len()` must equal `out.len() * 3`. No allocation occurs.
pub fn decode_into(format: HeightmapFormat, rgb: &[u8], out: &mut [f32]) {
    decode_into_with(rgb, out, |px| decode_pixel(format, px))
}

/// Encode `elevations` into RGB bytes streamed to a writer.
///
/// Uses a 4 KiB stack-allocated chunk to amortise per-call write overhead.
pub fn encode_to<W: Write>(
    format: HeightmapFormat,
    elevations: &[f32],
    writer: W,
) -> io::Result<()> {
    encode_to_with(elevations, writer, |e| encode_pixel(format, e))
}

/// Encode `elevations` into a freshly allocated RGB `Vec`.
///
/// # Panics
///
/// Panics if `elevations.len() != (width * height) as usize`.
pub fn encode(format: HeightmapFormat, elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
    let expected = (width as usize) * (height as usize);
    assert_eq!(
        elevations.len(),
        expected,
        "elevations length mismatch: expected {expected}, got {}",
        elevations.len()
    );
    let mut out = vec![0u8; expected * 3];
    encode_into(format, elevations, &mut out);
    out
}

/// Decode a flat `width × height × 3` RGB buffer back to elevations.
///
/// # Panics
///
/// Panics if `rgb.len() != (width * height * 3) as usize`.
pub fn decode(format: HeightmapFormat, rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
    let pixels = (width as usize) * (height as usize);
    assert_eq!(
        rgb.len(),
        pixels * 3,
        "rgb length mismatch: expected {}, got {}",
        pixels * 3,
        rgb.len()
    );
    let mut out = vec![0f32; pixels];
    decode_into(format, rgb, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Zero-copy decode view
// ---------------------------------------------------------------------------

/// Zero-copy borrowed view over an encoded RGB heightmap.
///
/// Holds a `&[u8]` referencing the source bytes and lazily yields decoded
/// elevations via [`Self::iter`] / [`Self::get`] — no `Vec<f32>` is
/// materialised unless the caller explicitly asks via [`Self::to_vec`] or
/// [`Self::decode_into`].
#[derive(Debug, Clone, Copy)]
pub struct HeightmapView<'a> {
    /// Format used to interpret the bytes.
    pub format: HeightmapFormat,
    /// Raw RGB bytes (`width * height * 3` bytes, row-major).
    pub rgb: &'a [u8],
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

impl<'a> HeightmapView<'a> {
    /// Wrap an RGB buffer. Asserts `rgb.len() == width * height * 3`.
    pub fn new(format: HeightmapFormat, rgb: &'a [u8], width: u32, height: u32) -> Self {
        let pixels = (width as usize) * (height as usize);
        assert_eq!(
            rgb.len(),
            pixels * 3,
            "rgb length mismatch: expected {}, got {}",
            pixels * 3,
            rgb.len()
        );
        Self {
            format,
            rgb,
            width,
            height,
        }
    }

    /// Number of pixels.
    #[inline]
    pub fn len(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// `true` if the view has zero pixels.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.rgb.is_empty()
    }

    /// Sample the elevation at `(x, y)` in pixels.
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> f32 {
        let i = (y as usize) * (self.width as usize) + (x as usize);
        let o = i * 3;
        decode_pixel(self.format, [self.rgb[o], self.rgb[o + 1], self.rgb[o + 2]])
    }

    /// Iterator over decoded elevations in row-major order.
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        let fmt = self.format;
        self.rgb
            .chunks_exact(3)
            .map(move |c| decode_pixel(fmt, [c[0], c[1], c[2]]))
    }

    /// Decode the entire view into a caller-owned buffer.
    pub fn decode_into(&self, out: &mut [f32]) {
        decode_into(self.format, self.rgb, out);
    }

    /// Decode into a freshly allocated `Vec<f32>`.
    pub fn to_vec(&self) -> Vec<f32> {
        let mut out = vec![0f32; self.len()];
        self.decode_into(&mut out);
        out
    }
}

// ---------------------------------------------------------------------------
// Generic helpers shared by per-format modules
// ---------------------------------------------------------------------------

#[inline]
fn encode_into_with(elevations: &[f32], out: &mut [u8], encode_pixel: impl Fn(f32) -> [u8; 3]) {
    assert_eq!(
        out.len(),
        elevations.len() * 3,
        "rgb buffer length mismatch: expected {}, got {}",
        elevations.len() * 3,
        out.len()
    );
    for (&e, chunk) in elevations.iter().zip(out.chunks_exact_mut(3)) {
        chunk.copy_from_slice(&encode_pixel(e));
    }
}

#[inline]
fn decode_into_with(rgb: &[u8], out: &mut [f32], decode_pixel: impl Fn([u8; 3]) -> f32) {
    assert_eq!(
        rgb.len(),
        out.len() * 3,
        "rgb buffer length mismatch: expected {}, got {}",
        out.len() * 3,
        rgb.len()
    );
    for (chunk, dst) in rgb.chunks_exact(3).zip(out.iter_mut()) {
        *dst = decode_pixel([chunk[0], chunk[1], chunk[2]]);
    }
}

#[inline]
fn encode_to_with<W: Write>(
    elevations: &[f32],
    mut writer: W,
    encode_pixel: impl Fn(f32) -> [u8; 3],
) -> io::Result<()> {
    let mut buf = [0u8; 4095]; // multiple of 3
    let mut len = 0;
    for &e in elevations {
        let px = encode_pixel(e);
        buf[len] = px[0];
        buf[len + 1] = px[1];
        buf[len + 2] = px[2];
        len += 3;
        if len + 3 > buf.len() {
            writer.write_all(&buf[..len])?;
            len = 0;
        }
    }
    if len > 0 {
        writer.write_all(&buf[..len])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-format modules
// ---------------------------------------------------------------------------

/// Mapzen / Tilezen / Stadia "Terrarium" elevation tile encoding.
///
/// Formula: `elevation = (R * 256 + G + B / 256) - 32768` (metres).
///
/// Reference: <https://github.com/tilezen/joerd/blob/master/docs/formats.md#terrarium>
pub mod terrarium {
    use std::io::{self, Write};

    /// Encode a single elevation sample (metres) into Terrarium `(R, G, B)`.
    ///
    /// Values are clamped to the representable range
    /// `[-32768, 8388.99…]` metres. `NaN` encodes as zero metres.
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

    /// Encode `elevations` into a caller-owned RGB buffer (no allocation).
    pub fn encode_into(elevations: &[f32], out: &mut [u8]) {
        super::encode_into_with(elevations, out, encode_pixel);
    }

    /// Decode RGB bytes into a caller-owned `f32` buffer (no allocation).
    pub fn decode_into(rgb: &[u8], out: &mut [f32]) {
        super::decode_into_with(rgb, out, decode_pixel);
    }

    /// Stream-encode `elevations` to a writer.
    pub fn encode_to<W: Write>(elevations: &[f32], writer: W) -> io::Result<()> {
        super::encode_to_with(elevations, writer, encode_pixel)
    }

    /// Encode `elevations` into a freshly allocated `Vec<u8>`.
    ///
    /// # Panics
    ///
    /// Panics if `elevations.len() != (width * height) as usize`.
    pub fn encode(elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
        super::encode(super::HeightmapFormat::Terrarium, elevations, width, height)
    }

    /// Decode RGB bytes into a freshly allocated `Vec<f32>`.
    ///
    /// # Panics
    ///
    /// Panics if `rgb.len() != (width * height * 3) as usize`.
    pub fn decode(rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
        super::decode(super::HeightmapFormat::Terrarium, rgb, width, height)
    }
}

/// Mapbox "Terrain-RGB" elevation tile encoding.
///
/// Formula: `elevation = -10000 + (R * 65536 + G * 256 + B) * 0.1` (metres).
///
/// Reference: <https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-rgb-v1/>
pub mod mapbox {
    use std::io::{self, Write};

    /// Encode a single elevation sample (metres) into Mapbox `(R, G, B)`.
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

    /// Decode a single Mapbox Terrain-RGB `(R, G, B)` pixel into elevation (metres).
    #[inline]
    pub fn decode_pixel(rgb: [u8; 3]) -> f32 {
        let r = rgb[0] as f32;
        let g = rgb[1] as f32;
        let b = rgb[2] as f32;
        -10000.0 + (r * 65536.0 + g * 256.0 + b) * 0.1
    }

    /// Encode `elevations` into a caller-owned RGB buffer (no allocation).
    pub fn encode_into(elevations: &[f32], out: &mut [u8]) {
        super::encode_into_with(elevations, out, encode_pixel);
    }

    /// Decode RGB bytes into a caller-owned `f32` buffer (no allocation).
    pub fn decode_into(rgb: &[u8], out: &mut [f32]) {
        super::decode_into_with(rgb, out, decode_pixel);
    }

    /// Stream-encode `elevations` to a writer.
    pub fn encode_to<W: Write>(elevations: &[f32], writer: W) -> io::Result<()> {
        super::encode_to_with(elevations, writer, encode_pixel)
    }

    /// Encode `elevations` into a freshly allocated `Vec<u8>`.
    pub fn encode(elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
        super::encode(super::HeightmapFormat::Mapbox, elevations, width, height)
    }

    /// Decode RGB bytes into a freshly allocated `Vec<f32>`.
    pub fn decode(rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
        super::decode(super::HeightmapFormat::Mapbox, rgb, width, height)
    }
}

/// Geospatial Information Authority of Japan (GSI / 国土地理院) DEM tile
/// encoding.
///
/// Each pixel packs a signed 24-bit integer of 0.01 m units.
/// The no-data sentinel `(128, 0, 0)` decodes to `NaN`.
///
/// Reference: <https://maps.gsi.go.jp/development/demtile.html>
pub mod gsi {
    use std::io::{self, Write};

    /// No-data sentinel `(R=128, G=0, B=0)`, decoded as `NaN`.
    pub const SENTINEL_RGB: [u8; 3] = [0x80, 0x00, 0x00];
    const SIGN_BIT: u32 = 1 << 23;
    const RANGE: i64 = 1 << 24;

    /// Encode a single elevation sample (metres) into GSI `(R, G, B)`.
    ///
    /// `NaN` encodes as the no-data sentinel `(128, 0, 0)`.
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

    /// Encode `elevations` into a caller-owned RGB buffer (no allocation).
    pub fn encode_into(elevations: &[f32], out: &mut [u8]) {
        super::encode_into_with(elevations, out, encode_pixel);
    }

    /// Decode RGB bytes into a caller-owned `f32` buffer (no allocation).
    pub fn decode_into(rgb: &[u8], out: &mut [f32]) {
        super::decode_into_with(rgb, out, decode_pixel);
    }

    /// Stream-encode `elevations` to a writer.
    pub fn encode_to<W: Write>(elevations: &[f32], writer: W) -> io::Result<()> {
        super::encode_to_with(elevations, writer, encode_pixel)
    }

    /// Encode `elevations` into a freshly allocated `Vec<u8>`.
    pub fn encode(elevations: &[f32], width: u32, height: u32) -> Vec<u8> {
        super::encode(super::HeightmapFormat::Gsi, elevations, width, height)
    }

    /// Decode RGB bytes into a freshly allocated `Vec<f32>`.
    pub fn decode(rgb: &[u8], width: u32, height: u32) -> Vec<f32> {
        super::decode(super::HeightmapFormat::Gsi, rgb, width, height)
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
            let dispatched = encode(fmt, &elevations, elevations.len() as u32, 1);
            let direct = match fmt {
                HeightmapFormat::Terrarium => terrarium::encode(&elevations, 5, 1),
                HeightmapFormat::Mapbox => mapbox::encode(&elevations, 5, 1),
                HeightmapFormat::Gsi => gsi::encode(&elevations, 5, 1),
            };
            assert_eq!(dispatched, direct, "encode mismatch for {fmt}");

            for &e in &elevations {
                let px = encode_pixel(fmt, e);
                let back = decode_pixel(fmt, px);
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

    #[test]
    fn encode_into_matches_encode_vec() {
        let elevations: Vec<f32> = (0..16).map(|i| i as f32 * 10.0).collect();
        for fmt in HeightmapFormat::ALL {
            let expected = encode(fmt, &elevations, 16, 1);
            let mut buf = vec![0u8; elevations.len() * 3];
            encode_into(fmt, &elevations, &mut buf);
            assert_eq!(expected, buf, "encode_into mismatch for {fmt}");
        }
    }

    #[test]
    fn encode_to_writer_matches_encode_vec() {
        let elevations: Vec<f32> = (0..2000).map(|i| i as f32 * 0.5).collect();
        for fmt in HeightmapFormat::ALL {
            let expected = encode(fmt, &elevations, elevations.len() as u32, 1);
            let mut buf = Vec::new();
            encode_to(fmt, &elevations, &mut buf).unwrap();
            assert_eq!(expected, buf, "encode_to mismatch for {fmt}");
        }
    }

    #[test]
    fn heightmap_view_iter_matches_decode() {
        let elevations: Vec<f32> = vec![0.0, 100.0, 200.0, -50.0];
        for fmt in HeightmapFormat::ALL {
            let rgb = encode(fmt, &elevations, 4, 1);
            let view = HeightmapView::new(fmt, &rgb, 4, 1);
            let decoded: Vec<f32> = view.iter().collect();
            let direct = decode(fmt, &rgb, 4, 1);
            assert_eq!(decoded, direct);
            assert_eq!(view.get(0, 0), direct[0]);
            assert_eq!(view.get(3, 0), direct[3]);
            assert_eq!(view.to_vec(), direct);
        }
    }
}
