#!/usr/bin/env node
// Rasterize the provider marks in ui/assets/providers from SVG to PNG.
//
// Why this exists: GPUI paints every `svg()` element as a single-channel alpha
// mask tinted by the element's `text_color` (see `render_alpha_mask` in
// gpui/src/svg_renderer.rs, which keeps only `pixel.alpha()`). Source colors are
// therefore discarded, so artwork shaped like "colored rounded tile + white
// glyph" collapses into one flat silhouette. `img()` decodes a real RGBA image
// and keeps the original colors, so the marks are pre-rendered here instead.
//
// The SVG files stay the source of truth. Run this script after editing them:
//
//     node scripts/build-provider-marks.mjs
import { execFileSync } from 'node:child_process';
import {
  copyFileSync, existsSync, mkdtempSync, readdirSync, rmSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const marksDir = join(root, 'ui', 'assets', 'providers');

// Marks render at 16-20px; 64px keeps them crisp on 3x displays.
const SIZE = 64;

const CHROME_CANDIDATES = [
  process.env.CHROME_PATH,
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
].filter(Boolean);

const chrome = CHROME_CANDIDATES.find(candidate => existsSync(candidate));
if (chrome === undefined) {
  console.error('build-provider-marks: no Chrome/Chromium found; set CHROME_PATH.');
  process.exit(1);
}

const sources = readdirSync(marksDir).filter(name => name.endsWith('.svg')).sort();
if (sources.length === 0) {
  console.error(`build-provider-marks: no SVG sources in ${marksDir}.`);
  process.exit(1);
}

const scratch = mkdtempSync(join(tmpdir(), 'provider-marks-'));
let written = 0;
try {
  for (const name of sources) {
    const source = join(marksDir, name);
    const target = join(marksDir, `${basename(name, '.svg')}.png`);

    // Screenshot through an HTML host so the page box, not the SVG's intrinsic
    // size, decides the raster size, and keep the background transparent.
    const page = join(scratch, `${basename(name, '.svg')}.html`);
    writeFileSync(page, `<!doctype html><meta charset="utf-8">`
      + `<style>html,body{margin:0;padding:0;background:transparent}`
      + `img{display:block;width:${SIZE}px;height:${SIZE}px}</style>`
      + `<img src="${name}">`);

    copyFileSync(source, join(scratch, name));
    execFileSync(chrome, [
      '--headless', '--disable-gpu', '--hide-scrollbars',
      '--default-background-color=00000000',
      `--screenshot=${target}`,
      `--window-size=${SIZE},${SIZE}`,
      page,
    ], { cwd: scratch, stdio: 'ignore' });
    written += 1;
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

console.log(`build-provider-marks: wrote ${written} PNG mark(s) at ${SIZE}px into ui/assets/providers.`);
