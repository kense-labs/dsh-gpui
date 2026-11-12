#!/usr/bin/env node
// Drive the headless KWork harness over CDP: click through to the
// model-services settings page and capture a screenshot.
//
// The UI paints into a canvas, so there are no DOM nodes to query; navigation
// is by the coordinates the accessibility tree reports. Kept as a script (rather
// than a one-off) so the same visual check can be repeated after UI changes.
//
// Usage: node scripts/render-provider-marks.mjs <port> <output.png>
import { writeFileSync } from 'node:fs';

const port = Number(process.argv[2] ?? 9333);
const output = process.argv[3] ?? '/tmp/kw-providers.png';

const version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = targets.find(target => target.type === 'page');
if (!page) throw new Error('no page target');

const socket = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();
socket.addEventListener('message', event => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
});
await new Promise(resolve => socket.addEventListener('open', resolve));

const send = (method, params = {}) => new Promise((resolve, reject) => {
  const id = nextId++;
  pending.set(id, message => (message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result)));
  socket.send(JSON.stringify({ id, method, params }));
});

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const click = async (x, y) => {
  for (const type of ['mousePressed', 'mouseReleased']) {
    await send('Input.dispatchMouseEvent', { type, x, y, button: 'left', clickCount: 1 });
  }
};

// Wait for the WASM app to paint its first frame.
for (let attempt = 0; attempt < 40; attempt += 1) {
  const ready = await send('Runtime.evaluate', { expression: 'globalThis.__kwReady === true', returnByValue: true });
  if (ready.result?.value === true) break;
  await sleep(500);
}
await sleep(2500);

// Sidebar "更多", then the settings entry inside that page.
const steps = JSON.parse(process.env.KW_STEPS ?? '[]');
for (const [x, y, waitMs] of steps) {
  await click(x, y);
  await sleep(waitMs ?? 900);
}

const shot = await send('Page.captureScreenshot', { format: 'png' });
writeFileSync(output, Buffer.from(shot.data, 'base64'));
console.log(`wrote ${output}`);
socket.close();
