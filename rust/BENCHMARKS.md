# Benchmarks & SIMD investigation

This documents the performance work on the hot paths of `terrain-codec` /
`quantized-mesh` / `martini`, with an eye on staying compilable to
**WebAssembly with SIMD (`simd128`)**.

Two harnesses are used:

- **Native** — [criterion](https://docs.rs/criterion) benches, run on the host
  (measurements below are Apple Silicon, `aarch64`, NEON, `--release`).
- **wasm** — the `wasmbench` crate (a `cdylib`, `publish = false`) exposes
  `extern "C"` kernels; `wasmbench/bench.js` times them under Node.js. This is
  how the actual `wasm32 + simd128` numbers were obtained without a standalone
  wasm runtime.

## Running

```sh
# Native (criterion). Save a baseline, then compare after a change:
cargo bench -p terrain-codec -p quantized-mesh -- --save-baseline step0
cargo bench -p terrain-codec -p quantized-mesh -- --baseline step0

# wasm (Node). simd128 is enabled via .cargo/config.toml.
cargo build -p wasmbench --target wasm32-unknown-unknown --release
node wasmbench/bench.js target/wasm32-unknown-unknown/release/wasmbench.wasm
```

## Summary of what shipped

| Change | Where | Win | Shipped |
|---|---|---|---|
| Hoist the per-row web-mercator inverse (`tan().asinh()`) out of the pixel loop | `mercator.rs` | **5.4–8.7×** (native + wasm) | ✅ |
| `-C target-feature=+simd128` for wasm builds (enables autovectorisation) | `.cargo/config.toml` | **1.2–2.2×** on wasm, zero code change | ✅ |
| wasm-only simd128 batch oct-normal **encode** | `quantized-mesh/encoding.rs` | **1.67×** on wasm | ✅ (wasm only) |
| Explicit SIMD for RGB heightmap codec | — | regressed | ❌ reverted |
| Explicit SIMD for oct-normal **decode** | — | regressed / marginal | ❌ not shipped |

---

## 1. Mercator resampling — the biggest win (algorithmic, not SIMD)

`MercatorDem::geodetic_grid` / `buffered_geodetic` recomputed the web-mercator
inverse `tan().asinh()` **per output pixel**, but latitude is constant across a
row. Hoisting that transcendental into a per-row `RowSampler` makes the inner
loop pure linear index math + a bilinear blend. Output is bit-for-bit
identical.

**Native (criterion):**

| Bench | Before | After | Change |
|---|---|---|---|
| `geodetic_grid/257` | 2.131 ms | 366.7 µs | **−82.9% (5.8×)** |
| `buffered_geodetic/257+4` | 3.073 ms | 358.2 µs | **−88.5% (8.7×)** |

**wasm (Node, simd128 on), per-op:**

| | Before (per-pixel) | After (hoisted) |
|---|---|---|
| `geodetic_grid` (257²) | 4110 µs | 758 µs — **5.4×** |

Mercator resampling was by far the hottest path (milliseconds vs microseconds
for everything else), so this dominates end-to-end tile generation cost.

---

## 2. The `+simd128` build flag alone (autovectorisation)

On wasm, nothing SIMD is emitted without `-C target-feature=+simd128`. Simply
enabling it lets LLVM autovectorise the **existing scalar** code. Measured on
the shipped scalar kernels (Node, per-op, flag off → on):

| Kernel (scalar) | off | on | Speedup |
|---|---|---|---|
| `geodetic_grid` | 1341 µs | 758 µs | 1.77× |
| heightmap encode | 365 µs | 200 µs | 1.83× |
| heightmap decode | 280 µs | 236 µs | 1.19× |
| oct encode | 651 µs | 492 µs | 1.32× |
| oct decode | 299 µs | 136 µs | 2.20× |

This is a free win — build flag only, no code change — so `.cargo/config.toml`
sets it for the wasm targets.

---

## 3. Explicit `wide` SIMD — mixed on wasm, negative on native

Hand-written `wide` (`f32x4`) kernels were prototyped for the RGB heightmap
codec and the oct-normal codec, then benchmarked against the scalar form.

**Native (criterion), apples-to-apples (same allocation-free buffers):**

| Kernel | scalar | explicit SIMD | Verdict |
|---|---|---|---|
| heightmap encode | 151.6 µs | 169 µs | slower |
| heightmap decode | 62.7 µs | 136 µs | **2× slower** |
| oct encode | 75.7 µs | 78.2 µs | ~3% slower |
| oct decode | 75.6 µs | 102 µs | 35% slower |

On native, the scalar form autovectorises better, and de-interleaving
array-of-struct data (`[f32; 3]` / `[u8; 3]` / `[u8; 2]`) into SIMD lanes —
with no gather instruction — costs more than the vectorised arithmetic saves.
So explicit SIMD lost across the board natively.

**wasm (Node, simd128 on), scalar / simd ratio (>1 = SIMD faster):**

| Kernel | ratio | Verdict |
|---|---|---|
| **oct encode** | **1.67×** | SIMD wins — **shipped (wasm only)** |
| heightmap decode | 1.24× | marginal — not shipped |
| heightmap encode | 0.56× | 1.8× slower — not shipped |
| oct decode | 0.87× | slower — not shipped |

wasm's autovectoriser is weaker than native LLVM, so explicit SIMD **can** help
on wasm even where it regressed natively — but it is per-kernel. Only
oct-encode was a clear, reproducible win, so only it was adopted, gated to
`#[cfg(target_arch = "wasm32")]`. Native keeps the scalar path and doesn't even
pull the `wide` dependency (it's a wasm-only target dependency).

The shipped wasm encoder is verified bit-for-bit identical to the scalar
encoder under Node (`oct_encode_verify: 0 mismatches`).

---

## Takeaways

1. **Look for redundant work before reaching for SIMD.** The Mercator
   transcendental hoist beat every SIMD attempt by an order of magnitude.
2. **The `+simd128` flag is the cheapest wasm win** — autovectorisation of
   plain scalar loops, no code and no unsafe.
3. **Memory-bound loops and AoS-struct codecs don't like hand-written SIMD**
   (no gather, de-interleave overhead) — the compiler's autovectoriser wins.
4. **Native ≠ wasm.** SIMD that regresses natively can still win on wasm. Always
   measure both targets before committing a SIMD path, and gate it per-target.
