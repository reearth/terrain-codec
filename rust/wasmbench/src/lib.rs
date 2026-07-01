//! wasm micro-benchmark kernels: scalar vs explicit `wide` SIMD.
//!
//! Each `extern "C"` export runs one kernel `iters` times and returns a
//! checksum (so the optimiser can't delete the work). A node driver times the
//! calls under WebAssembly. Data is generated once inside each export; with a
//! large `iters` that one-off setup is negligible.

use core::hint::black_box;

use quantized_mesh::{TileBounds, oct_decode_normal, oct_encode_normal, oct_encode_normals_into};
use terrain_codec::heightmap::terrarium;
use terrain_codec::mercator::MercatorDem;
use terrain_codec::tile_coords::web_mercator;
use wide::{CmpGe, CmpGt, CmpLt, CmpNe, f32x4};

// ---------------------------------------------------------------------------
// Shared data generators
// ---------------------------------------------------------------------------

const GRID: usize = 257;
const RASTER: usize = 512;

fn make_dem() -> (MercatorDem, TileBounds) {
    let (zoom, x0, y0, tiles, ts) = (12u8, 3637u32, 1612u32, 4u32, 256u32);
    let (w, h) = ((tiles * ts) as usize, (tiles * ts) as usize);
    let elev: Vec<f32> = (0..(w * h))
        .map(|i| {
            let x = (i % w) as f32;
            let y = (i / w) as f32;
            (x * 0.02).sin() * 500.0 + (y * 0.017).cos() * 400.0 + 800.0
        })
        .collect();
    let dem = MercatorDem::new(zoom, x0, y0, tiles, tiles, ts, elev);
    let (west, south, east, north) = web_mercator::tile_to_bounds(zoom, x0 + 1, y0 + 1);
    (dem, TileBounds::new(west, south, east, north))
}

fn make_elevations(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = (i % RASTER) as f32;
            let y = (i / RASTER) as f32;
            (x * 0.05).sin() * 800.0 + (y * 0.03).cos() * 600.0 + 1200.0
        })
        .collect()
}

fn make_normals(n: usize) -> Vec<[f32; 3]> {
    (0..n)
        .map(|i| {
            let a = (i as f32) * 0.618_034;
            let b = (i as f32) * 0.159_154_94;
            let x = a.sin() * 0.9;
            let y = b.cos() * 0.9;
            let z = 1.0 - x.abs() - y.abs();
            let len = (x * x + y * y + z * z).sqrt().max(1e-6);
            [x / len, y / len, z / len]
        })
        .collect()
}

fn sum(v: &[f32]) -> f64 {
    v.iter().map(|&x| x as f64).sum()
}

// ---------------------------------------------------------------------------
// Mercator: old (per-pixel transcendental) vs new (hoisted, the shipped code)
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn merc_old(iters: u32) -> f64 {
    let (dem, b) = make_dem();
    let gs = GRID;
    let denom = (gs - 1) as f64;
    let (lon_span, lat_span) = (b.east - b.west, b.north - b.south);
    let mut grid = vec![0f32; gs * gs];
    let mut acc = 0.0;
    for _ in 0..iters {
        for j in 0..gs {
            let lat = b.north - (j as f64 / denom) * lat_span;
            for i in 0..gs {
                let lon = b.west + (i as f64 / denom) * lon_span;
                // Per-pixel `sample` recomputes the tan().asinh() every call.
                grid[j * gs + i] = dem.sample(lon, lat);
            }
        }
        acc += sum(black_box(&grid));
    }
    acc
}

#[unsafe(no_mangle)]
pub extern "C" fn merc_new(iters: u32) -> f64 {
    let (dem, b) = make_dem();
    let mut acc = 0.0;
    for _ in 0..iters {
        let grid = dem.geodetic_grid(&b, GRID as u32);
        acc += sum(black_box(&grid));
    }
    acc
}

// ---------------------------------------------------------------------------
// Heightmap Terrarium: scalar (shipped) vs explicit SIMD (reverted variant)
// ---------------------------------------------------------------------------

const RGB24_MAX: f32 = (1u32 << 24) as f32 - 1.0;

fn encode_simd(elevations: &[f32], out: &mut [u8], bias: f32, scale: f32) {
    let (bias4, scale4) = (f32x4::splat(bias), f32x4::splat(scale));
    let (zero, max4) = (f32x4::splat(0.0), f32x4::splat(RGB24_MAX));
    let mut ec = elevations.chunks_exact(4);
    let mut oc = out.chunks_exact_mut(12);
    for (e, o) in ec.by_ref().zip(oc.by_ref()) {
        let e = f32x4::new([e[0], e[1], e[2], e[3]]);
        let v = (e + bias4) * scale4;
        let v = e.cmp_ne(e).blend(zero, v).max(zero).min(max4);
        let a = v.to_array();
        for k in 0..4 {
            let x = a[k] as u32;
            o[k * 3] = ((x >> 16) & 0xff) as u8;
            o[k * 3 + 1] = ((x >> 8) & 0xff) as u8;
            o[k * 3 + 2] = (x & 0xff) as u8;
        }
    }
}

fn decode_simd(rgb: &[u8], out: &mut [f32], mul: f32, add: f32) {
    let (mul4, add4) = (f32x4::splat(mul), f32x4::splat(add));
    let mut rc = rgb.chunks_exact(12);
    let mut oc = out.chunks_exact_mut(4);
    let raw = |p: &[u8]| (p[0] as u32 * 65536 + p[1] as u32 * 256 + p[2] as u32) as f32;
    for (r, o) in rc.by_ref().zip(oc.by_ref()) {
        let raws = f32x4::new([raw(&r[0..3]), raw(&r[3..6]), raw(&r[6..9]), raw(&r[9..12])]);
        (raws * mul4 + add4)
            .to_array()
            .iter()
            .enumerate()
            .for_each(|(k, &v)| o[k] = v);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn hm_encode_scalar(iters: u32) -> f64 {
    let elev = make_elevations(RASTER * RASTER);
    let mut out = vec![0u8; RASTER * RASTER * 3];
    let mut acc = 0.0;
    for _ in 0..iters {
        terrarium::encode_into(black_box(&elev), &mut out);
        acc += out[0] as f64 + out[out.len() - 1] as f64;
    }
    acc
}

#[unsafe(no_mangle)]
pub extern "C" fn hm_encode_simd(iters: u32) -> f64 {
    let elev = make_elevations(RASTER * RASTER);
    let mut out = vec![0u8; RASTER * RASTER * 3];
    let mut acc = 0.0;
    for _ in 0..iters {
        encode_simd(black_box(&elev), &mut out, 32768.0, 256.0);
        acc += out[0] as f64 + out[out.len() - 1] as f64;
    }
    acc
}

#[unsafe(no_mangle)]
pub extern "C" fn hm_decode_scalar(iters: u32) -> f64 {
    let elev = make_elevations(RASTER * RASTER);
    let rgb = terrarium::encode(&elev, RASTER as u32, RASTER as u32);
    let mut out = vec![0f32; RASTER * RASTER];
    let mut acc = 0.0;
    for _ in 0..iters {
        terrarium::decode_into(black_box(&rgb), &mut out);
        acc += out[0] as f64;
    }
    acc
}

#[unsafe(no_mangle)]
pub extern "C" fn hm_decode_simd(iters: u32) -> f64 {
    let elev = make_elevations(RASTER * RASTER);
    let rgb = terrarium::encode(&elev, RASTER as u32, RASTER as u32);
    let mut out = vec![0f32; RASTER * RASTER];
    let mut acc = 0.0;
    for _ in 0..iters {
        decode_simd(black_box(&rgb), &mut out, 1.0 / 256.0, -32768.0);
        acc += out[0] as f64;
    }
    acc
}

// ---------------------------------------------------------------------------
// Oct normals: scalar (shipped) vs explicit SIMD (reverted variant)
// ---------------------------------------------------------------------------

fn oct_encode_simd(normals: &[[f32; 3]], out: &mut [u8]) {
    let one = f32x4::splat(1.0);
    let neg_one = f32x4::splat(-1.0);
    let zero = f32x4::splat(0.0);
    let half = f32x4::splat(0.5);
    let n255 = f32x4::splat(255.0);
    let mut nc = normals.chunks_exact(4);
    let mut oc = out.chunks_exact_mut(8);
    for (n, o) in nc.by_ref().zip(oc.by_ref()) {
        let x = f32x4::new([n[0][0], n[1][0], n[2][0], n[3][0]]);
        let y = f32x4::new([n[0][1], n[1][1], n[2][1], n[3][1]]);
        let z = f32x4::new([n[0][2], n[1][2], n[2][2], n[3][2]]);
        let inv_l1 = one / (x.abs() + y.abs() + z.abs());
        let px = x * inv_l1;
        let py = y * inv_l1;
        let neg = z.cmp_lt(zero);
        let sx = px.cmp_ge(zero).blend(one, neg_one);
        let sy = py.cmp_ge(zero).blend(one, neg_one);
        let fx = (one - py.abs()) * sx;
        let fy = (one - px.abs()) * sy;
        let px = neg.blend(fx, px);
        let py = neg.blend(fy, py);
        let bx = ((px * half + half) * n255).max(zero).min(n255).to_array();
        let by = ((py * half + half) * n255).max(zero).min(n255).to_array();
        for k in 0..4 {
            o[k * 2] = bx[k] as u8;
            o[k * 2 + 1] = by[k] as u8;
        }
    }
}

fn oct_decode_simd(bytes: &[u8], out: &mut [[f32; 3]]) {
    let one = f32x4::splat(1.0);
    let neg_one = f32x4::splat(-1.0);
    let zero = f32x4::splat(0.0);
    let two = f32x4::splat(2.0);
    let n255 = f32x4::splat(255.0);
    let mut bc = bytes.chunks_exact(8);
    let mut oc = out.chunks_exact_mut(4);
    for (b, o) in bc.by_ref().zip(oc.by_ref()) {
        let e0 = f32x4::new([b[0] as f32, b[2] as f32, b[4] as f32, b[6] as f32]);
        let e1 = f32x4::new([b[1] as f32, b[3] as f32, b[5] as f32, b[7] as f32]);
        let x = (e0 / n255) * two - one;
        let y = (e1 / n255) * two - one;
        let z = one - x.abs() - y.abs();
        let neg = z.cmp_lt(zero);
        let sx = x.cmp_ge(zero).blend(one, neg_one);
        let sy = y.cmp_ge(zero).blend(one, neg_one);
        let fx = (one - y.abs()) * sx;
        let fy = (one - x.abs()) * sy;
        let x = neg.blend(fx, x);
        let y = neg.blend(fy, y);
        let len = (x * x + y * y + z * z).sqrt();
        let pos = len.cmp_gt(zero);
        let rx = pos.blend(x / len, zero).to_array();
        let ry = pos.blend(y / len, zero).to_array();
        let rz = pos.blend(z / len, one).to_array();
        for k in 0..4 {
            o[k] = [rx[k], ry[k], rz[k]];
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn oct_encode_scalar(iters: u32) -> f64 {
    let normals = make_normals(GRID * GRID);
    let mut out = vec![0u8; GRID * GRID * 2];
    let mut acc = 0.0;
    for _ in 0..iters {
        for (n, o) in black_box(&normals).iter().zip(out.chunks_exact_mut(2)) {
            o.copy_from_slice(&oct_encode_normal(*n));
        }
        acc += out[0] as f64;
    }
    acc
}

/// Times the SHIPPED library function (`oct_encode_normals_into`), which uses
/// simd128 on wasm.
#[unsafe(no_mangle)]
pub extern "C" fn oct_encode_simd_bench(iters: u32) -> f64 {
    let normals = make_normals(GRID * GRID);
    let mut out = vec![0u8; GRID * GRID * 2];
    let mut acc = 0.0;
    for _ in 0..iters {
        oct_encode_normals_into(black_box(&normals), &mut out);
        acc += out[0] as f64;
    }
    acc
}

/// Correctness check: returns the number of bytes where the shipped batch
/// encoder disagrees with the per-normal scalar encoder. Must be 0.
#[unsafe(no_mangle)]
pub extern "C" fn oct_encode_verify() -> u32 {
    let normals = make_normals(GRID * GRID + 3); // non-multiple of 4
    let mut batch = vec![0u8; normals.len() * 2];
    oct_encode_normals_into(&normals, &mut batch);
    let mut mism = 0u32;
    for (i, n) in normals.iter().enumerate() {
        let s = oct_encode_normal(*n);
        if batch[i * 2] != s[0] || batch[i * 2 + 1] != s[1] {
            mism += 1;
        }
    }
    mism
}

#[unsafe(no_mangle)]
pub extern "C" fn oct_decode_scalar(iters: u32) -> f64 {
    let normals = make_normals(GRID * GRID);
    let mut bytes = vec![0u8; GRID * GRID * 2];
    oct_encode_simd(&normals, &mut bytes);
    let mut out = vec![[0f32; 3]; GRID * GRID];
    let mut acc = 0.0;
    for _ in 0..iters {
        for (c, dst) in black_box(&bytes).chunks_exact(2).zip(out.iter_mut()) {
            *dst = oct_decode_normal([c[0], c[1]]);
        }
        acc += out[0][2] as f64;
    }
    acc
}

#[unsafe(no_mangle)]
pub extern "C" fn oct_decode_simd_bench(iters: u32) -> f64 {
    let normals = make_normals(GRID * GRID);
    let mut bytes = vec![0u8; GRID * GRID * 2];
    oct_encode_simd(&normals, &mut bytes);
    let mut out = vec![[0f32; 3]; GRID * GRID];
    let mut acc = 0.0;
    for _ in 0..iters {
        oct_decode_simd(black_box(&bytes), &mut out);
        acc += out[0][2] as f64;
    }
    acc
}
