import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
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

run('rustup', ['target', 'add', 'wasm32-unknown-unknown']);
run('rustup', [
  'run', 'stable', 'cargo', 'build',
  '-p', 'dsh-gpui-ui',
  '--target', 'wasm32-unknown-unknown',
  '--release',
]);

const lock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
const match = lock.match(/name = "wasm-bindgen"\nversion = "([^"]+)"/);
if (!match) throw new Error('无法从 Cargo.lock 确定 wasm-bindgen 版本。');
const version = match[1];
const tools = process.env.CARGO_INSTALL_ROOT || join(root, '.tools');
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
const pkg = join(dist, 'pkg');
rmSync(dist, { recursive: true, force: true });
mkdirSync(pkg, { recursive: true });
run(bindgen, [
  '--target', 'web', '--no-typescript',
  '--out-dir', pkg,
  join(root, 'target', 'wasm32-unknown-unknown', 'release', 'dsh_gpui_ui.wasm'),
]);
const { metafile: preview } = await build({
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
});
const bundledPackages = new Set(Object.keys(preview.inputs).flatMap(file =>
  file.match(/^(.*node_modules\/(?:@[^/]+\/)?[^/]+)\//)?.[1] ?? [],
));
const previewLicenses = [...bundledPackages].sort().flatMap(path =>
  readdirSync(join(root, path)).filter(file => /^(licen[cs]e|notice)(\.|$)/i.test(file)).map(file =>
    `${path.slice(path.lastIndexOf('node_modules/') + 'node_modules/'.length)} / ${file}\n\n${readFileSync(join(root, path, file), 'utf8')}`,
  ),
);
writeFileSync(join(dist, 'preview-LICENSES.txt'), previewLicenses.join('\n\n'));
copyFileSync(join(root, 'ui', 'assets', 'DeepSeek-LICENSE.txt'), join(dist, 'DeepSeek-LICENSE.txt'));
copyFileSync(join(root, 'src-tauri', 'icons', '32x32.png'), join(dist, 'kense-mark.png'));
copyFileSync(join(root, 'ui', 'assets', 'xterm-LICENSE.txt'), join(dist, 'xterm-LICENSE.txt'));
copyFileSync(
  join(root, 'node_modules', '@xterm', 'headless', 'lib-headless', 'xterm-headless.mjs'),
  join(dist, 'xterm-headless.mjs'),
);
copyFileSync(
  join(root, 'node_modules', '@xterm', 'addon-serialize', 'lib', 'addon-serialize.mjs'),
  join(dist, 'xterm-addon-serialize.mjs'),
);
for (const file of ['index.html', 'app.css', 'bootstrap.js', 'terminal-screen.js', 'terminal-selection.js']) {
  copyFileSync(join(root, 'ui', file), join(dist, file));
}
console.log(`KUI/GPUI frontend ready: ${dist}`);
