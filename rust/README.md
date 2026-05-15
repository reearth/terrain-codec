# terrain-codec (Rust)

[![Rust CI](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml/badge.svg)](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml)

Rust implementation of terrain processing libraries.

## Crates

| crates.io name | Library name | Description |
|----------------|--------------|-------------|
| [`martini-rs`](./martini) | `martini` | RTIN mesh generation from heightmaps |
| [`quantized-mesh`](./quantized-mesh) | `quantized_mesh` | Cesium quantized-mesh-1.0 encoder/decoder |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
martini-rs = "0.1"
quantized-mesh = "0.1"
```

## Quick Start

```rust
use martini::Martini;
use quantized_mesh::{
    QuantizedMeshEncoder, QuantizedMeshHeader, QuantizedVertices,
    EdgeIndices, EncodeOptions, TileBounds, QUANTIZED_MAX,
};

// 1. Generate mesh from heightmap
let mut martini = Martini::new(257);
let heightmap: Vec<f64> = load_heightmap(); // Your heightmap data

let tile = martini.create_terrain(|x, y| heightmap[y * 257 + x]);
let (vertices, indices, _uvs) = tile.construct_mesh(
    &mut martini,
    10.0,
    &mut |(u, v)| (u, v, heightmap[(v * 256.0) as usize * 257 + (u * 256.0) as usize]),
);

// 2. Convert to quantized-mesh format
let bounds = TileBounds::new(west, south, east, north);
let header = QuantizedMeshHeader::from_bounds(&bounds, min_height, max_height);

// Quantize vertices to 0-32767 range
let quantized = QuantizedVertices {
    u: vertices.iter().step_by(3).map(|&x| (x * QUANTIZED_MAX as f32) as u16).collect(),
    v: vertices.iter().skip(1).step_by(3).map(|&y| (y * QUANTIZED_MAX as f32) as u16).collect(),
    height: vertices.iter().skip(2).step_by(3).map(|&z| /* quantize height */).collect(),
};

let edge_indices = EdgeIndices::from_vertices(&quantized);
let encoder = QuantizedMeshEncoder::new(header, quantized, indices, edge_indices);

// 3. Write to file
let mut file = File::create("0/0/0.terrain")?;
encoder.encode_to_with_options(&mut file, &EncodeOptions {
    compression_level: 6,
    ..Default::default()
})?;
```

## Building

```bash
cargo build --release
```

## Testing

```bash
cargo test
```

## License

MIT OR Apache-2.0
