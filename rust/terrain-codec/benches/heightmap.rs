use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use terrain_codec::heightmap::{mapbox, terrarium};

fn make_elevations(w: usize, h: usize) -> Vec<f32> {
    (0..(w * h))
        .map(|i| {
            let x = (i % w) as f32;
            let y = (i / w) as f32;
            (x * 0.05).sin() * 800.0 + (y * 0.03).cos() * 600.0 + 1200.0
        })
        .collect()
}

fn bench_heightmap(c: &mut Criterion) {
    let (w, h) = (512usize, 512usize);
    let elev = make_elevations(w, h);
    let n = (w * h) as u64;

    let terr_rgb = terrarium::encode(&elev, w as u32, h as u32);
    let mbox_rgb = mapbox::encode(&elev, w as u32, h as u32);

    let mut out_rgb = vec![0u8; w * h * 3];
    let mut out_elev = vec![0f32; w * h];

    let mut group = c.benchmark_group("heightmap");
    group.throughput(Throughput::Elements(n));

    group.bench_function("terrarium/encode", |b| {
        b.iter(|| terrarium::encode_into(black_box(&elev), black_box(&mut out_rgb)));
    });
    group.bench_function("terrarium/decode", |b| {
        b.iter(|| terrarium::decode_into(black_box(&terr_rgb), black_box(&mut out_elev)));
    });
    group.bench_function("mapbox/encode", |b| {
        b.iter(|| mapbox::encode_into(black_box(&elev), black_box(&mut out_rgb)));
    });
    group.bench_function("mapbox/decode", |b| {
        b.iter(|| mapbox::decode_into(black_box(&mbox_rgb), black_box(&mut out_elev)));
    });

    group.finish();
}

criterion_group!(benches, bench_heightmap);
criterion_main!(benches);
