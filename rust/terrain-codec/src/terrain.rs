//! One-shot heightmap → quantized-mesh (`.terrain`) encoding.
//!
//! This module ties together the three crates that otherwise have to be
//! wired up by hand:
//!
//! 1. [`martini`] generates an adaptive RTIN mesh from the elevation grid.
//! 2. The mesh's `(u, v, height)` are quantised to the 0..=32767 range.
//! 3. [`quantized_mesh`] encodes the header, vertices, edge indices and
//!    optional extensions into the quantized-mesh-1.0 byte stream.
//!
//! The fiddly bits it handles for you:
//!
//! - Re-sampling each mesh vertex's height (martini discards heights once
//!   the error pyramid is built, so the transform has to recover the grid
//!   coordinate from `(u, v)` and read the DEM again).
//! - Computing the encoded height range from the *mesh* vertices (what is
//!   actually stored), not the full grid.
//! - Streaming the mesh vertices through
//!   [`QuantizedMeshHeader::from_bounds_with_vertices_iter`] for a tight
//!   horizon-occlusion point.
//! - Vertex normals via the [`NormalMode`] of your choice.
//!
//! # Grid orientation
//!
//! `elevations` (and the `get_height` closure's `y`) are **row-major,
//! north → south**: row `0` is the northern edge, row `grid_size - 1` the
//! southern edge. This matches [`crate::normals::BufferedElevations`], so a
//! buffered grid can be reused directly for [`NormalMode::BufferedGradient`].
//!
//! # Seamless tiling — the caller supplies the halo
//!
//! These functions encode **one tile in isolation**; they never fetch
//! neighbouring tiles. For gap-free, seam-free output the *caller* must
//! widen the input to overlap the neighbours — fetch the halo cells along
//! with the tile and stitch them in before calling:
//!
//! - **Geometry seam.** martini needs a `2^n + 1` grid, so an `N`-post DEM
//!   tile needs one extra post on its east and south edges. That `+1` post
//!   is the neighbour tile's *first* post for the shared edge — read it
//!   from the neighbour, don't edge-replicate, or adjacent tiles won't
//!   agree on the boundary and the globe cracks along tile seams.
//! - **Normal seam.** [`NormalMode::BufferedGradient`] needs a
//!   `buffer`-cell halo of neighbour samples on **every** side (see
//!   [`BufferedElevations`]). Edge vertices read their `±1` neighbours out
//!   of that halo, so the same physical edge gets identical normals from
//!   either tile and lighting stays continuous.
//!
//! Gathering that neighbour data (over HTTP, from disk, from a cache, …) is
//! deliberately left to the caller — hence this module takes an
//! already-assembled grid rather than fetching tiles itself, which also
//! keeps it free of any async/runtime assumptions.
//!
//! # Example
//!
//! ```
//! use terrain_codec::quantized_mesh::TileBounds;
//! use terrain_codec::terrain::{encode_terrain, TerrainOptions};
//!
//! let grid_size = 65; // 2^6 + 1
//! let elevations = vec![0.0f32; (grid_size * grid_size) as usize];
//! let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
//!
//! let terrain: Vec<u8> = encode_terrain(
//!     &elevations,
//!     grid_size,
//!     &bounds,
//!     &TerrainOptions {
//!         max_error: 1.0,
//!         ..Default::default()
//!     },
//! );
//! assert!(terrain.starts_with(&[0x1f, 0x8b])); // gzip magic (default level 6)
//! ```

use std::io::{self, Write};

use martini::Martini;
use quantized_mesh::{
    EdgeIndices, EncodeOptions, QUANTIZED_MAX, QuantizedMeshEncoder, QuantizedMeshHeader,
    QuantizedVertices, TileBounds, TileMetadata, WaterMask,
};

use crate::normals::{BufferedElevations, buffered_gradient_normals, face_normals};

/// How (and whether) per-vertex normals are computed for the oct-encoded
/// vertex-normals extension.
#[derive(Debug, Clone, Default)]
pub enum NormalMode {
    /// No vertex-normals extension.
    #[default]
    None,
    /// Per-tile face normals ([`crate::normals::face_normals`]). Simple, but
    /// produces a visible shading seam at tile boundaries.
    FaceNormals,
    /// Seam-free DEM-gradient normals
    /// ([`crate::normals::buffered_gradient_normals`]) sampled from a
    /// buffer-extended grid. Its `tile_grid_size` must equal the encode
    /// `grid_size`.
    ///
    /// The caller is responsible for filling the `buffer`-cell halo around
    /// the tile with the **neighbour tiles'** elevations — that overlap is
    /// what makes edge normals match across the seam. A halo filled by
    /// edge-replication still encodes fine, but won't be seam-free.
    BufferedGradient(BufferedElevations),
}

/// Options controlling [`encode_terrain`] and the other encode functions in
/// this module.
#[derive(Debug, Clone)]
pub struct TerrainOptions {
    /// Maximum RTIN error threshold in metres. Lower values keep more
    /// triangles (higher fidelity, larger output).
    pub max_error: f64,
    /// Gzip compression level: `0` emits uncompressed bytes, `1..=9` gzip at
    /// that level. Defaults to `6`.
    pub compression_level: u32,
    /// Vertex-normal strategy.
    pub normals: NormalMode,
    /// Optional water-mask extension.
    pub water_mask: Option<WaterMask>,
    /// Optional metadata (child-tile availability) extension.
    pub metadata: Option<TileMetadata>,
}

impl Default for TerrainOptions {
    fn default() -> Self {
        Self {
            max_error: 1.0,
            compression_level: 6,
            normals: NormalMode::None,
            water_mask: None,
            metadata: None,
        }
    }
}

/// Encode a heightmap to a quantized-mesh `.terrain` byte vector, sampling
/// elevations through a closure.
///
/// `get_height(x, y)` returns the elevation in metres at grid column `x`
/// (`0..grid_size`, west → east) and row `y` (`0..grid_size`, north →
/// south).
///
/// This is the primitive form; [`encode_terrain`] wraps it for a flat
/// `&[f32]` grid. `get_height` is called twice per grid vertex that ends up
/// in the mesh (once while building the error pyramid, once to recover the
/// stored height), so keep it cheap or memoised.
///
/// # Panics
///
/// Panics if `grid_size` is not `2^n + 1`, or — for
/// [`NormalMode::BufferedGradient`] — if the buffered grid's
/// `tile_grid_size` does not equal `grid_size`.
pub fn encode_terrain_from_fn<F>(
    grid_size: u32,
    bounds: &TileBounds,
    get_height: F,
    options: &TerrainOptions,
) -> Vec<u8>
where
    F: Fn(u32, u32) -> f64,
{
    let (encoder, encode_opts) = build(grid_size, bounds, get_height, options);
    encoder.encode_with_options(&encode_opts)
}

/// Like [`encode_terrain_from_fn`], but streams the encoded bytes to a
/// writer instead of allocating a `Vec`.
///
/// # Panics
///
/// Same panics as [`encode_terrain_from_fn`].
pub fn encode_terrain_from_fn_to<F, W>(
    grid_size: u32,
    bounds: &TileBounds,
    get_height: F,
    options: &TerrainOptions,
    writer: W,
) -> io::Result<()>
where
    F: Fn(u32, u32) -> f64,
    W: Write,
{
    let (encoder, encode_opts) = build(grid_size, bounds, get_height, options);
    encoder.encode_to_with_options(writer, &encode_opts)
}

/// Encode a flat row-major (north → south) `f32` elevation grid to a
/// quantized-mesh `.terrain` byte vector.
///
/// `elevations.len()` must equal `grid_size * grid_size`.
///
/// # Panics
///
/// Panics if the length check fails, or for the panics listed on
/// [`encode_terrain_from_fn`].
pub fn encode_terrain(
    elevations: &[f32],
    grid_size: u32,
    bounds: &TileBounds,
    options: &TerrainOptions,
) -> Vec<u8> {
    assert_grid_len(elevations.len(), grid_size);
    let gs = grid_size as usize;
    encode_terrain_from_fn(
        grid_size,
        bounds,
        |x, y| elevations[y as usize * gs + x as usize] as f64,
        options,
    )
}

/// Like [`encode_terrain`], but streams the encoded bytes to a writer.
///
/// # Panics
///
/// Same panics as [`encode_terrain`].
pub fn encode_terrain_to<W: Write>(
    elevations: &[f32],
    grid_size: u32,
    bounds: &TileBounds,
    options: &TerrainOptions,
    writer: W,
) -> io::Result<()> {
    assert_grid_len(elevations.len(), grid_size);
    let gs = grid_size as usize;
    encode_terrain_from_fn_to(
        grid_size,
        bounds,
        |x, y| elevations[y as usize * gs + x as usize] as f64,
        options,
        writer,
    )
}

fn assert_grid_len(len: usize, grid_size: u32) {
    let expected = (grid_size as usize) * (grid_size as usize);
    assert_eq!(
        len, expected,
        "elevations length mismatch: expected {expected} ({grid_size}×{grid_size}), got {len}"
    );
}

/// Run martini, quantise the mesh, build the header + extensions, and return
/// a ready-to-encode [`QuantizedMeshEncoder`] alongside its [`EncodeOptions`].
fn build<F>(
    grid_size: u32,
    bounds: &TileBounds,
    get_height: F,
    options: &TerrainOptions,
) -> (QuantizedMeshEncoder, EncodeOptions)
where
    F: Fn(u32, u32) -> f64,
{
    if let NormalMode::BufferedGradient(buf) = &options.normals {
        assert_eq!(
            buf.tile_grid_size, grid_size,
            "BufferedGradient tile_grid_size ({}) must equal encode grid_size ({grid_size})",
            buf.tile_grid_size
        );
    }

    let mut martini = Martini::new(grid_size);
    let max = (grid_size - 1) as f64;
    let tile = martini.create_terrain(|x, y| get_height(x as u32, y as u32));

    // Hijack the UV transform to keep martini's `(u, v)` and re-sample the
    // height at the grid vertex. Martini computes `u = x/max` and
    // `v = 1 - y/max`, both exact for grid points, so the inverse recovers
    // the integer grid coordinate without drift.
    let (positions, indices, _uvs) =
        tile.construct_mesh(&mut martini, options.max_error, &mut |(u, v)| {
            let gx = (u * max).round();
            let gy = ((1.0 - v) * max).round();
            (u, v, get_height(gx as u32, gy as u32))
        });

    let vertex_count = positions.len() / 3;

    // Height range over the mesh vertices — i.e. exactly the heights we
    // quantise and store. A flat tile collapses to a zero span.
    let mut min_h = f64::INFINITY;
    let mut max_h = f64::NEG_INFINITY;
    for i in 0..vertex_count {
        let h = positions[i * 3 + 2] as f64;
        min_h = min_h.min(h);
        max_h = max_h.max(h);
    }
    if vertex_count == 0 {
        min_h = 0.0;
        max_h = 0.0;
    }
    let height_span = max_h - min_h;

    // Quantise (u, v, height) → 0..=32767.
    let quant_max = QUANTIZED_MAX as f64;
    let mut vertices = QuantizedVertices::with_capacity(vertex_count);
    for i in 0..vertex_count {
        let u = positions[i * 3] as f64;
        let v = positions[i * 3 + 1] as f64;
        let h = positions[i * 3 + 2] as f64;
        let uq = (u * quant_max).round().clamp(0.0, quant_max) as u16;
        let vq = (v * quant_max).round().clamp(0.0, quant_max) as u16;
        let hq = if height_span > 0.0 {
            (((h - min_h) / height_span) * quant_max)
                .round()
                .clamp(0.0, quant_max) as u16
        } else {
            0
        };
        vertices.push(uq, vq, hq);
    }

    let edge_indices = EdgeIndices::from_vertices(&vertices);

    // Feed the mesh vertices (geodetic) to the header so the horizon
    // occlusion point is as tight as possible.
    let lon_span = bounds.east - bounds.west;
    let lat_span = bounds.north - bounds.south;
    let geodetic = (0..vertex_count).map(|i| {
        let u = positions[i * 3] as f64;
        let v = positions[i * 3 + 1] as f64;
        let h = positions[i * 3 + 2] as f64;
        [bounds.west + u * lon_span, bounds.south + v * lat_span, h]
    });
    let header = QuantizedMeshHeader::from_bounds_with_vertices_iter(
        bounds,
        min_h as f32,
        max_h as f32,
        geodetic,
    );

    let normals = match &options.normals {
        NormalMode::None => None,
        NormalMode::FaceNormals => Some(face_normals(&vertices, &indices, bounds, min_h, max_h)),
        NormalMode::BufferedGradient(buf) => {
            Some(buffered_gradient_normals(&vertices, bounds, buf))
        }
    };

    let encode_opts = EncodeOptions {
        include_normals: normals.is_some(),
        normals,
        include_water_mask: options.water_mask.is_some(),
        water_mask: options.water_mask.clone(),
        include_metadata: options.metadata.is_some(),
        metadata: options.metadata.clone(),
        compression_level: options.compression_level,
    };

    let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);
    (encoder, encode_opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quantized_mesh::DecodedMesh;

    fn bumpy(x: u32, y: u32) -> f64 {
        ((x as f64) / 8.0).sin() * 50.0 + ((y as f64) / 8.0).cos() * 30.0
    }

    #[test]
    fn flat_tile_roundtrips_to_two_triangles() {
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let bytes = encode_terrain_from_fn(
            65,
            &bounds,
            |_, _| 0.0,
            &TerrainOptions {
                max_error: 0.0,
                compression_level: 0,
                ..Default::default()
            },
        );

        let mesh = DecodedMesh::decode(&bytes).expect("decode");
        // Flat terrain with zero error → just the 2 corner triangles.
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.header.min_height, 0.0);
        assert_eq!(mesh.header.max_height, 0.0);
        // All four corners present, heights all quantise to 0.
        assert!(mesh.vertices.height.iter().all(|&h| h == 0));
    }

    #[test]
    fn default_options_gzip_compress() {
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let bytes = encode_terrain_from_fn(65, &bounds, bumpy, &TerrainOptions::default());
        assert_eq!(&bytes[0..2], &[0x1f, 0x8b]); // gzip magic
    }

    #[test]
    fn height_range_matches_decoded_extremes() {
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let bytes = encode_terrain_from_fn(
            129,
            &bounds,
            bumpy,
            &TerrainOptions {
                max_error: 0.5,
                compression_level: 0,
                ..Default::default()
            },
        );
        let mesh = DecodedMesh::decode(&bytes).expect("decode");

        // The lowest mesh vertex must quantise to 0 and the highest to
        // QUANTIZED_MAX (the encoded range is defined by the header extremes).
        assert_eq!(*mesh.vertices.height.iter().min().unwrap(), 0);
        assert_eq!(*mesh.vertices.height.iter().max().unwrap(), QUANTIZED_MAX);
        assert!(mesh.header.max_height > mesh.header.min_height);
    }

    #[test]
    fn slice_and_closure_agree() {
        let grid_size = 65u32;
        let gs = grid_size as usize;
        let elevations: Vec<f32> = (0..gs * gs)
            .map(|i| bumpy((i % gs) as u32, (i / gs) as u32) as f32)
            .collect();
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let opts = TerrainOptions {
            max_error: 1.0,
            compression_level: 0,
            ..Default::default()
        };

        let from_slice = encode_terrain(&elevations, grid_size, &bounds, &opts);
        let from_fn = encode_terrain_from_fn(
            grid_size,
            &bounds,
            |x, y| elevations[y as usize * gs + x as usize] as f64,
            &opts,
        );
        assert_eq!(from_slice, from_fn);
    }

    #[test]
    fn writer_form_matches_vec_form() {
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let opts = TerrainOptions {
            max_error: 1.0,
            compression_level: 6,
            ..Default::default()
        };
        let vec_form = encode_terrain_from_fn(129, &bounds, bumpy, &opts);

        let mut writer_form = Vec::new();
        encode_terrain_from_fn_to(129, &bounds, bumpy, &opts, &mut writer_form).unwrap();
        assert_eq!(vec_form, writer_form);
    }

    #[test]
    fn face_normals_are_emitted_and_unit_length() {
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let bytes = encode_terrain_from_fn(
            65,
            &bounds,
            bumpy,
            &TerrainOptions {
                max_error: 1.0,
                compression_level: 0,
                normals: NormalMode::FaceNormals,
                ..Default::default()
            },
        );
        let mesh = DecodedMesh::decode(&bytes).expect("decode");
        let normals = mesh.extensions.normals.expect("normals present");
        assert_eq!(normals.len(), mesh.vertices.len());
        for n in &normals {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            // Oct-encoding is lossy, so allow a little slack around unit length.
            assert!(
                (len - 1.0).abs() < 0.05,
                "normal not ~unit: {n:?} (len {len})"
            );
        }
    }

    #[test]
    fn buffered_gradient_normals_are_emitted() {
        let grid_size = 65u32;
        let buffer = 1u32;
        let full = (grid_size + 2 * buffer) as usize;
        // Buffered grid sampling the same bumpy field, including the halo.
        let mut buffered = Vec::with_capacity(full * full);
        for j in 0..full {
            for i in 0..full {
                let x = i as i64 - buffer as i64;
                let y = j as i64 - buffer as i64;
                buffered.push(bumpy(x.max(0) as u32, y.max(0) as u32));
            }
        }
        let buffered = BufferedElevations::new(buffered, grid_size, buffer);

        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        let bytes = encode_terrain_from_fn(
            grid_size,
            &bounds,
            bumpy,
            &TerrainOptions {
                max_error: 1.0,
                compression_level: 0,
                normals: NormalMode::BufferedGradient(buffered),
                ..Default::default()
            },
        );
        let mesh = DecodedMesh::decode(&bytes).expect("decode");
        let normals = mesh.extensions.normals.expect("normals present");
        assert_eq!(normals.len(), mesh.vertices.len());
    }

    #[test]
    #[should_panic(expected = "elevations length mismatch")]
    fn slice_length_mismatch_panics() {
        let bounds = TileBounds::new(139.0, 35.0, 139.01, 35.01);
        encode_terrain(&[0.0f32; 10], 65, &bounds, &TerrainOptions::default());
    }
}
