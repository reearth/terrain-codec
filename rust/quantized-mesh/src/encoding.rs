//! Encoding functions and main encoder for quantized-mesh format.
//!
//! The streaming writer uses inline zigzag-delta and high-water-mark
//! encoding with a small chunked buffer, so no intermediate `Vec<u16>` /
//! `Vec<u32>` is allocated for the encoded streams.

use std::io::{self, Write};

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::{
    EdgeIndices, ExtensionId, QuantizedMeshHeader, QuantizedVertices, TileMetadata, WaterMask,
};

/// Encode a value using zig-zag encoding.
///
/// Maps signed integers to unsigned integers so that small magnitude values
/// (positive or negative) have small encoded values.
///
/// ```text
/// 0 -> 0, -1 -> 1, 1 -> 2, -2 -> 3, 2 -> 4, ...
/// ```
#[inline]
pub fn zigzag_encode(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

/// Decode a zig-zag encoded value.
#[inline]
pub fn zigzag_decode(value: u32) -> i32 {
    ((value >> 1) as i32) ^ (-((value & 1) as i32))
}

/// Oct-encode a unit normal vector to 2 bytes.
///
/// Uses octahedron encoding for efficient normal compression.
pub fn oct_encode_normal(normal: [f32; 3]) -> [u8; 2] {
    let [mut x, mut y, z] = normal;

    // Project to octahedron
    let inv_l1 = 1.0 / (x.abs() + y.abs() + z.abs());
    x *= inv_l1;
    y *= inv_l1;

    // Unfold lower hemisphere
    if z < 0.0 {
        let ox = x;
        x = (1.0 - y.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if y >= 0.0 { 1.0 } else { -1.0 };
    }

    // Map from [-1, 1] to [0, 255]
    let encode = |v: f32| -> u8 { ((v * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8 };

    [encode(x), encode(y)]
}

/// Oct-encode a batch of unit normals, writing 2 bytes per normal into `out`.
///
/// Equivalent to calling [`oct_encode_normal`] on each normal and
/// concatenating the results — the output is identical on every target.
///
/// On `wasm32` the batch is encoded with explicit simd128 (four normals per
/// `f32x4` step), which benchmarked ~1.66× faster than the scalar loop under
/// a WebAssembly runtime. On every other target it is the plain scalar loop:
/// native back-ends autovectorise that form better than the hand-written
/// SIMD, which regressed in native benchmarks, so the SIMD path is gated to
/// wasm only.
///
/// # Panics
///
/// Panics if `out.len() != normals.len() * 2`.
pub fn oct_encode_normals_into(normals: &[[f32; 3]], out: &mut [u8]) {
    assert_eq!(
        out.len(),
        normals.len() * 2,
        "normal output length mismatch: expected {}, got {}",
        normals.len() * 2,
        out.len()
    );

    #[cfg(not(target_arch = "wasm32"))]
    for (n, o) in normals.iter().zip(out.chunks_exact_mut(2)) {
        o.copy_from_slice(&oct_encode_normal(*n));
    }

    #[cfg(target_arch = "wasm32")]
    oct_encode_normals_simd128(normals, out);
}

/// simd128 batch oct-encode. Bit-for-bit identical to [`oct_encode_normal`]:
/// every step is IEEE add/sub/mul/div/abs/max/min plus lane selects, and the
/// `as u8` truncation matches the scalar `clamp(..) as u8`.
#[cfg(target_arch = "wasm32")]
fn oct_encode_normals_simd128(normals: &[[f32; 3]], out: &mut [u8]) {
    use wide::{CmpGe, CmpLt, f32x4};

    let one = f32x4::splat(1.0);
    let neg_one = f32x4::splat(-1.0);
    let zero = f32x4::splat(0.0);
    let half = f32x4::splat(0.5);
    let n255 = f32x4::splat(255.0);

    let mut nchunks = normals.chunks_exact(4);
    let mut ochunks = out.chunks_exact_mut(8);
    for (nc, oc) in nchunks.by_ref().zip(ochunks.by_ref()) {
        let x = f32x4::new([nc[0][0], nc[1][0], nc[2][0], nc[3][0]]);
        let y = f32x4::new([nc[0][1], nc[1][1], nc[2][1], nc[3][1]]);
        let z = f32x4::new([nc[0][2], nc[1][2], nc[2][2], nc[3][2]]);

        let inv_l1 = one / (x.abs() + y.abs() + z.abs());
        let px = x * inv_l1;
        let py = y * inv_l1;

        // Unfold lower hemisphere where z < 0 (branchless via select).
        let neg = z.cmp_lt(zero);
        let sign_x = px.cmp_ge(zero).blend(one, neg_one);
        let sign_y = py.cmp_ge(zero).blend(one, neg_one);
        let fx = (one - py.abs()) * sign_x;
        let fy = (one - px.abs()) * sign_y;
        let px = neg.blend(fx, px);
        let py = neg.blend(fy, py);

        // Map [-1,1] → [0,255], clamp, truncate to u8.
        let bx = ((px * half + half) * n255).max(zero).min(n255).to_array();
        let by = ((py * half + half) * n255).max(zero).min(n255).to_array();
        for k in 0..4 {
            oc[k * 2] = bx[k] as u8;
            oc[k * 2 + 1] = by[k] as u8;
        }
    }

    // Scalar remainder for the ≤3 trailing normals.
    for (n, o) in nchunks
        .remainder()
        .iter()
        .zip(ochunks.into_remainder().chunks_exact_mut(2))
    {
        o.copy_from_slice(&oct_encode_normal(*n));
    }
}

/// Options for encoding quantized mesh.
#[derive(Debug, Clone, Default)]
pub struct EncodeOptions {
    /// Include oct-encoded vertex normals
    pub include_normals: bool,
    /// Vertex normals (required if include_normals is true)
    pub normals: Option<Vec<[f32; 3]>>,
    /// Include water mask
    pub include_water_mask: bool,
    /// Water mask data
    pub water_mask: Option<WaterMask>,
    /// Include metadata extension with tile availability
    pub include_metadata: bool,
    /// Metadata for tile availability
    pub metadata: Option<TileMetadata>,
    /// Gzip compression level (0-9, default 6)
    pub compression_level: u32,
}

/// Quantized mesh encoder.
///
/// Encodes terrain mesh data into the quantized-mesh-1.0 format.
pub struct QuantizedMeshEncoder {
    header: QuantizedMeshHeader,
    vertices: QuantizedVertices,
    indices: Vec<u32>,
    edge_indices: EdgeIndices,
}

impl QuantizedMeshEncoder {
    /// Create a new encoder with mesh data.
    pub fn new(
        header: QuantizedMeshHeader,
        vertices: QuantizedVertices,
        indices: Vec<u32>,
        edge_indices: EdgeIndices,
    ) -> Self {
        Self {
            header,
            vertices,
            indices,
            edge_indices,
        }
    }

    /// Encode to quantized-mesh format without compression.
    pub fn encode(&self) -> Vec<u8> {
        self.encode_with_options(&EncodeOptions::default())
    }

    /// Encode with options (extensions, compression).
    pub fn encode_with_options(&self, options: &EncodeOptions) -> Vec<u8> {
        let mut output = Vec::new();
        self.encode_to_with_options(&mut output, options)
            .expect("Failed to encode to Vec");
        output
    }

    /// Encode to a writer without compression.
    pub fn encode_to<W: Write>(&self, writer: W) -> io::Result<()> {
        self.encode_to_with_options(writer, &EncodeOptions::default())
    }

    /// Encode to a writer with options (extensions, compression).
    pub fn encode_to_with_options<W: Write>(
        &self,
        writer: W,
        options: &EncodeOptions,
    ) -> io::Result<()> {
        if options.compression_level == 0 {
            self.encode_uncompressed_to(writer, options)
        } else {
            let mut encoder = GzEncoder::new(writer, Compression::new(options.compression_level));
            self.encode_uncompressed_to(&mut encoder, options)?;
            encoder.finish()?;
            Ok(())
        }
    }

    /// Encode without compression to a writer, streaming each section
    /// directly without intermediate Vec allocations.
    fn encode_uncompressed_to<W: Write>(
        &self,
        mut writer: W,
        options: &EncodeOptions,
    ) -> io::Result<()> {
        let vertex_count = self.vertices.len();
        let use_32bit = vertex_count > 65535;

        // Header (88 bytes).
        writer.write_all(&self.header.to_bytes())?;
        // Vertex count.
        writer.write_all(&(vertex_count as u32).to_le_bytes())?;

        // Vertex u/v/height streams (zigzag-delta).
        write_zigzag_delta_to(&mut writer, &self.vertices.u)?;
        write_zigzag_delta_to(&mut writer, &self.vertices.v)?;
        write_zigzag_delta_to(&mut writer, &self.vertices.height)?;

        // Pad to index alignment. After header+count+vertices the offset is:
        let current_pos = 88 + 4 + vertex_count * 6;
        let align = if use_32bit { 4 } else { 2 };
        let padding = (align - (current_pos % align)) % align;
        if padding > 0 {
            let zeros = [0u8; 4];
            writer.write_all(&zeros[..padding])?;
        }

        // Triangle count + high-water-mark indices.
        let triangle_count = self.indices.len() / 3;
        writer.write_all(&(triangle_count as u32).to_le_bytes())?;
        write_high_water_mark_to(&mut writer, &self.indices, use_32bit)?;

        // Edge index streams.
        for edge in [
            &self.edge_indices.west,
            &self.edge_indices.south,
            &self.edge_indices.east,
            &self.edge_indices.north,
        ] {
            writer.write_all(&(edge.len() as u32).to_le_bytes())?;
            write_indices_to(&mut writer, edge, use_32bit)?;
        }

        // Extensions.
        if options.include_normals
            && let Some(normals) = &options.normals
        {
            write_normals_extension_to(&mut writer, normals)?;
        }
        if options.include_water_mask {
            let water_mask = options.water_mask.as_ref().cloned().unwrap_or_default();
            write_water_mask_extension_to(&mut writer, &water_mask)?;
        }
        if options.include_metadata
            && let Some(metadata) = &options.metadata
        {
            write_metadata_extension_to(&mut writer, metadata)?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Streaming write helpers
// ---------------------------------------------------------------------------

/// Buffer size for streaming writes. 4 KiB = 2048 u16s or 1024 u32s — large
/// enough to amortise per-call `write_all` overhead.
const WRITE_BUF: usize = 4096;

fn write_zigzag_delta_to<W: Write>(writer: &mut W, values: &[u16]) -> io::Result<()> {
    let mut buf = [0u8; WRITE_BUF];
    let mut len = 0;
    let mut prev = 0i32;
    for &value in values {
        let current = value as i32;
        let delta = current - prev;
        let bytes = (zigzag_encode(delta) as u16).to_le_bytes();
        buf[len] = bytes[0];
        buf[len + 1] = bytes[1];
        len += 2;
        prev = current;
        if len + 2 > WRITE_BUF {
            writer.write_all(&buf[..len])?;
            len = 0;
        }
    }
    if len > 0 {
        writer.write_all(&buf[..len])?;
    }
    Ok(())
}

fn write_high_water_mark_to<W: Write>(
    writer: &mut W,
    indices: &[u32],
    use_32bit: bool,
) -> io::Result<()> {
    let mut buf = [0u8; WRITE_BUF];
    let mut len = 0;
    let mut highest = 0u32;
    let stride = if use_32bit { 4 } else { 2 };
    for &index in indices {
        let code = if index == highest {
            highest += 1;
            0
        } else {
            highest - index
        };
        if use_32bit {
            buf[len..len + 4].copy_from_slice(&code.to_le_bytes());
        } else {
            buf[len..len + 2].copy_from_slice(&(code as u16).to_le_bytes());
        }
        len += stride;
        if len + stride > WRITE_BUF {
            writer.write_all(&buf[..len])?;
            len = 0;
        }
    }
    if len > 0 {
        writer.write_all(&buf[..len])?;
    }
    Ok(())
}

fn write_indices_to<W: Write>(writer: &mut W, indices: &[u32], use_32bit: bool) -> io::Result<()> {
    let mut buf = [0u8; WRITE_BUF];
    let mut len = 0;
    let stride = if use_32bit { 4 } else { 2 };
    for &idx in indices {
        if use_32bit {
            buf[len..len + 4].copy_from_slice(&idx.to_le_bytes());
        } else {
            buf[len..len + 2].copy_from_slice(&(idx as u16).to_le_bytes());
        }
        len += stride;
        if len + stride > WRITE_BUF {
            writer.write_all(&buf[..len])?;
            len = 0;
        }
    }
    if len > 0 {
        writer.write_all(&buf[..len])?;
    }
    Ok(())
}

fn write_normals_extension_to<W: Write>(writer: &mut W, normals: &[[f32; 3]]) -> io::Result<()> {
    writer.write_all(&[ExtensionId::OctEncodedVertexNormals as u8])?;
    writer.write_all(&((normals.len() * 2) as u32).to_le_bytes())?;
    // Encode up to WRITE_BUF/2 normals per flush via the batch encoder
    // (simd128 on wasm, scalar elsewhere). WRITE_BUF is even.
    let mut buf = [0u8; WRITE_BUF];
    for chunk in normals.chunks(WRITE_BUF / 2) {
        let n = chunk.len() * 2;
        oct_encode_normals_into(chunk, &mut buf[..n]);
        writer.write_all(&buf[..n])?;
    }
    Ok(())
}

fn write_water_mask_extension_to<W: Write>(
    writer: &mut W,
    water_mask: &WaterMask,
) -> io::Result<()> {
    writer.write_all(&[ExtensionId::WaterMask as u8])?;
    match water_mask {
        WaterMask::Uniform(value) => {
            writer.write_all(&1u32.to_le_bytes())?;
            writer.write_all(&[*value])?;
        }
        WaterMask::Grid(grid) => {
            writer.write_all(&(256 * 256u32).to_le_bytes())?;
            writer.write_all(grid.as_ref())?;
        }
    }
    Ok(())
}

fn write_metadata_extension_to<W: Write>(
    writer: &mut W,
    metadata: &TileMetadata,
) -> io::Result<()> {
    let json = serde_json::to_string(metadata)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let json_bytes = json.as_bytes();
    writer.write_all(&[ExtensionId::Metadata as u8])?;
    let extension_length = 4 + json_bytes.len() as u32;
    writer.write_all(&extension_length.to_le_bytes())?;
    writer.write_all(&(json_bytes.len() as u32).to_le_bytes())?;
    writer.write_all(json_bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spread of unit normals over both hemispheres; `n` need not be a
    /// multiple of 4 (exercises the SIMD remainder tail).
    fn spread_normals(n: usize) -> Vec<[f32; 3]> {
        (0..n)
            .map(|i| {
                let a = (i as f32) * 0.61803398875;
                let b = (i as f32) * 0.15915494309;
                let x = a.sin() * 0.9;
                let y = b.cos() * 0.9;
                let z = 1.0 - x.abs() - y.abs();
                let len = (x * x + y * y + z * z).sqrt().max(1e-6);
                [x / len, y / len, z / len]
            })
            .collect()
    }

    #[test]
    fn oct_encode_normals_into_matches_per_normal() {
        let normals = spread_normals(1003); // 1003 % 4 == 3
        let mut batch = vec![0u8; normals.len() * 2];
        oct_encode_normals_into(&normals, &mut batch);
        for (i, n) in normals.iter().enumerate() {
            assert_eq!(
                [batch[i * 2], batch[i * 2 + 1]],
                oct_encode_normal(*n),
                "batch encode mismatch at normal {i}"
            );
        }
    }

    #[test]
    fn test_zigzag_encode() {
        assert_eq!(zigzag_encode(0), 0);
        assert_eq!(zigzag_encode(-1), 1);
        assert_eq!(zigzag_encode(1), 2);
        assert_eq!(zigzag_encode(-2), 3);
        assert_eq!(zigzag_encode(2), 4);
    }

    #[test]
    fn test_zigzag_roundtrip() {
        for i in -1000..1000 {
            assert_eq!(zigzag_decode(zigzag_encode(i)), i);
        }
    }

    #[test]
    fn test_oct_encode_normal() {
        let up = [0.0f32, 0.0, 1.0];
        let encoded = oct_encode_normal(up);
        assert!((encoded[0] as i32 - 127).abs() < 2);
        assert!((encoded[1] as i32 - 127).abs() < 2);

        let down = [0.0f32, 0.0, -1.0];
        let encoded = oct_encode_normal(down);
        assert!(encoded[0] == 0 || encoded[0] == 255);
    }

    #[test]
    fn test_encoder_basic() {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);
        let data = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        assert!(data.len() > 88);
        let parsed_header = QuantizedMeshHeader::from_bytes(&data).unwrap();
        assert_eq!(parsed_header.min_height, 0.0);
    }

    #[test]
    fn test_encoder_with_compression() {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);

        let compressed = encoder.encode_with_options(&EncodeOptions {
            compression_level: 6,
            ..Default::default()
        });

        assert_eq!(&compressed[0..2], &[0x1f, 0x8b]);
    }

    #[test]
    fn test_encoder_with_extensions() {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);

        let normals = vec![[0.0, 0.0, 1.0]; 4];

        let data = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            include_normals: true,
            normals: Some(normals),
            include_water_mask: true,
            water_mask: Some(WaterMask::Uniform(0)),
            ..Default::default()
        });

        let without_ext = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        assert!(data.len() > without_ext.len());
    }

    #[test]
    fn test_encode_to_writer_matches_encode_with_options() {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);

        let data_vec = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        let mut data_writer = Vec::new();
        encoder
            .encode_to_with_options(
                &mut data_writer,
                &EncodeOptions {
                    compression_level: 0,
                    ..Default::default()
                },
            )
            .expect("Failed to encode to writer");

        assert_eq!(data_vec, data_writer);
    }

    #[test]
    fn test_encode_to_writer_compressed() {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);

        let mut data_writer = Vec::new();
        encoder
            .encode_to_with_options(
                &mut data_writer,
                &EncodeOptions {
                    compression_level: 6,
                    ..Default::default()
                },
            )
            .expect("Failed to encode to writer");

        assert_eq!(&data_writer[0..2], &[0x1f, 0x8b]);
    }
}
