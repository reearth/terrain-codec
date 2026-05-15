//! Encoder and decoder for Cesium terrain tile formats.
//!
//! The primary format is **quantized-mesh-1.0** (3D triangulated mesh tiles
//! used by CesiumJS for efficient 3D terrain streaming). The legacy
//! **heightmap-1.0** format is also supported via the [`heightmap_1`]
//! module.
//!
//! # References
//!
//! - quantized-mesh-1.0 specification:
//!   <https://github.com/CesiumGS/quantized-mesh>
//! - heightmap-1.0 specification:
//!   <https://github.com/CesiumGS/cesium/wiki/heightmap-1.0>
//!
//! # Example
//!
//! ```
//! use quantized_mesh::{
//!     QuantizedMeshEncoder, QuantizedMeshHeader, QuantizedVertices,
//!     EdgeIndices, TileBounds,
//! };
//!
//! // Create header with tile metadata
//! let header = QuantizedMeshHeader {
//!     center: [0.0, 0.0, 6378137.0],
//!     min_height: 0.0,
//!     max_height: 100.0,
//!     bounding_sphere_center: [0.0, 0.0, 6378137.0],
//!     bounding_sphere_radius: 1000.0,
//!     horizon_occlusion_point: [0.0, 0.0, 1.0],
//! };
//!
//! // Create vertices (simple 2-triangle mesh)
//! let vertices = QuantizedVertices {
//!     u: vec![0, 32767, 0, 32767],
//!     v: vec![0, 0, 32767, 32767],
//!     height: vec![0, 0, 0, 0],
//! };
//!
//! // Triangle indices (2 triangles)
//! let indices = vec![0, 1, 2, 1, 3, 2];
//!
//! // Edge indices
//! let edge_indices = EdgeIndices {
//!     west: vec![0, 2],
//!     south: vec![0, 1],
//!     east: vec![1, 3],
//!     north: vec![2, 3],
//! };
//!
//! // Encode to quantized-mesh format
//! let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);
//! let data = encoder.encode();
//! ```

pub mod coords;
mod decoding;
mod encoding;
mod header;
pub mod heightmap_1;
mod types;

pub use decoding::*;
pub use encoding::*;
pub use header::*;
pub use types::*;
