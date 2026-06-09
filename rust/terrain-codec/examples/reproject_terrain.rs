//! Real-data web-mercator → geodetic-TMS reprojection smoke test.
//!
//! Targets a geodetic TMS tile, reprojects the covering web-mercator
//! Terrarium DEM tiles (read from a directory of `z_x_y.png` files) onto its
//! geodetic grid, encodes a `.terrain`, and — if given a real production
//! tile for the same TMS coordinate — prints both for comparison.
//!
//! ```text
//! cargo run -p terrain-codec --example reproject_terrain --features png -- \
//!     <tms_z> <tms_x> <tms_y> <src_zoom> <tile_size> <png_dir> [real.terrain]
//! ```

use std::env;
use std::path::PathBuf;

use terrain_codec::heightmap::container::decode_image;
use terrain_codec::heightmap::terrarium;
use terrain_codec::mercator::MercatorDem;
use terrain_codec::quantized_mesh::{DecodedMesh, TileBounds};
use terrain_codec::terrain::{NormalMode, TerrainOptions, encode_terrain};
use terrain_codec::tile_coords::geodetic_tms;

fn main() {
    let mut a = env::args().skip(1);
    let tz: u8 = a.next().expect("tms_z").parse().unwrap();
    let tx: u32 = a.next().expect("tms_x").parse().unwrap();
    let ty: u32 = a.next().expect("tms_y").parse().unwrap();
    let src_zoom: u8 = a.next().expect("src_zoom").parse().unwrap();
    let tile_size: u32 = a.next().expect("tile_size").parse().unwrap();
    let png_dir = PathBuf::from(a.next().expect("png_dir"));
    let real = a.next();

    // Target geodetic TMS tile.
    let (w, s, e, n) = geodetic_tms::tile_to_bounds(tz, tx, ty);
    let bounds = TileBounds::new(w, s, e, n);
    let grid_size = tile_size + 1; // 2^n + 1
    println!("target geodetic TMS z{tz}/{tx}/{ty}  bounds = [{w:.5}, {s:.5}, {e:.5}, {n:.5}]");

    // Widen by ~one geodetic cell so the halo (buffer=1) stays in coverage.
    let cell_lon = (e - w) / (grid_size - 1) as f64;
    let cell_lat = (n - s) / (grid_size - 1) as f64;
    let (x0, y0, ntx, nty) = MercatorDem::tiles_covering(
        src_zoom,
        w - cell_lon,
        s - cell_lat,
        e + cell_lon,
        n + cell_lat,
    );
    println!(
        "covering web-mercator z{src_zoom} tiles: x {x0}..{}  y {y0}..{}  ({} tiles, {tile_size}px)",
        x0 + ntx - 1,
        y0 + nty - 1,
        ntx * nty
    );

    // Build the mercator DEM by reading each covering tile from disk.
    let dem = MercatorDem::from_tiles(src_zoom, x0, y0, ntx, nty, tile_size, |z, x, y| {
        let path = png_dir.join(format!("{z}_{x}_{y}.png"));
        let bytes = std::fs::read(&path).unwrap_or_else(|_| panic!("missing tile {path:?}"));
        let img = decode_image(&bytes).expect("decode png");
        assert_eq!(img.width, tile_size);
        assert_eq!(img.height, tile_size);
        terrarium::decode(&img.rgb, img.width, img.height)
    });

    // Reproject onto the geodetic grid + a halo grid for seamless normals.
    let grid = dem.geodetic_grid(&bounds, grid_size);
    let buffered = dem.buffered_geodetic(&bounds, grid_size, 1);

    let (gmin, gmax) = min_max(&grid);
    println!("reprojected geodetic grid {grid_size}×{grid_size}: {gmin:.1} .. {gmax:.1} m");

    let bytes = encode_terrain(
        &grid,
        grid_size,
        &bounds,
        &TerrainOptions {
            max_error: 4.0,
            compression_level: 6,
            normals: NormalMode::BufferedGradient(buffered),
            ..Default::default()
        },
    );
    let mesh = DecodedMesh::decode(&bytes).expect("decode our output");
    println!(
        "ours: {} bytes (gzip), {} verts, {} tris, height {:.1}..{:.1} m, normals={}",
        bytes.len(),
        mesh.vertices.len(),
        mesh.indices.len() / 3,
        mesh.header.min_height,
        mesh.header.max_height,
        mesh.extensions.normals.is_some(),
    );

    if let Some(path) = real {
        let raw = std::fs::read(&path).expect("read real .terrain");
        let rm = DecodedMesh::decode(&raw).expect("decode real");
        println!(
            "real: {} bytes, {} verts, {} tris, height {:.1}..{:.1} m, normals={}  ({})",
            raw.len(),
            rm.vertices.len(),
            rm.indices.len() / 3,
            rm.header.min_height,
            rm.header.max_height,
            rm.extensions.normals.is_some(),
            path,
        );
        println!(
            "note: height offset vs real is expected — real is EGM2008 geoid-blended, ours is raw Terrarium (ellipsoidal)."
        );
    }

    println!("\nOK ✅  web-mercator → geodetic-TMS reprojection produced valid .terrain");
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
