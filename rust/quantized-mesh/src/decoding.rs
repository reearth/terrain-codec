//! Zero-copy decoder for quantized-mesh format.
//!
//! [`QuantizedMeshView`] borrows directly from the input byte slice and
//! exposes each section (vertices, indices, edges, extensions) as
//! `&[u8]` slices, with iterator helpers that lazily apply the
//! zigzag-delta and high-water-mark decoding without allocating
//! intermediate `Vec`s.
//!
//! For a fully-owned result use [`DecodedMesh::decode`] / [`DecodedMesh::decode_from`],
//! which transparently handle gzip decompression and materialise every
//! section into `Vec`s.

use std::io::{self, Read};

use flate2::read::GzDecoder;

use crate::{
    EdgeIndices, QuantizedMeshHeader, QuantizedVertices, TileMetadata, WaterMask, zigzag_decode,
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
            DecodeError::InvalidData(msg) => write!(f, "Invalid data: {msg}"),
            DecodeError::DecompressionError(msg) => write!(f, "Decompression error: {msg}"),
            DecodeError::JsonError(msg) => write!(f, "JSON error: {msg}"),
            DecodeError::IoError(err) => write!(f, "IO error: {err}"),
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

// ---------------------------------------------------------------------------
// View types (zero-copy)
// ---------------------------------------------------------------------------

/// Triangle / edge indices viewed as borrowed bytes.
///
/// The original quantized-mesh stream stores indices as either u16 or u32
/// depending on whether `vertex_count > 65535`. The view keeps the raw
/// little-endian bytes and decodes lazily via [`Self::iter`].
#[derive(Debug, Clone, Copy)]
pub enum IndicesView<'a> {
    /// 16-bit indices (used when `vertex_count <= 65535`). 2 bytes per index.
    U16(&'a [u8]),
    /// 32-bit indices. 4 bytes per index.
    U32(&'a [u8]),
}

impl<'a> IndicesView<'a> {
    /// Number of indices.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::U16(b) => b.len() / 2,
            Self::U32(b) => b.len() / 4,
        }
    }

    /// `true` if there are no indices.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterate the raw (still high-water-mark encoded) indices.
    pub fn iter_raw(&self) -> RawIndexIter<'a> {
        match self {
            Self::U16(b) => RawIndexIter::U16(b.chunks_exact(2)),
            Self::U32(b) => RawIndexIter::U32(b.chunks_exact(4)),
        }
    }

    /// Iterate the fully decoded triangle indices.
    pub fn iter(&self) -> HighWaterMarkIter<RawIndexIter<'a>> {
        HighWaterMarkIter::new(self.iter_raw())
    }

    /// Decode into an owned `Vec<u32>`.
    pub fn to_vec(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.len());
        out.extend(self.iter());
        out
    }
}

/// Iterator over raw little-endian-encoded indices (u16 or u32) yielding `u32`.
pub enum RawIndexIter<'a> {
    /// 16-bit chunks (2 bytes each).
    U16(std::slice::ChunksExact<'a, u8>),
    /// 32-bit chunks (4 bytes each).
    U32(std::slice::ChunksExact<'a, u8>),
}

impl<'a> Iterator for RawIndexIter<'a> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        match self {
            Self::U16(it) => it.next().map(|c| u16::from_le_bytes([c[0], c[1]]) as u32),
            Self::U32(it) => it
                .next()
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::U16(it) => it.size_hint(),
            Self::U32(it) => it.size_hint(),
        }
    }
}

impl<'a> ExactSizeIterator for RawIndexIter<'a> {}

/// Iterator that applies inverse zigzag-delta decoding to a stream of raw
/// little-endian u16 bytes, yielding decoded `u16` values.
pub struct ZigzagDeltaIter<'a> {
    chunks: std::slice::ChunksExact<'a, u8>,
    state: i32,
}

impl<'a> ZigzagDeltaIter<'a> {
    /// Wrap a byte slice (length must be a multiple of 2). Trailing odd
    /// bytes are silently ignored.
    #[inline]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            chunks: bytes.chunks_exact(2),
            state: 0,
        }
    }
}

impl<'a> Iterator for ZigzagDeltaIter<'a> {
    type Item = u16;

    #[inline]
    fn next(&mut self) -> Option<u16> {
        let chunk = self.chunks.next()?;
        let enc = u16::from_le_bytes([chunk[0], chunk[1]]) as u32;
        let delta = zigzag_decode(enc);
        self.state = self.state.wrapping_add(delta);
        Some(self.state as u16)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chunks.size_hint()
    }
}

impl<'a> ExactSizeIterator for ZigzagDeltaIter<'a> {}

/// Iterator that applies inverse high-water-mark decoding to an inner
/// iterator of raw `u32` codes.
pub struct HighWaterMarkIter<I> {
    inner: I,
    highest: u32,
}

impl<I> HighWaterMarkIter<I> {
    /// Wrap a code iterator.
    #[inline]
    pub fn new(inner: I) -> Self {
        Self { inner, highest: 0 }
    }
}

impl<I: Iterator<Item = u32>> Iterator for HighWaterMarkIter<I> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        let code = self.inner.next()?;
        let index = self.highest.wrapping_sub(code);
        if code == 0 {
            self.highest = self.highest.wrapping_add(1);
        }
        Some(index)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<I: Iterator<Item = u32> + ExactSizeIterator> ExactSizeIterator for HighWaterMarkIter<I> {}

/// Water mask viewed as borrowed bytes.
#[derive(Debug, Clone, Copy)]
pub enum WaterMaskView<'a> {
    /// Uniform mask: single byte (0 = land, 255 = water).
    Uniform(u8),
    /// 256×256 byte grid borrowed from the source buffer.
    Grid(&'a [u8; 256 * 256]),
}

impl<'a> WaterMaskView<'a> {
    /// Convert to the owned [`WaterMask`] form (copies the 64 KiB grid).
    pub fn to_owned(self) -> WaterMask {
        match self {
            Self::Uniform(v) => WaterMask::Uniform(v),
            Self::Grid(g) => {
                let mut owned = Box::new([0u8; 256 * 256]);
                owned.copy_from_slice(g.as_ref());
                WaterMask::Grid(owned)
            }
        }
    }
}

/// Extensions viewed as borrowed slices into the source buffer.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtensionsView<'a> {
    /// Oct-encoded normals — 2 bytes per vertex. Use [`oct_decode_normal`]
    /// or [`iter_oct_normals`] to decode.
    pub normals_oct: Option<&'a [u8]>,
    /// Water mask.
    pub water_mask: Option<WaterMaskView<'a>>,
    /// Raw metadata JSON. Use [`TileMetadata`]::from a `serde_json::from_str`
    /// to materialise.
    pub metadata_json: Option<&'a str>,
}

impl<'a> ExtensionsView<'a> {
    /// Materialise into the owned [`DecodedExtensions`] form.
    pub fn to_owned(&self) -> DecodeResult<DecodedExtensions> {
        Ok(DecodedExtensions {
            normals: self
                .normals_oct
                .map(|b| iter_oct_normals(b).collect::<Vec<_>>()),
            water_mask: self.water_mask.map(|w| w.to_owned()),
            metadata: self
                .metadata_json
                .map(serde_json::from_str::<TileMetadata>)
                .transpose()
                .map_err(|e| DecodeError::JsonError(e.to_string()))?,
        })
    }
}

/// Iterator over oct-encoded normals (2 bytes each), yielding `[f32; 3]`.
pub fn iter_oct_normals(bytes: &[u8]) -> impl Iterator<Item = [f32; 3]> + '_ {
    bytes
        .chunks_exact(2)
        .map(|c| oct_decode_normal([c[0], c[1]]))
}

/// Zero-copy view over a parsed quantized-mesh stream.
///
/// Every byte section (vertex / index / edge / extension data) is a slice
/// borrowed from the input. Use the `iter_*` helpers to decode on the fly,
/// or [`Self::into_owned`] to materialise everything into a [`DecodedMesh`].
#[derive(Debug, Clone, Copy)]
pub struct QuantizedMeshView<'a> {
    /// Parsed 88-byte header (copied — it's small and not aligned in the source).
    pub header: QuantizedMeshHeader,
    /// Number of vertices.
    pub vertex_count: u32,
    /// Whether the index stream uses 32-bit indices (`vertex_count > 65535`).
    pub use_32bit_indices: bool,
    /// Number of triangles.
    pub triangle_count: u32,

    /// Zigzag-delta encoded `u` coordinate bytes (2 bytes per vertex).
    pub encoded_u_bytes: &'a [u8],
    /// Zigzag-delta encoded `v` coordinate bytes.
    pub encoded_v_bytes: &'a [u8],
    /// Zigzag-delta encoded `height` bytes.
    pub encoded_height_bytes: &'a [u8],

    /// High-water-mark encoded triangle indices.
    pub indices: IndicesView<'a>,
    /// West-edge indices (raw little-endian).
    pub edge_west: IndicesView<'a>,
    /// South-edge indices.
    pub edge_south: IndicesView<'a>,
    /// East-edge indices.
    pub edge_east: IndicesView<'a>,
    /// North-edge indices.
    pub edge_north: IndicesView<'a>,

    /// Parsed extension views.
    pub extensions: ExtensionsView<'a>,
}

impl<'a> QuantizedMeshView<'a> {
    /// Parse a raw (already gzip-decompressed) quantized-mesh byte stream.
    pub fn parse(data: &'a [u8]) -> DecodeResult<Self> {
        let mut cur = Cursor::new(data);
        let header = QuantizedMeshHeader::from_bytes(cur.take(88)?)
            .ok_or_else(|| DecodeError::InvalidData("Invalid header".to_string()))?;

        let vertex_count = cur.read_u32()?;
        let use_32bit = vertex_count > 65535;
        let vc = vertex_count as usize;

        let encoded_u_bytes = cur.take(vc * 2)?;
        let encoded_v_bytes = cur.take(vc * 2)?;
        let encoded_height_bytes = cur.take(vc * 2)?;

        cur.align_to(if use_32bit { 4 } else { 2 });

        let triangle_count = cur.read_u32()?;
        let index_stride = if use_32bit { 4 } else { 2 };
        let index_bytes = cur.take(triangle_count as usize * 3 * index_stride)?;
        let indices = if use_32bit {
            IndicesView::U32(index_bytes)
        } else {
            IndicesView::U16(index_bytes)
        };

        let edge_west = read_edge_indices(&mut cur, use_32bit)?;
        let edge_south = read_edge_indices(&mut cur, use_32bit)?;
        let edge_east = read_edge_indices(&mut cur, use_32bit)?;
        let edge_north = read_edge_indices(&mut cur, use_32bit)?;

        let extensions = parse_extensions(&mut cur, vc)?;

        Ok(Self {
            header,
            vertex_count,
            use_32bit_indices: use_32bit,
            triangle_count,
            encoded_u_bytes,
            encoded_v_bytes,
            encoded_height_bytes,
            indices,
            edge_west,
            edge_south,
            edge_east,
            edge_north,
            extensions,
        })
    }

    /// Lazily decoded `u` coordinates.
    #[inline]
    pub fn iter_u(&self) -> ZigzagDeltaIter<'a> {
        ZigzagDeltaIter::new(self.encoded_u_bytes)
    }

    /// Lazily decoded `v` coordinates.
    #[inline]
    pub fn iter_v(&self) -> ZigzagDeltaIter<'a> {
        ZigzagDeltaIter::new(self.encoded_v_bytes)
    }

    /// Lazily decoded `height` values.
    #[inline]
    pub fn iter_height(&self) -> ZigzagDeltaIter<'a> {
        ZigzagDeltaIter::new(self.encoded_height_bytes)
    }

    /// Materialise into a fully owned [`DecodedMesh`].
    pub fn into_owned(self) -> DecodeResult<DecodedMesh> {
        let vertices = QuantizedVertices {
            u: self.iter_u().collect(),
            v: self.iter_v().collect(),
            height: self.iter_height().collect(),
        };
        let indices = self.indices.to_vec();
        let edge_indices = EdgeIndices {
            west: self.edge_west.iter_raw().collect(),
            south: self.edge_south.iter_raw().collect(),
            east: self.edge_east.iter_raw().collect(),
            north: self.edge_north.iter_raw().collect(),
        };
        let extensions = self.extensions.to_owned()?;
        Ok(DecodedMesh {
            header: self.header,
            vertices,
            indices,
            edge_indices,
            extensions,
        })
    }
}

fn read_edge_indices<'a>(cur: &mut Cursor<'a>, use_32bit: bool) -> DecodeResult<IndicesView<'a>> {
    let count = cur.read_u32()? as usize;
    let stride = if use_32bit { 4 } else { 2 };
    let bytes = cur.take(count * stride)?;
    Ok(if use_32bit {
        IndicesView::U32(bytes)
    } else {
        IndicesView::U16(bytes)
    })
}

fn parse_extensions<'a>(
    cur: &mut Cursor<'a>,
    vertex_count: usize,
) -> DecodeResult<ExtensionsView<'a>> {
    let mut ext = ExtensionsView::default();
    while cur.remaining() >= 5 {
        let id = cur.read_u8()?;
        let length = cur.read_u32()? as usize;
        if cur.remaining() < length {
            // Truncated extension; bail without erroring (matches old behaviour).
            break;
        }
        let payload = cur.take(length)?;
        match id {
            1 => {
                // Oct-encoded normals: 2 bytes per vertex.
                let want = vertex_count * 2;
                let bytes = if payload.len() >= want {
                    &payload[..want]
                } else {
                    payload
                };
                ext.normals_oct = Some(bytes);
            }
            2 => {
                ext.water_mask = Some(match length {
                    1 => WaterMaskView::Uniform(payload[0]),
                    n if n == 256 * 256 => {
                        let arr: &[u8; 256 * 256] = payload
                            .try_into()
                            .map_err(|_| DecodeError::InvalidData("water mask size".into()))?;
                        WaterMaskView::Grid(arr)
                    }
                    _ => WaterMaskView::Uniform(0),
                });
            }
            4 => {
                if payload.len() < 4 {
                    return Err(DecodeError::UnexpectedEof);
                }
                let json_len =
                    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                if payload.len() < 4 + json_len {
                    return Err(DecodeError::UnexpectedEof);
                }
                let json_bytes = &payload[4..4 + json_len];
                let json_str = std::str::from_utf8(json_bytes)
                    .map_err(|e| DecodeError::JsonError(e.to_string()))?;
                ext.metadata_json = Some(json_str);
            }
            _ => {} // unknown extension — already advanced past it
        }
    }
    Ok(ext)
}

// ---------------------------------------------------------------------------
// Owned form (backward-compatible)
// ---------------------------------------------------------------------------

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

/// Fully owned decoded quantized-mesh data.
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

impl DecodedMesh {
    /// Decode a (possibly gzip-compressed) quantized-mesh byte slice into the
    /// fully owned form.
    pub fn decode(data: &[u8]) -> DecodeResult<Self> {
        if is_gzip(data) {
            let mut decompressed = Vec::new();
            GzDecoder::new(data)
                .read_to_end(&mut decompressed)
                .map_err(|e| DecodeError::DecompressionError(e.to_string()))?;
            QuantizedMeshView::parse(&decompressed)?.into_owned()
        } else {
            QuantizedMeshView::parse(data)?.into_owned()
        }
    }

    /// Decode from a `Read`er. The reader is fully consumed.
    pub fn decode_from<R: Read>(mut reader: R) -> DecodeResult<Self> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Self::decode(&buf)
    }
}

/// Check if `data` starts with the gzip magic number.
#[inline]
pub fn is_gzip(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b
}

/// Decompress a gzip-compressed quantized-mesh blob. Returns the input unchanged
/// (copied) if it isn't gzip.
pub fn decompress_gzip(data: &[u8]) -> DecodeResult<Vec<u8>> {
    if is_gzip(data) {
        let mut out = Vec::new();
        GzDecoder::new(data)
            .read_to_end(&mut out)
            .map_err(|e| DecodeError::DecompressionError(e.to_string()))?;
        Ok(out)
    } else {
        Ok(data.to_vec())
    }
}

// ---------------------------------------------------------------------------
// Internal cursor
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    #[inline]
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    fn take(&mut self, n: usize) -> DecodeResult<&'a [u8]> {
        if self.remaining() < n {
            return Err(DecodeError::UnexpectedEof);
        }
        let slice = &self.data[self.offset..self.offset + n];
        self.offset += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> DecodeResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self) -> DecodeResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn align_to(&mut self, alignment: usize) {
        let rem = self.offset % alignment;
        if rem != 0 {
            self.offset += alignment - rem;
        }
    }
}

// ---------------------------------------------------------------------------
// Oct decode (also used by encoder tests)
// ---------------------------------------------------------------------------

/// Decode oct-encoded normal to unit vector.
///
/// Reverses the octahedron encoding used for normal compression.
pub fn oct_decode_normal(encoded: [u8; 2]) -> [f32; 3] {
    let mut x = (encoded[0] as f32 / 255.0) * 2.0 - 1.0;
    let mut y = (encoded[1] as f32 / 255.0) * 2.0 - 1.0;
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        let ox = x;
        x = (1.0 - y.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if y >= 0.0 { 1.0 } else { -1.0 };
    }
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
    use crate::{EncodeOptions, QuantizedMeshEncoder, oct_encode_normal};

    fn sample_mesh() -> (
        QuantizedMeshHeader,
        QuantizedVertices,
        Vec<u32>,
        EdgeIndices,
    ) {
        let header = QuantizedMeshHeader::default();
        let vertices = QuantizedVertices {
            u: vec![0, 32767, 0, 32767],
            v: vec![0, 0, 32767, 32767],
            height: vec![0, 0, 0, 0],
        };
        let indices = vec![0, 1, 2, 1, 3, 2];
        let edge_indices = EdgeIndices::from_vertices(&vertices);
        (header, vertices, indices, edge_indices)
    }

    #[test]
    fn test_oct_decode_normal_roundtrip() {
        let test_normals = [
            [0.0f32, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.577, 0.577, 0.577],
        ];
        for normal in test_normals {
            let encoded = oct_encode_normal(normal);
            let decoded = oct_decode_normal(encoded);
            let dot = normal[0] * decoded[0] + normal[1] * decoded[1] + normal[2] * decoded[2];
            assert!(dot > 0.95, "{normal:?} → {encoded:?} → {decoded:?}");
        }
    }

    #[test]
    fn view_parses_uncompressed_mesh() {
        let (header, vertices, indices, edge_indices) = sample_mesh();
        let encoder = QuantizedMeshEncoder::new(
            header,
            vertices.clone(),
            indices.clone(),
            edge_indices.clone(),
        );
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        let view = QuantizedMeshView::parse(&encoded).expect("parse");
        assert_eq!(view.vertex_count as usize, vertices.u.len());
        assert_eq!(view.triangle_count as usize * 3, indices.len());

        let u: Vec<u16> = view.iter_u().collect();
        let v: Vec<u16> = view.iter_v().collect();
        let h: Vec<u16> = view.iter_height().collect();
        assert_eq!(u, vertices.u);
        assert_eq!(v, vertices.v);
        assert_eq!(h, vertices.height);

        let idx: Vec<u32> = view.indices.iter().collect();
        assert_eq!(idx, indices);
    }

    #[test]
    fn owned_decode_roundtrips() {
        let (header, vertices, indices, edge_indices) = sample_mesh();
        let encoder = QuantizedMeshEncoder::new(
            header,
            vertices.clone(),
            indices.clone(),
            edge_indices.clone(),
        );
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            ..Default::default()
        });

        let decoded = DecodedMesh::decode(&encoded).expect("decode");
        assert_eq!(decoded.header.min_height, header.min_height);
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
    fn owned_decode_handles_gzip() {
        let (header, vertices, indices, edge_indices) = sample_mesh();
        let encoder =
            QuantizedMeshEncoder::new(header, vertices.clone(), indices.clone(), edge_indices);
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 6,
            ..Default::default()
        });
        assert_eq!(&encoded[0..2], &[0x1f, 0x8b]);

        let decoded = DecodedMesh::decode(&encoded).expect("decode");
        assert_eq!(decoded.vertices.u, vertices.u);
        assert_eq!(decoded.indices, indices);
    }

    #[test]
    fn owned_decode_with_extensions() {
        let (header, vertices, indices, edge_indices) = sample_mesh();
        let normals = vec![[0.0_f32, 0.0, 1.0]; 4];
        let encoder = QuantizedMeshEncoder::new(header, vertices.clone(), indices, edge_indices);
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 0,
            include_normals: true,
            normals: Some(normals),
            include_water_mask: true,
            water_mask: Some(WaterMask::Uniform(128)),
            ..Default::default()
        });

        let decoded = DecodedMesh::decode(&encoded).expect("decode");
        let ns = decoded.extensions.normals.expect("normals");
        assert_eq!(ns.len(), 4);
        for n in ns {
            assert!(n[2] > 0.9);
        }
        match decoded.extensions.water_mask.expect("water mask") {
            WaterMask::Uniform(v) => assert_eq!(v, 128),
            _ => panic!("expected uniform"),
        }
    }

    #[test]
    fn owned_decode_from_reader_handles_gzip() {
        use std::io::Cursor;
        let (header, vertices, indices, edge_indices) = sample_mesh();
        let encoder =
            QuantizedMeshEncoder::new(header, vertices.clone(), indices.clone(), edge_indices);
        let encoded = encoder.encode_with_options(&EncodeOptions {
            compression_level: 6,
            ..Default::default()
        });
        let decoded = DecodedMesh::decode_from(Cursor::new(encoded)).expect("decode");
        assert_eq!(decoded.vertices.u, vertices.u);
        assert_eq!(decoded.indices, indices);
    }
}
