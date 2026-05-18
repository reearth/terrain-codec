//! Cesium **heightmap-1.0** terrain tile format.
//!
//! The legacy Cesium terrain format. Each tile is a regular grid of 16-bit
//! heights (default 65 × 65, north → south, west → east) plus a 1-byte
//! child-availability mask. Optional extensions follow: a water mask
//! (1 byte uniform or 256 × 256 bytes), then oct-encoded per-vertex
//! normals (2 bytes per vertex).
//!
//! Specification:
//! <https://github.com/CesiumGS/cesium/wiki/heightmap-1.0>
//!
//! # Layout
//!
//! ```text
//! +------------------------------------+
//! | u16 LE heights (size×size × 2 B)   |
//! +------------------------------------+
//! | u8 child mask                      |
//! +------------------------------------+
//! | water mask (optional, 1 or 256² B) |
//! +------------------------------------+
//! | oct normals (optional, 2 × N B)    |
//! +------------------------------------+
//! ```
//!
//! Encoded files are typically **gzip-compressed**; this module returns
//! raw uncompressed bytes — compress them yourself with `flate2` or
//! similar.
//!
//! # Height encoding
//!
//! `value = (elevation + 1000) * 5`, i.e. range −1000 m … +12107 m at
//! 0.2 m resolution. Use [`elevation_to_u16`] / [`u16_to_elevation`] for
//! single-sample conversion.

/// Default heightmap-1.0 tile side length (Cesium streams 65 × 65 tiles).
pub const TILE_SIZE: u32 = 65;

/// Minimum elevation representable in metres.
pub const MIN_ELEVATION: f64 = -1000.0;

/// Maximum elevation representable in metres.
pub const MAX_ELEVATION: f64 = 12107.0;

/// Scale factor: `value = (elevation - MIN_ELEVATION) * SCALE`.
pub const SCALE: f64 = 5.0;

/// Encode a single elevation sample (metres) into the heightmap-1.0 u16.
///
/// Values are clamped to `[MIN_ELEVATION, MAX_ELEVATION]`.
#[inline]
pub fn elevation_to_u16(elevation: f64) -> u16 {
    let clamped = elevation.clamp(MIN_ELEVATION, MAX_ELEVATION);
    ((clamped - MIN_ELEVATION) * SCALE).round() as u16
}

/// Decode a heightmap-1.0 u16 back to elevation (metres).
#[inline]
pub fn u16_to_elevation(value: u16) -> f64 {
    value as f64 / SCALE + MIN_ELEVATION
}

/// Encode a row-major `width × height` elevation grid into a caller-owned
/// little-endian u16 byte buffer. `out.len()` must equal `width * height * 2`.
///
/// Rows are expected north → south, columns west → east (Cesium's
/// convention).
///
/// # Panics
///
/// Panics if `elevations.len() < (width * height) as usize` or if `out`
/// is the wrong size.
pub fn encode_heights_into(elevations: &[f64], width: u32, height: u32, out: &mut [u8]) {
    let expected = (width as usize) * (height as usize);
    assert!(
        elevations.len() >= expected,
        "expected at least {expected} elevations, got {}",
        elevations.len()
    );
    assert_eq!(
        out.len(),
        expected * 2,
        "out buffer length mismatch: expected {}, got {}",
        expected * 2,
        out.len()
    );
    for (&e, chunk) in elevations[..expected].iter().zip(out.chunks_exact_mut(2)) {
        chunk.copy_from_slice(&elevation_to_u16(e).to_le_bytes());
    }
}

/// Stream-encode a `width × height` elevation grid as little-endian u16
/// bytes to a writer. Uses a 4 KiB stack buffer.
pub fn encode_heights_to<W: std::io::Write>(
    elevations: &[f64],
    width: u32,
    height: u32,
    mut writer: W,
) -> std::io::Result<()> {
    let expected = (width as usize) * (height as usize);
    assert!(
        elevations.len() >= expected,
        "expected at least {expected} elevations, got {}",
        elevations.len()
    );
    let mut buf = [0u8; 4096];
    let mut len = 0;
    for &e in &elevations[..expected] {
        let bytes = elevation_to_u16(e).to_le_bytes();
        buf[len] = bytes[0];
        buf[len + 1] = bytes[1];
        len += 2;
        if len + 2 > buf.len() {
            writer.write_all(&buf[..len])?;
            len = 0;
        }
    }
    if len > 0 {
        writer.write_all(&buf[..len])?;
    }
    Ok(())
}

/// Encode a row-major elevation grid into a freshly allocated `Vec<u8>`.
///
/// # Panics
///
/// Panics if `elevations.len() < (width * height) as usize`.
pub fn encode_heights(elevations: &[f64], width: u32, height: u32) -> Vec<u8> {
    let expected = (width as usize) * (height as usize);
    let mut out = vec![0u8; expected * 2];
    encode_heights_into(elevations, width, height, &mut out);
    out
}

/// Lazily iterate decoded elevations from little-endian u16 bytes.
/// Trailing odd bytes are ignored.
pub fn iter_heights(data: &[u8]) -> impl Iterator<Item = f64> + '_ {
    data.chunks_exact(2)
        .map(|c| u16_to_elevation(u16::from_le_bytes([c[0], c[1]])))
}

/// Decode little-endian u16 bytes back to a flat elevation grid.
///
/// Truncates any trailing odd byte.
pub fn decode_heights(data: &[u8]) -> Vec<f64> {
    iter_heights(data).collect()
}

/// Child-availability mask: one bit per quadrant indicating whether a
/// child tile exists at the next zoom level.
///
/// Bit layout (matches the heightmap-1.0 spec):
///
/// | Bit | Value | Quadrant  |
/// |-----|-------|-----------|
/// | 0   | 1     | Southwest |
/// | 1   | 2     | Southeast |
/// | 2   | 4     | Northwest |
/// | 3   | 8     | Northeast |
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChildTileMask {
    /// Southwest child present.
    pub southwest: bool,
    /// Southeast child present.
    pub southeast: bool,
    /// Northwest child present.
    pub northwest: bool,
    /// Northeast child present.
    pub northeast: bool,
}

impl ChildTileMask {
    /// Mask with every quadrant present.
    pub const fn all() -> Self {
        Self {
            southwest: true,
            southeast: true,
            northwest: true,
            northeast: true,
        }
    }

    /// Mask with no quadrants present.
    pub const fn none() -> Self {
        Self {
            southwest: false,
            southeast: false,
            northwest: false,
            northeast: false,
        }
    }

    /// Pack into a single byte.
    pub const fn to_byte(self) -> u8 {
        let mut mask = 0u8;
        if self.southwest {
            mask |= 1;
        }
        if self.southeast {
            mask |= 2;
        }
        if self.northwest {
            mask |= 4;
        }
        if self.northeast {
            mask |= 8;
        }
        mask
    }

    /// Unpack from a byte.
    pub const fn from_byte(byte: u8) -> Self {
        Self {
            southwest: byte & 1 != 0,
            southeast: byte & 2 != 0,
            northwest: byte & 4 != 0,
            northeast: byte & 8 != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_roundtrip_within_range() {
        for &e in &[-1000.0_f64, 0.0, 100.0, 8848.0, 12107.0] {
            let back = u16_to_elevation(elevation_to_u16(e));
            assert!((e - back).abs() < 0.2, "{e} → {back}");
        }
    }

    #[test]
    fn elevation_clamps_out_of_range() {
        let low = u16_to_elevation(elevation_to_u16(-2000.0));
        assert!((low - MIN_ELEVATION).abs() < 0.2);
        let high = u16_to_elevation(elevation_to_u16(20000.0));
        assert!((high - MAX_ELEVATION).abs() < 0.2);
    }

    #[test]
    fn encode_decode_bulk() {
        let elevations: Vec<f64> = vec![0.0, 100.0, -100.0, 1000.0];
        let bytes = encode_heights(&elevations, 2, 2);
        assert_eq!(bytes.len(), 8); // 4 × u16
        let back = decode_heights(&bytes);
        for (a, b) in elevations.iter().zip(&back) {
            assert!((a - b).abs() < 0.2);
        }
    }

    #[test]
    fn child_mask_bits_match_spec() {
        assert_eq!(ChildTileMask::all().to_byte(), 0b1111);
        assert_eq!(ChildTileMask::none().to_byte(), 0);
        let only_se = ChildTileMask {
            southeast: true,
            ..Default::default()
        };
        assert_eq!(only_se.to_byte(), 0b0010);
        assert_eq!(
            ChildTileMask::from_byte(0b1010),
            ChildTileMask {
                southwest: false,
                southeast: true,
                northwest: false,
                northeast: true,
            }
        );
    }
}
