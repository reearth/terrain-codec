use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use terrain_codec::mercator::MercatorDem;
use terrain_codec::quantized_mesh::TileBounds;
use terrain_codec::tile_coords::web_mercator;

/// Build a MercatorDem over a `tiles x tiles` block of synthetic elevation.
fn make_dem(zoom: u8, x0: u32, y0: u32, tiles: u32, tile_size: u32) -> MercatorDem {
    let w = (tiles * tile_size) as usize;
    let h = (tiles * tile_size) as usize;
    let elev: Vec<f32> = (0..(w * h))
        .map(|i| {
            let x = (i % w) as f32;
            let y = (i / w) as f32;
            (x * 0.02).sin() * 500.0 + (y * 0.017).cos() * 400.0 + 800.0
        })
        .collect();
    MercatorDem::new(zoom, x0, y0, tiles, tiles, tile_size, elev)
}

fn bench_mercator(c: &mut Criterion) {
    let zoom = 12u8;
    let (x0, y0) = (3637u32, 1612u32);
    let tiles = 4u32;
    let tile_size = 256u32;
    let dem = make_dem(zoom, x0, y0, tiles, tile_size);

    // Target geodetic tile = a tile well inside the block (offset by 1 so the
    // buffered halo still lands inside the fetched area).
    let (w, s, e, n) = web_mercator::tile_to_bounds(zoom, x0 + 1, y0 + 1);
    let bounds = TileBounds::new(w, s, e, n);

    let grid_size = 257u32;
    let cells = (grid_size * grid_size) as u64;

    let mut group = c.benchmark_group("mercator");
    group.throughput(Throughput::Elements(cells));

    group.bench_function("geodetic_grid/257", |b| {
        b.iter(|| black_box(dem.geodetic_grid(black_box(&bounds), grid_size)));
    });

    let buffer = 4u32;
    group.bench_function("buffered_geodetic/257+4", |b| {
        b.iter(|| black_box(dem.buffered_geodetic(black_box(&bounds), grid_size, buffer)));
    });

    group.finish();
}

criterion_group!(benches, bench_mercator);
criterion_main!(benches);
