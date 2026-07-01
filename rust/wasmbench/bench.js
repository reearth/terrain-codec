// Node driver: loads the wasm module and times each kernel export.
// Usage: node bench.js <path-to.wasm>
const fs = require('fs');
const { performance } = require('perf_hooks');

const wasmPath = process.argv[2];
if (!wasmPath) {
  console.error('usage: node bench.js <path-to.wasm>');
  process.exit(1);
}

// Each entry: [label, exportName, iters-per-call]. iters chosen so one call
// is a few ms; per-op time = elapsed / iters.
const CASES = [
  ['mercator/geodetic_grid  OLD (per-pixel)', 'merc_old', 40],
  ['mercator/geodetic_grid  NEW (hoisted)  ', 'merc_new', 40],
  ['heightmap/encode  scalar', 'hm_encode_scalar', 200],
  ['heightmap/encode  simd  ', 'hm_encode_simd', 200],
  ['heightmap/decode  scalar', 'hm_decode_scalar', 200],
  ['heightmap/decode  simd  ', 'hm_decode_simd', 200],
  ['oct/encode  scalar', 'oct_encode_scalar', 200],
  ['oct/encode  simd  ', 'oct_encode_simd_bench', 200],
  ['oct/decode  scalar', 'oct_decode_scalar', 200],
  ['oct/decode  simd  ', 'oct_decode_simd_bench', 200],
];

function bestOf(fn, iters, trials) {
  // Warmup.
  for (let i = 0; i < 3; i++) fn(iters);
  let best = Infinity;
  let checksum = 0;
  for (let t = 0; t < trials; t++) {
    const t0 = performance.now();
    checksum = fn(iters);
    const dt = performance.now() - t0;
    if (dt < best) best = dt;
  }
  return { best, checksum };
}

(async () => {
  const bytes = fs.readFileSync(wasmPath);
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const ex = instance.exports;

  console.log(`wasm: ${wasmPath}`);
  console.log(`node: ${process.version}\n`);

  // Correctness: shipped simd128 oct-encode must match scalar byte-for-byte.
  if (typeof ex.oct_encode_verify === 'function') {
    const mism = ex.oct_encode_verify();
    console.log(`oct_encode_verify: ${mism} mismatches ${mism === 0 ? '✓' : '✗ FAIL'}\n`);
  }
  console.log('kernel                                    per-call(ms)   per-op(us)');
  console.log('-'.repeat(74));

  const results = {};
  for (const [label, name, iters] of CASES) {
    const fn = ex[name];
    if (typeof fn !== 'function') {
      console.log(`${label}   <missing export ${name}>`);
      continue;
    }
    const { best } = bestOf(fn, iters, 12);
    const perOp = (best / iters) * 1000; // us
    results[name] = perOp;
    console.log(
      `${label.padEnd(40)}  ${best.toFixed(3).padStart(10)}   ${perOp.toFixed(3).padStart(10)}`
    );
  }

  // Speed ratios.
  console.log('\nratios (scalar / simd, >1 means simd faster):');
  const pairs = [
    ['heightmap/encode', 'hm_encode_scalar', 'hm_encode_simd'],
    ['heightmap/decode', 'hm_decode_scalar', 'hm_decode_simd'],
    ['oct/encode', 'oct_encode_scalar', 'oct_encode_simd_bench'],
    ['oct/decode', 'oct_decode_scalar', 'oct_decode_simd_bench'],
  ];
  for (const [label, s, v] of pairs) {
    if (results[s] && results[v]) {
      console.log(`  ${label.padEnd(20)} ${(results[s] / results[v]).toFixed(2)}x`);
    }
  }
  if (results['merc_old'] && results['merc_new']) {
    console.log(
      `  ${'mercator new/old'.padEnd(20)} ${(results['merc_old'] / results['merc_new']).toFixed(2)}x faster`
    );
  }
})();
