import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const rustc = execFileSync('rustup', ['which', 'rustc'], { encoding: 'utf8' }).trim();
const env = {
  ...process.env,
  PATH: `${dirname(rustc)}:${process.env.PATH || ''}`,
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
copyFileSync(join(root, 'ui', 'assets', 'deepseek-fish.svg'), join(dist, 'deepseek-fish.svg'));
for (const file of ['index.html', 'app.css', 'bootstrap.js']) {
  copyFileSync(join(root, 'ui', file), join(dist, file));
}
console.log(`KUI/GPUI frontend ready: ${dist}`);
