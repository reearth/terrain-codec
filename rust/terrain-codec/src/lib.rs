//! Terrain processing utilities for 3D tile generation.
//!
//! This crate ties together [`martini`] (RTIN mesh generation) and
//! [`quantized_mesh`] (Cesium quantized-mesh-1.0 encode/decode) and adds
//! higher-level utilities — most notably **seamless vertex-normal
//! computation from a halo-extended DEM grid**, which keeps shading
//! continuous across tile boundaries.
//!
//! # Re-exports
//!
//! The two underlying crates are re-exported as modules, so downstream
//! code can use them through this single dependency:
//!
//! ```no_run
//! use terrain_codec::{martini, quantized_mesh};
//! ```
//!
//! # Normals
//!
//! See [`normals`] for the seam-free buffered-gradient normal algorithm
//! and a simpler per-tile face-normal accumulator.
//!
//! # Heightmap codecs
//!
//! See [`heightmap`] for Terrarium / Mapbox Terrain-RGB / GSI 地理院標高タイル
//! `encode` ⇄ `decode` pairs.
//!
//! # One-shot heightmap → `.terrain`
//!
//! See [`terrain`] for [`encode_terrain`](terrain::encode_terrain) and
//! friends, which run martini + quantized-mesh end-to-end (mesh,
//! quantise, header, normals, encode) in a single call.
//!
//! # Web-mercator → geodetic reprojection
//!
//! See [`mercator`] for [`MercatorDem`](mercator::MercatorDem), which
//! resamples web-mercator (XYZ) DEM tiles — Terrarium / Mapbox Terrain-RGB
//! and friends — onto the geodetic grid that [`terrain`] consumes.

pub use martini;
pub use quantized_mesh;

pub mod heightmap;
pub mod layer_json;
pub mod mercator;
pub mod normals;
pub mod terrain;
pub mod tile_coords;
