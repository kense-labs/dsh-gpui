#!/usr/bin/env node
// Opt-in blaze accelerator for the *native Tauri host* dev loop only.
//
// blaze is a Rust incremental compiler, not a size optimiser: it refuses LTO,
// has no cross-function optimisation, and its wasm32 modules are documented as
// 8-50x larger than rustc's. So the WASM frontend and every release build stay
// on cargo. See README.md -> "构建提速与产物体积".
//
// blaze is young and KWork pulls in crates its codegen cannot compile yet. This
// wrapper probes blaze once per (blaze version, Cargo.lock, platform), remembers
// the answer in .tools/blaze-compat.json, and falls back to cargo for good when
// the probe fails. A default `just dev` therefore never gets slower or breaks.
//
//   KWORK_BLAZE=auto   (default) probe once, then use the cached answer
//   KWORK_BLAZE=force  probe again even if a cached failure exists
//   KWORK_BLAZE=off    cargo only
//   KWORK_BLAZE_ARGS   extra flags, e.g. "--target-dir target" for blaze v2,
//                      which otherwise builds outside the checkout

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const statePath = join(root, '.tools', 'blaze-compat.json');
const mode = process.env.KWORK_BLAZE || 'auto';
const extraArgs = (process.env.KWORK_BLAZE_ARGS || '').trim().split(/\s+/).filter(Boolean);

const invoke = (command, args, options = {}) => {
  const result = spawnSync(command, args, {
    cwd: root,
    env: process.env,
    stdio: options.capture ? ['ignore', 'pipe', 'pipe'] : 'inherit',
    encoding: options.capture ? 'utf8' : undefined,
  });
  return { code: result.error ? 1 : (result.status ?? 1), missing: result.error?.code === 'ENOENT', stdout: result.stdout || '' };
};

const cargoBuild = () => {
  const { code } = invoke('cargo', ['build', '-p', 'dsh-gpui']);
  process.exit(code);
};

if (mode === 'off') cargoBuild();

const versionResult = invoke('blaze', ['--version'], { capture: true });
if (versionResult.missing || versionResult.code !== 0) {
  console.error('blaze 未安装或不在 PATH 中，使用 cargo。安装：curl -fsSL https://ix.dev/blaze | sh');
  cargoBuild();
}
const version = versionResult.stdout.trim().split('\n')[0];
const key = createHash('sha256')
  .update(version)
  .update(readFileSync(join(root, 'Cargo.lock')))
  .update(`${process.platform}-${process.arch}`)
  .digest('hex')
  .slice(0, 16);

let cached = null;
if (existsSync(statePath)) {
  try { cached = JSON.parse(readFileSync(statePath, 'utf8')); } catch { /* re-probe */ }
}
if (mode !== 'force' && cached?.key === key && cached.status === 'failed') {
  console.error(`blaze 与当前 KWork 不兼容（探测于 ${cached.at}，退出码 ${cached.code}）；使用 cargo。`);
  console.error(`上游修复后可执行 KWORK_BLAZE=force bun scripts/blaze-build.mjs 重新探测。`);
  cargoBuild();
}

console.error(`blaze: ${version} (探测${cached?.key === key ? '缓存命中' : '首次'}，失败会自动回退 cargo)`);
const blazeResult = invoke('blaze', ['build', '-p', 'dsh-gpui', ...extraArgs]);
const record = {
  key,
  version,
  status: blazeResult.code === 0 ? 'ok' : 'failed',
  code: blazeResult.code,
  at: new Date().toISOString(),
};
mkdirSync(dirname(statePath), { recursive: true });
writeFileSync(statePath, `${JSON.stringify(record, null, 2)}\n`);
if (blazeResult.code === 0) process.exit(0);

console.error('blaze 构建失败，已回退 cargo；后续同版本构建会直接使用 cargo，直到重新探测。');
cargoBuild();
