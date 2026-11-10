import { execFileSync } from 'node:child_process';
import { copyFile, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const mark = await readFile(join(root, 'ui/assets/kense-mark-dark.png'), 'base64');
const icon = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="plate" x2="0" y2="1">
      <stop stop-color="#353840"/>
      <stop offset="1" stop-color="#23252b"/>
    </linearGradient>
  </defs>
  <path fill="url(#plate)" stroke="#ffffff" stroke-opacity=".12" stroke-width="3"
    d="M 320 100 H 704 C 846 100 924 178 924 320 V 704 C 924 846 846 924 704 924 H 320 C 178 924 100 846 100 704 V 320 C 100 178 178 100 320 100 Z"/>
  <image x="218" y="218" width="588" height="588" opacity=".96" xlink:href="data:image/png;base64,${mark}"/>
</svg>`;

const temp = await mkdtemp(join(tmpdir(), 'kwork-icon-'));
try {
  const source = join(temp, 'kwork.svg');
  const output = join(temp, 'icons');
  await writeFile(source, icon);
  execFileSync(process.execPath, [
    join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'icon', source, '--output', output,
  ], { cwd: root, stdio: 'inherit' });
  for (const entry of await readdir(output, { withFileTypes: true })) {
    if (entry.isFile()) {
      await copyFile(join(output, entry.name), join(root, 'src-tauri/icons', entry.name));
    }
  }
} finally {
  await rm(temp, { force: true, recursive: true });
}
