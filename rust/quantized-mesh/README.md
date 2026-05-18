# quantized-mesh

[![Crates.io](https://img.shields.io/crates/v/quantized-mesh.svg)](https://crates.io/crates/quantized-mesh)
[![Docs.rs](https://docs.rs/quantized-mesh/badge.svg)](https://docs.rs/quantized-mesh)
[![Rust CI](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml/badge.svg)](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Encoder and decoder for [Cesium quantized-mesh-1.0](https://github.com/CesiumGS/quantized-mesh) terrain format in Rust.

## Features

- Full quantized-mesh-1.0 format support
- Encoding and decoding with `io::Read`/`io::Write` support
- **Zero-copy decoding** via [`QuantizedMeshView<'a>`] — borrows the input
  bytes and exposes vertex/index streams as lazy iterators, with no
  intermediate `Vec` allocations
- **Streaming encoder** — `encode_to_with_options` writes each section
  directly to a `Write` target via a small stack buffer, skipping the
  intermediate `Vec<u16>` / `Vec<u32>` encoded buffers
- Gzip compression/decompression (auto-detected)
- Extensions support:
  - Oct-encoded vertex normals
  - Water mask (borrowed `&[u8; 65536]` view, or owned)
  - Metadata (tile availability)
- Coordinate transformations (geodetic to/from ECEF)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
quantized-mesh = "0.1"
```

## Usage

### Encoding

```rust
use quantized_mesh::{
    QuantizedMeshEncoder, QuantizedMeshHeader, QuantizedVertices,
    EdgeIndices, EncodeOptions, TileBounds,
};

// Create header from tile bounds
let bounds = TileBounds::new(-180.0, -90.0, 0.0, 90.0);
let header = QuantizedMeshHeader::from_bounds(&bounds, 0.0, 1000.0);

// Create vertices (quantized to 0-32767 range)
let vertices = QuantizedVertices {
    u: vec![0, 32767, 0, 32767],
    v: vec![0, 0, 32767, 32767],
    height: vec![0, 0, 0, 0],
};

// Triangle indices
let indices = vec![0, 1, 2, 1, 3, 2];

// Extract edge indices from vertices
let edge_indices = EdgeIndices::from_vertices(&vertices);

// Create encoder
let encoder = QuantizedMeshEncoder::new(header, vertices, indices, edge_indices);

// Encode to Vec<u8>
let data = encoder.encode();

// Or encode with options (compression, extensions)
let data = encoder.encode_with_options(&EncodeOptions {
    compression_level: 6, // gzip compression (0 = none, 1-9 = compression level)
    ..Default::default()
});

// Or encode directly to a writer (e.g., file)
use std::fs::File;
let file = File::create("tile.terrain").unwrap();
encoder.encode_to_with_options(file, &EncodeOptions {
    compression_level: 6,
    ..Default::default()
}).unwrap();
```

### Decoding

Two flavours are available — pick based on whether you need owned `Vec`s
or are OK with lazy borrowed iterators.

**Zero-copy view** (`QuantizedMeshView<'a>`): borrows the input bytes and
lazily decodes zigzag-delta vertex streams and high-water-mark indices,
allocating nothing for the bulk data.

```rust
use quantized_mesh::QuantizedMeshView;

let data: &[u8] = &[/* uncompressed terrain data */];
let view = QuantizedMeshView::parse(data).unwrap();

println!("vertices: {}", view.vertex_count);
for (u, (v, h)) in view.iter_u().zip(view.iter_v().zip(view.iter_height())) {
    // process each vertex on the fly...
}
for tri in view.indices.iter().collect::<Vec<_>>().chunks(3) {
    // triangle = [a, b, c]
}
```

**Owned mesh** (`DecodedMesh`): convenience wrapper that handles gzip
auto-detection and materialises every section into `Vec`s.

```rust
use quantized_mesh::DecodedMesh;

// Decode from byte slice (auto-detects gzip)
let data: &[u8] = &[/* terrain data */];
let mesh = DecodedMesh::decode(data).unwrap();

// Or decode from a reader (e.g., file)
use std::fs::File;
let file = File::open("tile.terrain").unwrap();
let mesh = DecodedMesh::decode_from(file).unwrap();

println!("Vertex count: {}", mesh.vertices.len());
println!("Triangle count: {}", mesh.indices.len() / 3);
println!("Height range: {} - {}", mesh.header.min_height, mesh.header.max_height);
```

### With Extensions

```rust
use quantized_mesh::{EncodeOptions, WaterMask, TileMetadata};

// Encode with vertex normals
let normals: Vec<[f32; 3]> = vec![[0.0, 0.0, 1.0]; vertex_count];

let options = EncodeOptions {
    compression_level: 6,
    include_normals: true,
    normals: Some(normals),
    include_water_mask: true,
    water_mask: Some(WaterMask::Uniform(0)), // 0 = all land, 255 = all water
    include_metadata: true,
    metadata: Some(TileMetadata::for_tile(x, y, zoom, max_zoom)),
};

let data = encoder.encode_with_options(&options);

// Decode and access extensions
let mesh = DecodedMesh::decode(&data).unwrap();
if let Some(normals) = mesh.extensions.normals {
    println!("Has {} normals", normals.len());
}
```

### Coordinate Transformations

```rust
use quantized_mesh::coords::{geodetic_to_ecef, ecef_to_geodetic};

// Convert longitude/latitude/height to ECEF
let ecef = geodetic_to_ecef(139.7, 35.7, 100.0); // Tokyo

// Convert ECEF back to geodetic
let (lon, lat, height) = ecef_to_geodetic(ecef[0], ecef[1], ecef[2]);
```

## Format Overview

The quantized-mesh format consists of:

1. **Header** (88 bytes): Tile center, height range, bounding sphere, horizon occlusion point
2. **Vertex Data**: Delta-encoded and zigzag-encoded u/v/height coordinates
3. **Index Data**: High-water mark encoded triangle indices
4. **Edge Indices**: Vertices on tile edges for seamless stitching
5. **Extensions** (optional): Normals, water mask, metadata

## License

MIT OR Apache-2.0
