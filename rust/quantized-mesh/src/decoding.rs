//! Decoding functions and main decoder for quantized-mesh format.

use std::io::{self, BufReader, Read};

use flate2::read::GzDecoder;

use crate::{
    decode_high_water_mark, decode_zigzag_delta, EdgeIndices, QuantizedMeshHeader,
    QuantizedVertices, TileMetadata, WaterMask,
};

/// Error type for quantized-mesh decoding.
#[derive(Debug)]
pub enum DecodeError {
    /// Input data is too short.
    UnexpectedEof,
    /// Invalid or corrupted data.
    InvalidData(String),
    /// Gzip decompression failed.
    DecompressionError(String),
    /// JSON parsing failed for metadata extension.
    JsonError(String),
    /// IO error.
    IoError(io::Error),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::UnexpectedEof => write!(f, "Unexpected end of data"),
            DecodeError::InvalidData(msg) => write!(f, "Invalid data: {}", msg),
            DecodeError::DecompressionError(msg) => write!(f, "Decompression error: {}", msg),
            DecodeError::JsonError(msg) => write!(f, "JSON error: {}", msg),
            DecodeError::IoError(err) => write!(f, "IO error: {}", err),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<io::Error> for DecodeError {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::UnexpectedEof {
            DecodeError::UnexpectedEof
        } else {
            DecodeError::IoError(err)
        }
    }
}

/// Result type for quantized-mesh decoding.
pub type DecodeResult<T> = Result<T, DecodeError>;

/// Decoded extensions from quantized-mesh format.
#[derive(Debug, Clone, Default)]
pub struct DecodedExtensions {
    /// Oct-decoded per-vertex normals (if present).
    pub normals: Option<Vec<[f32; 3]>>,
    /// Water mask (if present).
    pub water_mask: Option<WaterMask>,
    /// Metadata (if present).
    pub metadata: Option<TileMetadata>,
}

/// Decoded quantized-mesh data.
#[derive(Debug, Clone)]
pub struct DecodedMesh {
    /// Header with tile metadata.
    pub header: QuantizedMeshHeader,
    /// Quantized vertex data.
    pub vertices: QuantizedVertices,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Edge indices for skirt generation.
    pub edge_indices: EdgeIndices,
    /// Decoded extensions.
    pub extensions: DecodedExtensions,
}

/// Quantized mesh decoder.
///
/// Decodes terrain mesh data from the quantized-mesh-1.0 format.
pub struct QuantizedMeshDecoder<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> QuantizedMeshDecoder<'a> {
    /// Create a new decoder from raw bytes.
    ///
    /// Automatically detects and handles gzip compression.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// Decode the quantized-mesh data from a byte slice.
    ///
    /// If the data is gzip compressed, it will be decompressed first.
    pub fn decode(data: &[u8]) -> DecodeResult<DecodedMesh> {
        // Check for gzip magic number
        let decompressed: Vec<u8>;
        let data = if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
            // Gzip compressed
            let mut decoder = GzDecoder::new(data);
            decompressed = Vec::new();
            let mut buf = decompressed;
            decoder
                .read_to_end(&mut buf)
                .map_err(|e| DecodeError::DecompressionError(e.to_string()))?;
            buf
        } else {
            data.to_vec()
        };

        let mut decoder = QuantizedMeshDecoder::new(&data);
        decoder.decode_internal()
    }

    /// Decode the quantized-mesh data from a reader.
    ///
    /// If the data is gzip compressed, it will be decompressed first.
    /// The reader will be read to the end.
    pub fn decode_from<R: Read>(reader: R) -> DecodeResult<DecodedMesh> {
        let mut reader = BufReader::new(reader);

        // Read first two bytes to check for gzip magic
        let mut magic = [0u8; 2];
        reader.read_exact(&mut magic)?;

        // Read the rest of the data
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest)?;

        // Prepend magic bytes
        let mut data = magic.to_vec();
        data.extend(rest);

        // Check for gzip magic number
        let decompressed = if magic[0] == 0x1f && magic[1] == 0x8b {
            // Gzip compressed
            let mut decoder = GzDecoder::new(data.as_slice());
            let mut buf = Vec::new();
            decoder
                .read_to_end(&mut buf)
                .map_err(|e| DecodeError::DecompressionError(e.to_string()))?;
            buf
        } else {
            data
        };

        let mut decoder = QuantizedMeshDecoder::new(&decompressed);
        decoder.decode_internal()
    }

    fn decode_internal(&mut self) -> DecodeResult<DecodedMesh> {
        // Read header (88 bytes)
        let header = self.read_header()?;

        // Read vertex count
        let vertex_count = self.read_u32()? as usize;
        let use_32bit = vertex_count > 65535;

        // Read encoded vertex data
        let encoded_u = self.read_u16_array(vertex_count)?;
        let encoded_v = self.read_u16_array(vertex_count)?;
        let encoded_height = self.read_u16_array(vertex_count)?;

        // Decode vertices
        let u = decode_zigzag_delta(&encoded_u);
        let v = decode_zigzag_delta(&encoded_v);
        let height = decode_zigzag_delta(&encoded_height);

        let vertices = QuantizedVertices { u, v, height };

        // Align for index reading
        if use_32bit {
            self.align_to(4);
        } else {
            self.align_to(2);
        }

        // Read triangle count
        let triangle_count = self.read_u32()? as usize;
        let index_count = triangle_count * 3;

        // Read encoded indices
        let encoded_indices = if use_32bit {
            self.read_u32_array(index_count)?
        } else {
            self.read_u16_array(index_count)?
                .into_iter()
                .map(|x| x as u32)
                .collect()
        };

        // Decode indices
        let indices = decode_high_water_mark(&encoded_indices);

        // Read edge indices
        let west = self.read_edge_indices(use_32bit)?;
        let south = self.read_edge_indices(use_32bit)?;
        let east = self.read_edge_indices(use_32bit)?;
        let north = self.read_edge_indices(use_32bit)?;

        let edge_indices = EdgeIndices {
            west,
            south,
            east,
            north,
        };

        // Read extensions (if any remaining data)
        let extensions = self.read_extensions(vertex_count)?;

        Ok(DecodedMesh {
            header,
            vertices,
            indices,
            edge_indices,
            extensions,
        })
    }

    fn read_header(&mut self) -> DecodeResult<QuantizedMeshHeader> {
        if self.remaining() < 88 {
            return Err(DecodeError::UnexpectedEof);
        }

        let header = QuantizedMeshHeader::from_bytes(&self.data[self.offset..])
            .ok_or_else(|| DecodeError::InvalidData("Invalid header".to_string()))?;

        self.offset += 88;
        Ok(header)
    }

    fn read_u16(&mut self) -> DecodeResult<u16> {
        if self.remaining() < 2 {
            return Err(DecodeError::UnexpectedEof);
        }
        let value = u16::from_le_bytes([self.data[self.offset], self.data[self.offset + 1]]);
        self.offset += 2;
        Ok(value)
    }

    fn read_u32(&mut self) -> DecodeResult<u32> {
        if self.remaining() < 4 {
            return Err(DecodeError::UnexpectedEof);
        }
        let value = u32::from_le_bytes([
            self.data[self.offset],
            self.data[self.offset + 1],
            self.data[self.offset + 2],
            self.data[self.offset + 3],
        ]);
        self.offset += 4;
        Ok(value)
    }

    fn read_u16_array(&mut self, count: usize) -> DecodeResult<Vec<u16>> {
        let byte_count = count * 2;
        if self.remaining() < byte_count {
            return Err(DecodeError::UnexpectedEof);
        }

        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            result.push(self.read_u16()?);
        }
        Ok(result)
    }

    fn read_u32_array(&mut self, count: usize) -> DecodeResult<Vec<u32>> {
        let byte_count = count * 4;
        if self.remaining() < byte_count {
            return Err(DecodeError::UnexpectedEof);
        }

        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            result.push(self.read_u32()?);
        }
        Ok(result)
    }

    fn read_edge_indices(&mut self, use_32bit: bool) -> DecodeResult<Vec<u32>> {
        let count = self.read_u32()? as usize;
        if use_32bit {
            self.read_u32_array(count)
        } else {
            Ok(self
                .read_u16_array(count)?
                .into_iter()
                .map(|x| x as u32)
                .collect())
        }
    }

    fn read_extensions(&mut self, vertex_count: usize) -> DecodeResult<DecodedExtensions> {
        let mut extensions = DecodedExtensions::default();

        while self.remaining() >= 5 {
            // At least extension ID (1) + length (4)
            let extension_id = self.data[self.offset];
            self.offset += 1;

            let length = self.read_u32()? as usize;
            if self.remaining() < length {
                // Not enough data for this extension, skip
                break;
            }

            match extension_id {
                1 => {
                    // OctEncodedVertexNormals
                    extensions.normals = Some(self.read_normals(vertex_count)?);
                }
                2 => {
                    // WaterMask
                    extensions.water_mask = Some(self.read_water_mask(length)?);
                }
                4 => {
                    // Metadata
                    extensions.metadata = Some(self.read_metadata()?);
                }
                _ => {
                    // Unknown extension, skip
                    self.offset += length;
                }
            }
        }

        Ok(extensions)
    }

    fn read_normals(&mut self, vertex_count: usize) -> DecodeResult<Vec<[f32; 3]>> {
        let mut normals = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            if self.remaining() < 2 {
                return Err(DecodeError::UnexpectedEof);
            }
            let encoded = [self.data[self.offset], self.data[self.offset + 1]];
            self.offset += 2;
            normals.push(oct_decode_normal(encoded));
        }
        Ok(normals)
    }

    fn read_water_mask(&mut self, length: usize) -> DecodeResult<WaterMask> {
        if length == 1 {
            if self.remaining() < 1 {
                return Err(DecodeError::UnexpectedEof);
            }
            let value = self.data[self.offset];
            self.offset += 1;
            Ok(WaterMask::Uniform(value))
        } else if length == 256 * 256 {
            if self.remaining() < 256 * 256 {
                return Err(DecodeError::UnexpectedEof);
            }
            let mut grid = Box::new([0u8; 256 * 256]);
            grid.copy_from_slice(&self.data[self.offset..self.offset + 256 * 256]);
            self.offset += 256 * 256;
            Ok(WaterMask::Grid(grid))
        } else {
            // Unknown water mask format, skip
            self.offset += length;
            Ok(WaterMask::Uniform(0))
        }
    }

    fn read_metadata(&mut self) -> DecodeResult<TileMetadata> {
        let json_length = self.read_u32()? as usize;
        if self.remaining() < json_length {
            return Err(DecodeError::UnexpectedEof);
        }

        let json_bytes = &self.data[self.offset..self.offset + json_length];
        self.offset += json_length;

        let json_str =
            std::str::from_utf8(json_bytes).map_err(|e| DecodeError::JsonError(e.to_string()))?;

        serde_json::from_str(json_str).map_err(|e| DecodeError::JsonError(e.to_string()))
    }

    fn align_to(&mut self, alignment: usize) {
        let remainder = self.offset % alignment;
        if remainder != 0 {
            self.offset += alignment - remainder;
        }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }
}

/// Decode oct-encoded normal to unit vector.
///
/// Reverses the octahedron encoding used for normal compression.
pub fn oct_decode_normal(encoded: [u8; 2]) -> [f32; 3] {
    // Map from [0, 255] to [-1, 1]
    let mut x = (encoded[0] as f32 / 255.0) * 2.0 - 1.0;
    let mut y = (encoded[1] as f32 / 255.0) * 2.0 - 1.0;

    // Reconstruct z from octahedron
    let z = 1.0 - x.abs() - y.abs();

    // Fold lower hemisphere
    if z < 0.0 {
        let ox = x;
        x = (1.0 - y.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if y >= 0.0 { 1.0 } else { -1.0 };
    }

    // Normalize
    let len = (x * x + y * y + z * z).sqrt();
    if len > 0.0 {
        [x / len, y / len, z / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{oct_encode_normal, EncodeOptions, QuantizedMeshEncoder};

    #[test]
    fn test_oct_decode_normal_roundtrip() {
        let test_normals = [
            [0.0f32, 0.0, 1.0],   // Up
            [0.0, 0.0, -1.0],     // Down
            [1.0, 0.0, 0.0],      // Right
            [0.0, 1.0, 0.0],      // Forward
            [0.577, 0.577, 0.577], // Diagonal (roughly normalized)
        ];

        for normal in test_normals {
            let encoded = oct_encode_normal(normal);
            let decoded = oct_decode_normal(encoded);

            // Check that decoded is close to original (with some precision loss)
            let dot = normal[0] * decoded[0] + normal[1] * decoded[1] + normal[2] * decoded[2];
            assert!(
                dot > 0.95,
                "Normal roundtrip failed: {:?} -> {:?} -> {:?}, dot = {}",
                normal,
                encoded,
                decoded,
                dot
            );
        }
    }

    #[test]
    fn test_decode_simple_mesh() {
        // Create a simple mesh
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        // Encode
        let encoder =
            QuantizedMeshEncoder::new(header.clone(), vertices.clone(), indices.clone(), edge_indices.clone());
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        // Decode
        let decoded = QuantizedMeshDecoder::decode(&encoded).expect("Decoding failed");

        // Verify
        assert_eq!(decoded.header.min_height, header.min_height);
        assert_eq!(decoded.header.max_height, header.max_height);
        assert_eq!(decoded.vertices.u, vertices.u);
        assert_eq!(decoded.vertices.v, vertices.v);
        assert_eq!(decoded.vertices.height, vertices.height);
        assert_eq!(decoded.indices, indices);
        assert_eq!(decoded.edge_indices.west, edge_indices.west);
        assert_eq!(decoded.edge_indices.south, edge_indices.south);
        assert_eq!(decoded.edge_indices.east, edge_indices.east);
        assert_eq!(decoded.edge_indices.north, edge_indices.north);
    }

    #[test]
    fn test_decode_compressed_mesh() {
        // Create a simple mesh
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        // Encode with compression
        let encoder =
            QuantizedMeshEncoder::new(header.clone(), vertices.clone(), indices.clone(), edge_indices.clone());
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 6,
            ..Default::default()
        });

        // Verify it's gzip compressed
        assert_eq!(&encoded[0..2], &[0x1f, 0x8b]);

        // Decode
        let decoded = QuantizedMeshDecoder::decode(&encoded).expect("Decoding failed");

        // Verify
        assert_eq!(decoded.vertices.u, vertices.u);
        assert_eq!(decoded.vertices.v, vertices.v);
        assert_eq!(decoded.indices, indices);
    }

    #[test]
    fn test_decode_with_extensions() {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);
        let normals = vec![[0.0, 0.0, 1.0]; 4];

        // Encode with extensions
        let encoder = QuantizedMeshEncoder::new(header, vertices.clone(), indices, edge_indices);
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            include_normals: true,
            normals: Some(normals),
            include_water_mask: true,
            water_mask: Some(WaterMask::Uniform(128)),
            ..Default::default()
        });

        // Decode
        let decoded = QuantizedMeshDecoder::decode(&encoded).expect("Decoding failed");

        // Verify extensions
        assert!(decoded.extensions.normals.is_some());
        assert!(decoded.extensions.water_mask.is_some());

        let decoded_normals = decoded.extensions.normals.unwrap();
        assert_eq!(decoded_normals.len(), 4);
        // Check that normals point roughly upward
        for normal in decoded_normals {
            assert!(normal[2] > 0.9);
        }

        match decoded.extensions.water_mask.unwrap() {
            WaterMask::Uniform(v) => assert_eq!(v, 128),
            _ => panic!("Expected uniform water mask"),
        }
    }

    #[test]
    fn test_decode_from_reader() {
        use std::io::Cursor;

        // Create a simple mesh
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        // Encode
        let encoder =
            QuantizedMeshEncoder::new(header.clone(), vertices.clone(), indices.clone(), edge_indices.clone());
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        // Decode from reader
        let reader = Cursor::new(encoded);
        let decoded = QuantizedMeshDecoder::decode_from(reader).expect("Decoding from reader failed");

        // Verify
        assert_eq!(decoded.vertices.u, vertices.u);
        assert_eq!(decoded.vertices.v, vertices.v);
        assert_eq!(decoded.indices, indices);
    }

    #[test]
    fn test_decode_from_reader_compressed() {
        use std::io::Cursor;

        // Create a simple mesh
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);

        // Encode with compression
        let encoder =
            QuantizedMeshEncoder::new(header.clone(), vertices.clone(), indices.clone(), edge_indices.clone());
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 6,
            ..Default::default()
        });

        // Decode from reader
        let reader = Cursor::new(encoded);
        let decoded = QuantizedMeshDecoder::decode_from(reader).expect("Decoding from reader failed");

        // Verify
        assert_eq!(decoded.vertices.u, vertices.u);
        assert_eq!(decoded.vertices.v, vertices.v);
        assert_eq!(decoded.indices, indices);
    }
}
