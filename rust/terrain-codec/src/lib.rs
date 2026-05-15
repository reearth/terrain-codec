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
//! See [`normals`] for the seam-free halo-gradient normal algorithm and a
//! simpler per-tile face-normal accumulator.

pub use martini;
pub use quantized_mesh;

pub mod normals;
