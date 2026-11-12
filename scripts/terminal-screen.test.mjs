// Run against the packaged adapter after bun run ui:build.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SerializeAddon } from '../ui/dist/xterm-addon-serialize.mjs';
import {
  bindTerminal, createTerminal, fitTerminal, resizeScreen, restoreTerminal, terminalCopy, terminalFocus, terminalFrame, terminalInput,
  terminalMode, terminalMouse, terminalPaste, terminalScroll, terminalSelectAll, terminalSelectMove,
  terminalSelectStart, terminalWheel, unbindTerminal, writeScreen,
} from '../ui/dist/terminal-screen.js';

const line = (screen, row) => terminalFrame(screen).lines[row].map(run => run[2]).join('');

test('restores a host snapshot with its grid, alternate buffer and terminal modes', async () => {
  const input = [];
  globalThis.window = { __TAURI__: { core: { invoke: async (_, args) => input.push(args) } } };
  const host = createTerminal();
  const serializer = new SerializeAddon();
  host.engine.loadAddon(serializer);
  fitTerminal(host, 10, 4);
  await writeScreen(host, 'retained\r\n\x1b[6n');
  fitTerminal(host, 16, 5);
  await writeScreen(host, '\x1b[?1049h\x1b[H\x1b[31m中\x1b[0m\x1b[?2004h');

  const screen = createTerminal();
  await writeScreen(screen, 'obsolete\x1b[6n');
  await restoreTerminal(screen, host.engine.cols, host.engine.rows, serializer.serialize());
  assert.deepEqual(terminalFrame(screen), terminalFrame(host));
  assert.equal(line(screen, 0), '中');
  assert.equal(screen.engine.cols, 16);
  assert.equal(screen.engine.rows, 5);
  assert.equal(terminalMode(screen, 'bracketedPasteMode'), true);
  bindTerminal(screen, 'retained');
  await writeScreen(screen, '\x1b[?1049l');
  assert.equal(line(screen, 0), 'retained');
  terminalInput(screen, 'live');
  await screen.commands;
  assert.deepEqual(input, [{ terminalId: 'retained', data: 'live' }]);
});

test('applies queued output and resize in stream order after a snapshot', async () => {
  const expected = createTerminal();
  const current = createTerminal();
  for (const screen of [expected, current]) await restoreTerminal(screen, 8, 3, 'base');
  const before = '\x1b[2;8Hlast';
  const after = '\x1b[6n-continued';
  await writeScreen(expected, before);
  await resizeScreen(expected, 16, 4);
  await writeScreen(expected, after);
  await Promise.all([
    writeScreen(current, before),
    resizeScreen(current, 16, 4),
    writeScreen(current, after),
  ]);
  assert.deepEqual(terminalFrame(current), terminalFrame(expected));
  assert.deepEqual(current.replies, expected.replies);
});

test('projects fragmented ANSI, wide characters, alternate screens and scrollback', async () => {
  const screen = createTerminal();
  fitTerminal(screen, 12, 4);
  await writeScreen(screen, 'initial\r\x1b[');
  await writeScreen(screen, '2K\x1b[31;44m中\x1b[0mA');
  assert.deepEqual(terminalFrame(screen).lines[0], [
    [0, 2, '中', 0xcc0000, 0x3465a4, 0], [2, 1, 'A', null, null, 0],
  ]);
  assert.equal(line(createTerminal(), 0), '');

  await writeScreen(screen, '\x1b[?1049h\x1b[Halt\x1b[?1h\x1b[?25l');
  assert.equal(line(screen, 0), 'alt');
  assert.equal(terminalMode(screen, 'applicationCursorKeysMode'), true);
  assert.equal(terminalFrame(screen).cursor, null);
  await writeScreen(screen, '\x1b[?1049l\x1b[?25h\x1b[5 q');
  assert.equal(line(screen, 0), '中A');
  assert.deepEqual(terminalFrame(screen).cursor, [3, 0, 5]);

  await writeScreen(screen, '\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6');
  assert.equal(line(screen, 3), '6');
  terminalScroll(screen, -5);
  assert.equal(line(screen, 0), '中A');
  assert.equal(terminalFrame(screen).cursor, null);
});

test('serializes terminal replies, resize, raw input and bracketed paste', async () => {
  const commands = [];
  globalThis.window = { __TAURI__: { core: { invoke: async (name, args) => {
    await new Promise(resolve => setImmediate(resolve));
    commands.push([name, args]);
  } } } };
  const screen = createTerminal();
  await writeScreen(screen, 'abc\x1b[6n\x1b[?2004h');
  bindTerminal(screen, 'first');
  fitTerminal(screen, 12, 4);
  fitTerminal(screen, 12, 4);
  terminalInput(screen, '\x03');
  terminalPaste(screen, '你好\nnext\x1b[201~');
  await screen.commands;
  assert.deepEqual(commands, [
    ['write_terminal', { terminalId: 'first', data: '\x1b[1;4R' }],
    ['resize_terminal', { terminalId: 'first', cols: 12, rows: 4 }],
    ['write_terminal', { terminalId: 'first', data: '\x03' }],
    ['write_terminal', { terminalId: 'first', data: '\x1b[200~你好\rnext\x1b[201~' }],
  ]);
  terminalInput(screen, 'discard this');
  unbindTerminal(screen);
  await screen.commands;
  assert.equal(commands.length, 4);
});

test('selects full glyphs and joins wrapped rows while retaining real newlines', async () => {
  const screen = createTerminal();
  fitTerminal(screen, 8, 4);
  await writeScreen(screen, 'alpha 中Z\r\nsecond');
  terminalSelectStart(screen, 6, 2, 1, false, false);
  terminalSelectMove(screen, 6, 0);
  assert.equal(terminalCopy(screen), '中Z\nsecond');
  const first = terminalFrame(screen).lines[0];
  assert.deepEqual(first.find(run => run[2] === '中'), [6, 2, '中', null, null, 64]);
  assert.equal(first.find(run => run[2] === 'alpha ')[5], 0);

  terminalSelectStart(screen, 1, 0, 2, false, false);
  assert.equal(terminalCopy(screen), 'alpha');
  terminalSelectStart(screen, 0, 1, 3, false, false);
  assert.equal(terminalCopy(screen), 'alpha 中Z');
  terminalSelectStart(screen, 1, 0, 1, false, true);
  terminalSelectMove(screen, 3, 2);
  assert.equal(terminalCopy(screen), 'lp\n\nec');

  terminalSelectAll(screen);
  assert.equal(terminalCopy(screen), 'alpha 中Z\nsecond\n');
  fitTerminal(screen, 8, 3);
  assert.equal(terminalCopy(screen), '');
});

test('retains the untrimmed part of a selection and clears it on a buffer switch', async () => {
  const screen = createTerminal();
  screen.engine.options.scrollback = 0;
  fitTerminal(screen, 8, 4);
  await writeScreen(screen, 'old1\r\nold2\r\nold3\r\nold4');
  terminalSelectStart(screen, 0, 0, 1, false, false);
  terminalSelectMove(screen, 4, 2);
  await writeScreen(screen, '\r\nnew1');
  assert.equal(terminalCopy(screen), 'old2\nold3');
  await writeScreen(screen, '\x1b[?1049h');
  assert.equal(terminalCopy(screen), '');
});

test('uses xterm mouse mode restrictions, SGR encodings and focus reports', async () => {
  const input = [];
  globalThis.window = { __TAURI__: { core: { invoke: async (_, { data }) => input.push(data) } } };
  const screen = createTerminal();
  bindTerminal(screen, 'mouse');
  terminalMouse(screen, 1, 2, 9, 15, 0, 1, 0);
  await writeScreen(screen, '\x1b[?1002h\x1b[?1006h\x1b[?1004h');
  terminalFocus(screen, true);
  terminalMouse(screen, 1, 2, 9, 15, 0, 1, 0);
  terminalMouse(screen, 2, 2, 16, 15, 0, 32, 0);
  terminalMouse(screen, 2, 2, 16, 15, 0, 32, 0);
  terminalMouse(screen, 2, 2, 16, 15, 0, 0, 0);
  terminalMouse(screen, 3, 1, 23, 8, 4, 1, 0);
  await writeScreen(screen, '\x1b[?1016h');
  terminalMouse(screen, 1, 2, 9, 15, 0, 1, 1);
  terminalFocus(screen, false);
  terminalFocus(screen, false);
  await screen.commands;
  assert.deepEqual(input, [
    '\x1b[I', '\x1b[<0;2;3M', '\x1b[<32;3;3M', '\x1b[<0;3;3m', '\x1b[<65;4;2M', '\x1b[<16;9;15M', '\x1b[O',
  ]);
});

test('sends one wheel report or an application cursor key when the mode requests it', async () => {
  globalThis.WheelEvent ??= { DOM_DELTA_PIXEL: 0, DOM_DELTA_LINE: 1, DOM_DELTA_PAGE: 2 };
  const input = [];
  globalThis.window = { __TAURI__: { core: { invoke: async (_, { data }) => input.push(data) } } };
  const screen = createTerminal();
  bindTerminal(screen, 'wheel');
  const wheel = (delta, precise = false, modifiers = 0) => terminalWheel(screen, 1, 2, 9, 15, delta, 16, precise, modifiers);
  assert.equal(wheel(1), false);
  await writeScreen(screen, '\x1b[?1049h\x1b[?1h');
  assert.equal(wheel(-4), true);
  assert.equal(wheel(1, false, 4), true);
  await writeScreen(screen, '\x1b[?1000h\x1b[?1006h');
  assert.equal(wheel(4), true);
  assert.equal(wheel(-3, true), true);
  await screen.commands;
  assert.deepEqual(input, ['\x1bOA', '\x1b[<65;2;3M']);
});
