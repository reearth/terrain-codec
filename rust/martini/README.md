# martini

[![Crates.io](https://img.shields.io/crates/v/martini.svg)](https://crates.io/crates/martini)
[![Docs.rs](https://docs.rs/martini/badge.svg)](https://docs.rs/martini)
[![Rust CI](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml/badge.svg)](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

RTIN (Right-Triangulated Irregular Network) mesh generation for terrain data in Rust.

Based on the [Martini algorithm](https://github.com/mapbox/martini) by Mapbox.

## Features

- Fast terrain mesh generation from heightmap data
- Level-of-detail control via error threshold
- Pre-computed triangle coordinates for efficient reuse
- Outputs vertices, indices, and UV coordinates

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
martini = "0.1"
```

## Usage

```rust
use martini::Martini;

// Create a Martini instance for a 257x257 grid (2^8 + 1)
let mut martini = Martini::new(257);

// Create terrain from height data
// Height data should be a 257x257 grid
let heightmap: Vec<f64> = vec![0.0; 257 * 257];
let tile = martini.create_terrain(|x, y| heightmap[y * 257 + x]);

// Generate mesh with maximum error threshold
// Lower error = more triangles = higher detail
let (vertices, indices, uvs) = tile.construct_mesh(
    &mut martini,
    10.0,  // max error in height units
    &mut |(u, v)| {
        // Transform UV (0-1) to world coordinates
        let x = u * 1000.0;  // world X
        let y = v * 1000.0;  // world Y
        let z = heightmap[(v * 256.0) as usize * 257 + (u * 256.0) as usize];
        (x, y, z)
    },
);

println!("Generated {} vertices, {} triangles",
    vertices.len() / 3,
    indices.len() / 3);
```

## Grid Size

The grid size must be `2^n + 1`:
- 3, 5, 9, 17, 33, 65, 129, 257, 513, 1025, ...

Common sizes:
- **257** (256x256 tiles) - Good for web terrain
- **513** (512x512 tiles) - Higher detail
- **1025** (1024x1024 tiles) - Very high detail

## Error Threshold

The `max_error` parameter controls mesh detail:

| Error | Result |
|-------|--------|
| High (100+) | Fewer triangles, lower quality |
| Medium (10) | Balanced quality/performance |
| Low (1) | More triangles, higher quality |
| 0 | Maximum detail (may be slow) |

## Output Format

- **vertices**: `Vec<f32>` - Flat array `[x, y, z, x, y, z, ...]`
- **indices**: `Vec<u32>` - Triangle indices
- **uvs**: `Vec<f32>` - Flat array `[u, v, u, v, ...]` (0-1 range)

## Reusing Martini Instance

The `Martini` struct pre-computes triangle coordinates. Reuse it for multiple tiles of the same size:

```rust
let mut martini = Martini::new(257);

// Process multiple tiles
for heightmap in heightmaps {
    let tile = martini.create_terrain(|x, y| heightmap[y * 257 + x]);
    let mesh = tile.construct_mesh(&mut martini, 10.0, &mut transform);
}
```

## Algorithm

The Martini algorithm uses a right-triangulated irregular network (RTIN) to create adaptive terrain meshes:

1. Start with two triangles covering the entire tile
2. For each triangle, compute the error at the midpoint of its longest edge
3. If error exceeds threshold, split the triangle into two smaller triangles
4. Repeat until all triangles meet the error threshold

This produces meshes that adapt to terrain complexity - flat areas get fewer triangles, complex areas get more.

## References

- [mapbox/martini](https://github.com/mapbox/martini) - Original JavaScript implementation
- [Right-Triangulated Irregular Networks](https://www.cs.ubc.ca/~will/papers/rtin.pdf) - Paper by Will Evans et al.

## License

MIT OR Apache-2.0
