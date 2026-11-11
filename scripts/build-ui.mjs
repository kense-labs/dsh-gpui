import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, statSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const root = dirname(dirname(fileURLToPath(import.meta.url)));

// wasm-opt is the second half of the size pipeline: cargo's `release-wasm`
// profile makes small code, wasm-opt -Oz makes the wasm-bindgen output small.
// A native binary is used where one is published; elsewhere binaryen's Wasm
// build of the same tool is the portable fallback.
const WASM_OPT_VERSION = '2.0.1';
const BINARYEN_VERSION = '132.0.0';
const NATIVE_WASM_OPT_PACKAGES = {
  'darwin-arm64': '@wasm-opt/darwin-arm64',
  'linux-x64': '@wasm-opt/linux-x64',
  'linux-arm64': '@wasm-opt/linux-arm64',
  'win32-x64': '@wasm-opt/win32-x64',
  'win32-arm64': '@wasm-opt/win32-arm64',
};
// Features rustc enables on wasm32-unknown-unknown. wasm-opt must be told about
// them or it rejects the wasm-bindgen output (memory.fill / multi-value etc.).
const WASM_OPT_FEATURES = [
  '--enable-bulk-memory',
  '--enable-bulk-memory-opt',
  '--enable-multivalue',
  '--enable-mutable-globals',
  '--enable-nontrapping-float-to-int',
  '--enable-reference-types',
  '--enable-sign-ext',
];
const rustc = execFileSync('rustup', ['which', 'rustc'], { encoding: 'utf8' }).trim();
const env = {
  ...process.env,
  PATH: `${dirname(rustc)}:${process.env.PATH || ''}`,
  // Keep wasm artifacts in-repo so wasm-bindgen never packs a stale sandbox target dir.
  CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR || join(root, 'target'),
};
const run = (command, args) => {
  execFileSync(command, args, { cwd: root, env, stdio: 'inherit' });
};

function ensureWasmOpt(tools) {
  if (process.env.KWORK_WASM_OPT === 'off') return null;
  const prefix = join(tools, 'wasm-opt');
  const bin = join(prefix, 'node_modules', '.bin', process.platform === 'win32' ? 'wasm-opt.cmd' : 'wasm-opt');
  if (existsSync(bin)) return bin;
  const platform = `${process.platform}-${process.arch}`;
  const native = NATIVE_WASM_OPT_PACKAGES[platform];
  const packages = native
    ? [`wasm-opt@${WASM_OPT_VERSION}`, `${native}@${WASM_OPT_VERSION}`]
    : [`binaryen@${BINARYEN_VERSION}`];
  console.log(`Installing wasm-opt (${native ? 'native' : 'portable'}) into ${prefix} ...`);
  run('npm', [
    'install', '--prefix', prefix, '--no-save', '--no-package-lock',
    '--no-audit', '--no-fund', ...packages,
  ]);
  if (!existsSync(bin)) throw new Error('wasm-opt 安装失败：找不到可执行文件。');
  return bin;
}

function optimizeWasm(wasmPath, wasmOpt, tools) {
  if (!wasmOpt) return;
  const input = readFileSync(wasmPath);
  const key = createHash('sha256')
    .update(input)
    .update(JSON.stringify({ version: WASM_OPT_VERSION, features: WASM_OPT_FEATURES }))
    .digest('hex');
  const cache = join(tools, 'wasm-opt-cache');
  mkdirSync(cache, { recursive: true });
  const optimized = join(cache, `${key}.wasm`);
  if (existsSync(optimized)) {
    writeFileSync(wasmPath, readFileSync(optimized));
    console.log(`wasm-opt: cache hit (${(input.length / 1024 / 1024).toFixed(1)} MB -> ${(statSync(optimized).size / 1024 / 1024).toFixed(1)} MB)`);
    return;
  }
  run(wasmOpt, ['-Oz', ...WASM_OPT_FEATURES, '--strip-debug', '--strip-producers', wasmPath, '-o', optimized]);
  writeFileSync(wasmPath, readFileSync(optimized));
  const after = statSync(optimized).size;
  console.log(`wasm-opt: ${(input.length / 1024 / 1024).toFixed(1)} MB -> ${(after / 1024 / 1024).toFixed(1)} MB (${((1 - after / input.length) * 100).toFixed(1)}% smaller)`);
}

function collectFiles(dir, prefix, outputs) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    const key = join(prefix, entry.name);
    if (entry.isDirectory()) collectFiles(path, key, outputs);
    else outputs.set(key, readFileSync(path));
  }
}

// Write only what changed. The old script deleted the whole ui/dist tree and
// rewrote ~100 files on every build; this keeps warm builds cheap and still
// removes stale content-hashed esbuild chunks. It also avoids a bulk recursive
// delete, which sandboxed editors refuse.
function syncOutputs(dist, outputs) {
  mkdirSync(dist, { recursive: true });
  let written = 0;
  for (const [name, data] of outputs) {
    const target = join(dist, name);
    mkdirSync(dirname(target), { recursive: true });
    if (existsSync(target)) {
      const current = readFileSync(target);
      if (current.length === data.length && current.equals(data)) continue;
    }
    writeFileSync(target, data);
    written++;
  }
  const stale = [];
  const walk = (dir, prefix) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const key = join(prefix, entry.name);
      if (entry.isDirectory()) walk(join(dir, entry.name), key);
      else if (!outputs.has(key)) stale.push(join(dist, key));
    }
  };
  walk(dist, '');
  for (const path of stale) rmSync(path, { force: true });
  return { written, stale: stale.length };
}

run('rustup', ['target', 'add', 'wasm32-unknown-unknown']);
run('rustup', [
  'run', 'stable', 'cargo', 'build',
  '-p', 'dsh-gpui-ui',
  '--target', 'wasm32-unknown-unknown',
  '--profile', 'release-wasm',
]);

const lock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
const match = lock.match(/name = "wasm-bindgen"\nversion = "([^"]+)"/);
if (!match) throw new Error('无法从 Cargo.lock 确定 wasm-bindgen 版本。');
const version = match[1];
const tools = process.env.CARGO_INSTALL_ROOT || join(root, '.tools');
const wasmOpt = ensureWasmOpt(tools);
const executable = process.platform === 'win32' ? 'wasm-bindgen.exe' : 'wasm-bindgen';
const bindgen = join(tools, 'bin', executable);
let installed = false;
if (existsSync(bindgen)) {
  const output = execFileSync(bindgen, ['--version'], { encoding: 'utf8' }).trim();
  installed = output === `wasm-bindgen ${version}`;
}
if (!installed) {
  run('rustup', [
    'run', 'stable', 'cargo', 'install', 'wasm-bindgen-cli',
    '--locked', '--version', `=${version}`, '--root', tools,
  ]);
}

const dist = join(root, 'ui', 'dist');
const stage = mkdtempSync(join(tmpdir(), 'kwork-ui-'));
const outputs = new Map();
try {
  const stagePkg = join(stage, 'pkg');
  mkdirSync(stagePkg, { recursive: true });
  run(bindgen, [
    '--target', 'web', '--no-typescript',
    '--out-dir', stagePkg,
    join(root, 'target', 'wasm32-unknown-unknown', 'release-wasm', 'dsh_gpui_ui.wasm'),
  ]);
  optimizeWasm(join(stagePkg, 'dsh_gpui_ui_bg.wasm'), wasmOpt, tools);
  collectFiles(stagePkg, 'pkg', outputs);

  // `write: false` keeps esbuild's output in memory so unchanged chunks are
  // never rewritten to disk.
  const { outputFiles, metafile: preview } = await build({
    absWorkingDir: root,
    entryPoints: [
      'ui/document-highlight.js', 'ui/document-frontmatter.js',
      'ui/document-fonts.js', 'ui/document-fonts-worker.js',
      'ui/document-html.js',
    ],
    outdir: dist,
    chunkNames: 'syntax/[name]-[hash]',
    assetNames: 'fonts/[name]-[hash]',
    loader: { '.wasm': 'file' },
    bundle: true,
    splitting: true,
    format: 'esm',
    minify: true,
    metafile: true,
    write: false,
  });
  for (const file of outputFiles) outputs.set(relative(dist, file.path), Buffer.from(file.contents));

  const bundledPackages = new Set(Object.keys(preview.inputs).flatMap(file =>
    file.match(/^(.*node_modules\/(?:@[^/]+\/)?[^/]+)\//)?.[1] ?? [],
  ));
  const previewLicenses = [...bundledPackages].sort().flatMap(path =>
    readdirSync(join(root, path)).filter(file => /^(licen[cs]e|notice)(\.|$)/i.test(file)).map(file =>
      `${path.slice(path.lastIndexOf('node_modules/') + 'node_modules/'.length)} / ${file}\n\n${readFileSync(join(root, path, file), 'utf8')}`,
    ),
  );
  outputs.set('preview-LICENSES.txt', Buffer.from(previewLicenses.join('\n\n')));
  outputs.set('DeepSeek-LICENSE.txt', readFileSync(join(root, 'ui', 'assets', 'DeepSeek-LICENSE.txt')));
  outputs.set('kense-mark.png', readFileSync(join(root, 'src-tauri', 'icons', '32x32.png')));
  outputs.set('xterm-LICENSE.txt', readFileSync(join(root, 'ui', 'assets', 'xterm-LICENSE.txt')));
  outputs.set(
    'xterm-headless.mjs',
    readFileSync(join(root, 'node_modules', '@xterm', 'headless', 'lib-headless', 'xterm-headless.mjs')),
  );
  outputs.set(
    'xterm-addon-serialize.mjs',
    readFileSync(join(root, 'node_modules', '@xterm', 'addon-serialize', 'lib', 'addon-serialize.mjs')),
  );
  for (const file of ['index.html', 'app.css', 'bootstrap.js', 'terminal-screen.js', 'terminal-selection.js']) {
    outputs.set(file, readFileSync(join(root, 'ui', file)));
  }

  const { written, stale } = syncOutputs(dist, outputs);
  console.log(`KUI/GPUI frontend ready: ${dist} (${outputs.size} files, ${written} rewritten, ${stale} stale removed)`);
} finally {
  try { rmSync(stage, { recursive: true, force: true }); } catch { /* temp cleanup is best effort */ }
}
