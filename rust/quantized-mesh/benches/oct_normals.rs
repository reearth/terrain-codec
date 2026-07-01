use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use quantized_mesh::{iter_oct_normals, oct_decode_normal, oct_encode_normal};

/// Deterministic pseudo-random-ish unit normals covering both hemispheres.
fn make_normals(n: usize) -> Vec<[f32; 3]> {
    (0..n)
        .map(|i| {
            let a = (i as f32) * 0.618_034;
            let b = (i as f32) * 0.159_154_94;
            let x = a.sin() * 0.9;
            let y = b.cos() * 0.9;
            let z = 1.0 - x.abs() - y.abs(); // may be negative → lower hemisphere
            let len = (x * x + y * y + z * z).sqrt().max(1e-6);
            [x / len, y / len, z / len]
        })
        .collect()
}

fn bench_oct_normals(c: &mut Criterion) {
    // 257x257 vertex grid worth of normals — a large tile.
    let n = 257 * 257;
    let normals = make_normals(n);

    let mut encoded = vec![0u8; n * 2];
    for (nrm, o) in normals.iter().zip(encoded.chunks_exact_mut(2)) {
        o.copy_from_slice(&oct_encode_normal(*nrm));
    }
    let mut decoded = vec![[0.0f32; 3]; n];

    let mut group = c.benchmark_group("oct_normals");
    group.throughput(Throughput::Elements(n as u64));

    // Encode into a reused buffer — mirrors the real encoder's inner loop.
    group.bench_function("encode", |b| {
        let mut out = vec![0u8; n * 2];
        b.iter(|| {
            for (nrm, o) in black_box(&normals).iter().zip(out.chunks_exact_mut(2)) {
                o.copy_from_slice(&oct_encode_normal(*nrm));
            }
            black_box(&out);
        });
    });

    // Decode into a reused buffer — allocation-free.
    group.bench_function("decode", |b| {
        b.iter(|| {
            for (c, dst) in black_box(&encoded).chunks_exact(2).zip(decoded.iter_mut()) {
                *dst = oct_decode_normal([c[0], c[1]]);
            }
            black_box(&decoded);
        });
    });

    // Decode via the public lazy iterator, collecting an owned Vec — what
    // `ExtensionsView::to_owned` does today (includes the allocation).
    group.bench_function("decode_collect", |b| {
        b.iter(|| {
            let v: Vec<[f32; 3]> = iter_oct_normals(black_box(&encoded)).collect();
            black_box(v)
        });
    });

    group.finish();
}

criterion_group!(benches, bench_oct_normals);
criterion_main!(benches);
