# terrain-codec

[![Crates.io](https://img.shields.io/crates/v/terrain-codec.svg)](https://crates.io/crates/terrain-codec)
[![Docs.rs](https://docs.rs/terrain-codec/badge.svg)](https://docs.rs/terrain-codec)
[![Rust CI](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml/badge.svg)](https://github.com/reearth/terrain-codec/actions/workflows/rust.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Terrain processing utilities for 3D tile generation in Rust.

Ties together [`martini-rs`](https://crates.io/crates/martini-rs) (RTIN
mesh generation) and [`quantized-mesh`](https://crates.io/crates/quantized-mesh)
(Cesium quantized-mesh-1.0 encode/decode), and adds higher-level utilities
that don't fit cleanly in either — most notably **seamless vertex normals
computed from a halo-extended DEM grid**, which keeps shading continuous
across tile boundaries.

## Installation

```toml
[dependencies]
terrain-codec = "0.1"
```

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
- **`halo_gradient_normals`** — sample a halo-extended DEM grid that
  covers cells *beyond* the tile on every side. Adjacent tiles read the
  same samples at any shared physical position, so seam vertices get
  identical normals on both sides and lighting is continuous.

```rust
use terrain_codec::normals::{HaloElevations, halo_gradient_normals};

let halo = HaloElevations::new(
    elevations_with_halo, // size: (tile_grid_size + 2*halo)²
    tile_grid_size,
    halo_cells,
);

let normals = halo_gradient_normals(&vertices, &bounds, &halo);
```

## Why halo normals?

Face-normal accumulation only sees triangles inside the current tile, so
the **same physical edge is shaded inconsistently from adjacent tiles**.
Gradient normals computed from a halo-extended DEM grid use the same
samples both tiles can see, so edge vertices get identical normals on
both sides.

The crate ships regression tests that:

1. Verify that a perfectly tilted plane produces the analytical ENU
   normal everywhere (within float tolerance).
2. Verify that two adjacent tiles sharing an east/west edge produce
   bit-identical normals at the seam vertices when both use the same DEM
   field.

## License

MIT OR Apache-2.0
