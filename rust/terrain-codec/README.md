# terrain-codec

[![Crates.io](https://img.shields.io/crates/v/terrain-codec.svg)](https://crates.io/crates/terrain-codec)
[![Docs.rs](https://docs.rs/terrain-codec/badge.svg)](https://docs.rs/terrain-codec)
[![Rust CI](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml/badge.svg)](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Terrain processing utilities for 3D tile generation in Rust.

Ties together [`martini`](https://crates.io/crates/martini) (RTIN
mesh generation) and [`quantized-mesh`](https://crates.io/crates/quantized-mesh)
(Cesium quantized-mesh-1.0 encode/decode), and adds higher-level utilities
that don't fit cleanly in either — most notably **seamless vertex normals
computed from a buffer-extended DEM grid**, which keeps shading continuous
across tile boundaries.

## Installation

```toml
[dependencies]
terrain-codec = "0.1"

# Optional: enable one or more image-container encoders for
# heightmap::container (each pulls in the `image` crate + its codec).
terrain-codec = { version = "0.1", features = ["png", "webp", "avif"] }
```

Feature flags:

| Feature | Adds |
|---------|------|
| `png`   | `heightmap::container::rgb_to_png` + PNG decoding in `decode_image` |
| `webp`  | `heightmap::container::rgb_to_webp` (lossless) + WebP decoding |
| `avif`  | `heightmap::container::rgb_to_avif` (encode-only via ravif) |

## What's inside

### Re-exports

```rust
use terrain_codec::{martini, quantized_mesh};
```

Both crates are re-exported as modules, so downstream code can use them
through a single dependency.

### `normals` — vertex-normal computation

Two strategies, both returning unit-length ECEF normals (Cesium's
convention for oct-encoded normals):

- **`face_normals`** — accumulate triangle face normals onto vertices.
  Simple, but produces a **visible seam at tile boundaries** because each
  tile only sees its own triangles.
- **`buffered_gradient_normals`** — sample a buffer-extended DEM grid
  that covers cells *beyond* the tile on every side. Adjacent tiles read
  the same samples at any shared physical position, so seam vertices get
  identical normals on both sides and lighting is continuous.

```rust
use terrain_codec::normals::{BufferedElevations, buffered_gradient_normals};

let buffered = BufferedElevations::new(
    elevations_with_buffer, // size: (tile_grid_size + 2*buffer)²
    tile_grid_size,
    buffer_cells,
);

let normals = buffered_gradient_normals(&vertices, &bounds, &buffered);
```

### `heightmap` — RGB tile codecs

Symmetric `encode`/`decode` pairs for the three common RGB elevation
tile formats:

- **`heightmap::terrarium`** — Mapzen / Tilezen / Stadia Terrarium.
- **`heightmap::mapbox`** — Mapbox Terrain-RGB.
- **`heightmap::gsi`** — GSI 地理院標高タイル (signed 24-bit, with NaN
  no-data sentinel).

All operate on raw `(R, G, B)` byte triplets, so they're agnostic to
the container. Enable one or more of the `png`, `webp`, `avif` cargo
features to wrap them via [`heightmap::container`](https://docs.rs/terrain-codec/latest/terrain_codec/heightmap/container/).
A runtime-dispatched `rgb_to_container(ContainerFormat, …)` is also
provided for when the format is picked dynamically.

Each format exposes per-pixel and bulk APIs:

```rust
use terrain_codec::heightmap::{terrarium, mapbox, gsi};

// Per-pixel (for streaming, hot loops, tests)
let rgb_px: [u8; 3] = terrarium::encode_pixel(123.45);
let h: f32 = terrarium::decode_pixel([0x80, 0x00, 0x00]); // 0.0

// Bulk (row-major width × height buffer)
let rgb = terrarium::encode(&elevations, width, height);
let decoded = terrarium::decode(&rgb, width, height);
```

## Why buffered normals?

Face-normal accumulation only sees triangles inside the current tile, so
the **same physical edge is shaded inconsistently from adjacent tiles**.
Gradient normals computed from a buffer-extended DEM grid use the same
samples both tiles can see, so edge vertices get identical normals on
both sides. This is the same trick raster pipelines use under names like
"padding" or "buffer cells".

The crate ships regression tests that:

1. Verify that a perfectly tilted plane produces the analytical ENU
   normal everywhere (within float tolerance).
2. Verify that two adjacent tiles sharing an east/west edge produce
   bit-identical normals at the seam vertices when both use the same DEM
   field.

## License

MIT OR Apache-2.0
