//! Encoding functions and main encoder for quantized-mesh format.

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

/// Encode vertex coordinates using zig-zag delta encoding.
///
/// Each value is encoded as the zig-zag encoded difference from the previous value.
pub fn encode_zigzag_delta(values: &[u16]) -> Vec<u16> {
    let mut result = Vec::with_capacity(values.len());
    let mut prev = 0i32;

    for &value in values {
        let current = value as i32;
        let delta = current - prev;
        result.push(zigzag_encode(delta) as u16);
        prev = current;
    }

    result
}

/// Decode zig-zag delta encoded values.
pub fn decode_zigzag_delta(encoded: &[u16]) -> Vec<u16> {
    let mut result = Vec::with_capacity(encoded.len());
    let mut value = 0i32;

    for &enc in encoded {
        let delta = zigzag_decode(enc as u32);
        value += delta;
        result.push(value as u16);
    }

    result
}

/// Encode indices using high-water mark encoding.
///
/// This encoding is efficient when indices reference recently added vertices.
pub fn encode_high_water_mark(indices: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(indices.len());
    let mut highest = 0u32;

    for &index in indices {
        let code = if index == highest {
            highest += 1;
            0
        } else {
            highest - index
        };
        result.push(code);
    }

    result
}

/// Decode high-water mark encoded indices.
pub fn decode_high_water_mark(encoded: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(encoded.len());
    let mut highest = 0u32;

    for &code in encoded {
        let index = highest - code;
        if code == 0 {
            highest += 1;
        }
        result.push(index);
    }

    result
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
        mut writer: W,
        options: &EncodeOptions,
    ) -> io::Result<()> {
        if options.compression_level == 0 {
            self.encode_uncompressed_to(&mut writer, options)
        } else {
            // Gzip compress
            let mut encoder = GzEncoder::new(writer, Compression::new(options.compression_level));
            self.encode_uncompressed_to(&mut encoder, options)?;
            encoder.finish()?;
            Ok(())
        }
    }

    /// Encode without compression to a writer.
    fn encode_uncompressed_to<W: Write>(
        &self,
        writer: &mut W,
        options: &EncodeOptions,
    ) -> io::Result<()> {
        let vertex_count = self.vertices.len();
        let use_32bit = vertex_count > 65535;

        // Write header (88 bytes)
        writer.write_all(&self.header.to_bytes())?;

        // Write vertex count
        writer.write_all(&(vertex_count as u32).to_le_bytes())?;

        // Write encoded vertex data
        let encoded_u = encode_zigzag_delta(&self.vertices.u);
        let encoded_v = encode_zigzag_delta(&self.vertices.v);
        let encoded_height = encode_zigzag_delta(&self.vertices.height);

        for &u in &encoded_u {
            writer.write_all(&u.to_le_bytes())?;
        }
        for &v in &encoded_v {
            writer.write_all(&v.to_le_bytes())?;
        }
        for &h in &encoded_height {
            writer.write_all(&h.to_le_bytes())?;
        }

        // Calculate current position for padding
        // header (88) + vertex_count (4) + vertices (vertex_count * 6)
        let current_pos = 88 + 4 + vertex_count * 6;

        // Padding for index alignment
        if use_32bit {
            // Align to 4 bytes
            let padding = (4 - (current_pos % 4)) % 4;
            for _ in 0..padding {
                writer.write_all(&[0])?;
            }
        } else {
            // Align to 2 bytes
            let padding = (2 - (current_pos % 2)) % 2;
            for _ in 0..padding {
                writer.write_all(&[0])?;
            }
        }

        // Write triangle count
        let triangle_count = self.indices.len() / 3;
        writer.write_all(&(triangle_count as u32).to_le_bytes())?;

        // Write encoded indices
        let encoded_indices = encode_high_water_mark(&self.indices);
        if use_32bit {
            for &idx in &encoded_indices {
                writer.write_all(&idx.to_le_bytes())?;
            }
        } else {
            for &idx in &encoded_indices {
                writer.write_all(&(idx as u16).to_le_bytes())?;
            }
        }

        // Write edge indices
        self.write_edge_indices_to(writer, &self.edge_indices.west, use_32bit)?;
        self.write_edge_indices_to(writer, &self.edge_indices.south, use_32bit)?;
        self.write_edge_indices_to(writer, &self.edge_indices.east, use_32bit)?;
        self.write_edge_indices_to(writer, &self.edge_indices.north, use_32bit)?;

        // Write extensions
        if options.include_normals
            && let Some(normals) = &options.normals
        {
            self.write_normals_extension_to(writer, normals)?;
        }

        if options.include_water_mask {
            let water_mask = options.water_mask.as_ref().cloned().unwrap_or_default();
            self.write_water_mask_extension_to(writer, &water_mask)?;
        }

        if options.include_metadata
            && let Some(metadata) = &options.metadata
        {
            self.write_metadata_extension_to(writer, metadata)?;
        }

        Ok(())
    }

    fn write_edge_indices_to<W: Write>(
        &self,
        writer: &mut W,
        indices: &[u32],
        use_32bit: bool,
    ) -> io::Result<()> {
        writer.write_all(&(indices.len() as u32).to_le_bytes())?;
        if use_32bit {
            for &idx in indices {
                writer.write_all(&idx.to_le_bytes())?;
            }
        } else {
            for &idx in indices {
                writer.write_all(&(idx as u16).to_le_bytes())?;
            }
        }
        Ok(())
    }

    fn write_normals_extension_to<W: Write>(
        &self,
        writer: &mut W,
        normals: &[[f32; 3]],
    ) -> io::Result<()> {
        // Extension header
        writer.write_all(&[ExtensionId::OctEncodedVertexNormals as u8])?;
        let length = (normals.len() * 2) as u32;
        writer.write_all(&length.to_le_bytes())?;

        // Oct-encoded normals
        for &normal in normals {
            let encoded = oct_encode_normal(normal);
            writer.write_all(&encoded)?;
        }
        Ok(())
    }

    fn write_water_mask_extension_to<W: Write>(
        &self,
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
        &self,
        writer: &mut W,
        metadata: &TileMetadata,
    ) -> io::Result<()> {
        // Serialize metadata to JSON
        let json = serde_json::to_string(metadata)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let json_bytes = json.as_bytes();

        // Extension header
        writer.write_all(&[ExtensionId::Metadata as u8])?;

        // Extension length (4 bytes for json length + actual json)
        let extension_length = 4 + json_bytes.len() as u32;
        writer.write_all(&extension_length.to_le_bytes())?;

        // JSON length
        writer.write_all(&(json_bytes.len() as u32).to_le_bytes())?;

        // JSON data
        writer.write_all(json_bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_zigzag_delta_roundtrip() {
        let values: Vec<u16> = vec![0, 100, 50, 200, 150, 32767, 0];
        let encoded = encode_zigzag_delta(&values);
        let decoded = decode_zigzag_delta(&encoded);
        assert_eq!(values, decoded);
    }

    #[test]
    fn test_high_water_mark_simple() {
        // Sequential indices
        let indices = vec![0, 1, 2, 3, 4, 5];
        let encoded = encode_high_water_mark(&indices);
        // All zeros because each index equals highest
        assert_eq!(encoded, vec![0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_high_water_mark_roundtrip() {
        let indices = vec![0, 1, 2, 1, 3, 2, 0, 4, 3];
        let encoded = encode_high_water_mark(&indices);
        let decoded = decode_high_water_mark(&encoded);
        assert_eq!(indices, decoded);
    }

    #[test]
    fn test_oct_encode_normal() {
        // Up vector
        let up = [0.0f32, 0.0, 1.0];
        let encoded = oct_encode_normal(up);
        // Should be near center (127, 127)
        assert!((encoded[0] as i32 - 127).abs() < 2);
        assert!((encoded[1] as i32 - 127).abs() < 2);

        // Down vector
        let down = [0.0f32, 0.0, -1.0];
        let encoded = oct_encode_normal(down);
        // Should be at corners
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

        // Should have at least header + some data
        assert!(data.len() > 88);

        // First 88 bytes should be header
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

        let _uncompressed = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        let compressed = encoder.encode_with_options(&EncodeOptions {
            compression_level: 6,
            ..Default::default()
        });

        // Compressed should typically be smaller (or at least start with gzip magic)
        assert_eq!(&compressed[0..2], &[0x1f, 0x8b]); // gzip magic number
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

        // Should be larger with extensions
        let without_ext = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        assert!(data.len() > without_ext.len());
    }

    #[test]
    fn test_encode_to_writer() {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);

        // Encode to Vec via encode_with_options
        let data_vec = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        // Encode to writer
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

        // Both should produce the same output
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

        // Encode to writer with compression
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

        // Should be gzip compressed
        assert_eq!(&data_writer[0..2], &[0x1f, 0x8b]);
    }
}
