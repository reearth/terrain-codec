# terrain-codec

[![Rust CI](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml/badge.svg)](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Terrain processing libraries for 3D tile generation.

## Overview

This repository provides libraries for processing terrain data into formats suitable for 3D globe applications like CesiumJS.

### Features

- **Mesh Generation** - Generate adaptive meshes from heightmap data using the RTIN algorithm
- **Quantized Mesh** - Encode/decode Cesium quantized-mesh-1.0 terrain format
- **Coordinate Transformations** - Convert between geodetic and ECEF coordinates

## Implementations

| Language | Path | Status |
|----------|------|--------|
| Rust | [rust/](./rust) | Available |
| JavaScript | js/ | Planned |
| Go | go/ | Planned |

## Libraries

### martini

[![Crates.io](https://img.shields.io/crates/v/martini-rs.svg)](https://crates.io/crates/martini-rs)
[![Docs.rs](https://docs.rs/martini-rs/badge.svg)](https://docs.rs/martini-rs)

RTIN (Right-Triangulated Irregular Network) mesh generation from heightmaps.
Based on the [Martini algorithm](https://github.com/mapbox/martini) by Mapbox.

- Source: [`rust/martini/`](./rust/martini)
- Published on crates.io as `martini-rs` (imported as `martini`)

### quantized-mesh

[![Crates.io](https://img.shields.io/crates/v/quantized-mesh.svg)](https://crates.io/crates/quantized-mesh)
[![Docs.rs](https://docs.rs/quantized-mesh/badge.svg)](https://docs.rs/quantized-mesh)

Encoder and decoder for [Cesium quantized-mesh-1.0](https://github.com/CesiumGS/quantized-mesh) terrain format.

- Source: [`rust/quantized-mesh/`](./rust/quantized-mesh)

### terrain-codec

[![Crates.io](https://img.shields.io/crates/v/terrain-codec.svg)](https://crates.io/crates/terrain-codec)
[![Docs.rs](https://docs.rs/terrain-codec/badge.svg)](https://docs.rs/terrain-codec)

Higher-level utilities — re-exports `martini` + `quantized_mesh`, plus
seamless DEM-gradient vertex normals (eliminates tile-boundary shading
seams) and RGB heightmap codecs (Terrarium / Mapbox Terrain-RGB / GSI).

- Source: [`rust/terrain-codec/`](./rust/terrain-codec)

## Typical Workflow

1. **Load** a heightmap (GeoTIFF, PNG, etc.)
2. **Mesh** it with `martini` → vertices, indices, UVs
3. **Encode** with `quantized-mesh` → `.terrain` file (quantized-mesh-1.0)

## License

MIT OR Apache-2.0
