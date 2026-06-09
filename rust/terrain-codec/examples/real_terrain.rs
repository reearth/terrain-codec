//! Real-data smoke test for the `terrain::encode_terrain` pipeline.
//!
//! Feeds a real Terrarium elevation PNG (e.g. from terrain.reearth.land)
//! through heightmap-decode → `encode_terrain` → quantized-mesh decode, and
//! reports mesh stats plus the height round-trip error.
//!
//! Optionally also decodes a real quantized-mesh `.terrain` tile to confirm
//! the decoder handles production data.
//!
//! ```text
//! cargo run -p terrain-codec --example real_terrain --features png -- \
//!     /tmp/fuji.png 12 3626 1617 [/tmp/fuji.terrain]
//! ```

use std::env;

use terrain_codec::heightmap::container::decode_image;
use terrain_codec::heightmap::terrarium;
use terrain_codec::quantized_mesh::{DecodedMesh, QUANTIZED_MAX, TileBounds};
use terrain_codec::terrain::{NormalMode, TerrainOptions, encode_terrain};
use terrain_codec::tile_coords::web_mercator;

fn main() {
    let mut args = env::args().skip(1);
    let png_path = args
        .next()
        .expect("usage: real_terrain <png> <z> <x> <y> [terrain]");
    let z: u8 = args.next().expect("z").parse().unwrap();
    let x: u32 = args.next().expect("x").parse().unwrap();
    let y: u32 = args.next().expect("y").parse().unwrap();
    let real_terrain = args.next();

    // 1. Decode the Terrarium PNG → RGB → elevation grid (row-major N→S).
    let png_bytes = std::fs::read(&png_path).expect("read png");
    let img = decode_image(&png_bytes).expect("decode png");
    println!(
        "source PNG: {}×{} ({} bytes)",
        img.width,
        img.height,
        png_bytes.len()
    );
    let src = terrarium::decode(&img.rgb, img.width, img.height);

    let (smin, smax) = min_max(&src);
    println!("source DEM elevation range: {smin:.1} .. {smax:.1} m");

    // 2. Pad N×N → (N+1)×(N+1) so it's 2^n+1 for martini (edge-replicate the
    //    extra south row / east column — a real pipeline would pull these
    //    from the neighbour tiles, but edge-replicate is fine for a smoke test).
    let n = img.width;
    assert_eq!(img.width, img.height, "expected a square tile");
    let grid_size = n + 1;
    let (w, h) = (n as usize, n as usize);
    let gs = grid_size as usize;
    let mut grid = vec![0f32; gs * gs];
    for yy in 0..gs {
        for xx in 0..gs {
            let sx = xx.min(w - 1);
            let sy = yy.min(h - 1);
            grid[yy * gs + xx] = src[sy * w + sx];
        }
    }

    // 3. Tile bounds (Web-Mercator XYZ). The +1 post extends the footprint by
    //    ~one cell south/east; we ignore that sub-cell offset for the header.
    let (west, south, east, north) = web_mercator::tile_to_bounds(z, x, y);
    let bounds = TileBounds::new(west, south, east, north);
    println!(
        "tile z{z}/{x}/{y}  bounds = [{west:.5}, {south:.5}, {east:.5}, {north:.5}]  grid {grid_size}×{grid_size}"
    );

    // 4. Encode at a few error thresholds, compressed (default level 6).
    for &max_error in &[1.0_f64, 5.0, 20.0] {
        let opts = TerrainOptions {
            max_error,
            compression_level: 6,
            normals: NormalMode::FaceNormals,
            ..Default::default()
        };
        let bytes = encode_terrain(&grid, grid_size, &bounds, &opts);

        // also measure the uncompressed size
        let raw = encode_terrain(
            &grid,
            grid_size,
            &bounds,
            &TerrainOptions {
                compression_level: 0,
                ..opts.clone()
            },
        );

        let mesh = DecodedMesh::decode(&bytes).expect("decode our own output");
        let vc = mesh.vertices.len();
        let tc = mesh.indices.len() / 3;

        // 5. Height round-trip error: martini vertices sit exactly on grid
        //    posts, so dequantising should match the source grid to within
        //    the quantisation step (height_span / 32767).
        let span = (mesh.header.max_height - mesh.header.min_height) as f64;
        let step = span / QUANTIZED_MAX as f64;
        let mut max_err = 0.0f64;
        let mut sum_err = 0.0f64;
        for i in 0..vc {
            let u = mesh.vertices.u[i] as f64 / QUANTIZED_MAX as f64;
            let v = mesh.vertices.v[i] as f64 / QUANTIZED_MAX as f64;
            let gx = (u * (grid_size - 1) as f64).round() as usize;
            let gy = ((1.0 - v) * (grid_size - 1) as f64).round() as usize;
            let src_h = grid[gy * gs + gx] as f64;
            let deq_h = mesh.header.min_height as f64
                + mesh.vertices.height[i] as f64 / QUANTIZED_MAX as f64 * span;
            let e = (src_h - deq_h).abs();
            max_err = max_err.max(e);
            sum_err += e;
        }
        let mean_err = if vc > 0 { sum_err / vc as f64 } else { 0.0 };

        println!(
            "  max_error={max_error:>4} m | verts {vc:>6} tris {tc:>6} | \
height {:.1}..{:.1} m | gzip {:>6} B (raw {:>7} B, {:.1}×) | \
roundtrip err mean {mean_err:.3} m max {max_err:.3} m (q-step {step:.3} m)",
            mesh.header.min_height,
            mesh.header.max_height,
            bytes.len(),
            raw.len(),
            raw.len() as f64 / bytes.len() as f64,
        );

        assert!(
            max_err <= step + 1e-3,
            "roundtrip error {max_err} exceeds quantisation step {step}"
        );
    }

    // 6. Bonus: decode a real production quantized-mesh tile, if supplied.
    if let Some(path) = real_terrain {
        let raw = std::fs::read(&path).expect("read .terrain");
        let mesh = DecodedMesh::decode(&raw).expect("decode real quantized-mesh");
        println!(
            "\nreal {} ({} bytes): {} verts, {} tris, height {:.1}..{:.1} m, normals={}, water_mask={}, metadata={}",
            path,
            raw.len(),
            mesh.vertices.len(),
            mesh.indices.len() / 3,
            mesh.header.min_height,
            mesh.header.max_height,
            mesh.extensions.normals.is_some(),
            mesh.extensions.water_mask.is_some(),
            mesh.extensions.metadata.is_some(),
        );
    }

    println!("\nOK ✅  real-data pipeline produced valid, decodable .terrain");
}

fn min_max(v: &[f32]) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &x in v {
        lo = lo.min(x);
        hi = hi.max(x);
    }
    (lo, hi)
}
