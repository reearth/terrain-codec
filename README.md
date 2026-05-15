# terrain-codec

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

RTIN (Right-Triangulated Irregular Network) mesh generation from heightmaps.

Based on the [Martini algorithm](https://github.com/mapbox/martini) by Mapbox.

### quantized-mesh

Encoder and decoder for [Cesium quantized-mesh-1.0](https://github.com/CesiumGS/quantized-mesh) terrain format.

## Typical Workflow

```
Heightmap Data (GeoTIFF, PNG, etc.)
         │
         ▼
    ┌─────────┐
    │ martini │  Generate adaptive mesh from heightmap
    └────┬────┘
         │
         ▼
  Vertices, Indices, UVs
         │
         ▼
┌────────────────┐
│ quantized-mesh │  Encode to Cesium terrain format
└───────┬────────┘
        │
        ▼
   .terrain file (quantized-mesh-1.0)
```

## License

MIT OR Apache-2.0
