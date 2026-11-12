import { Terminal } from './xterm-headless.mjs';
import { TerminalSelection } from './terminal-selection.js';

const ansiColors = [
  0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf,
  0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
];

function palette(index) {
  if (index < 16) return ansiColors[index];
  if (index >= 232) return (8 + (index - 232) * 10) * 0x010101;
  const value = index - 16;
  const channel = level => level === 0 ? 0 : 55 + level * 40;
  return (channel(Math.floor(value / 36)) << 16) | (channel(Math.floor(value / 6) % 6) << 8) | channel(value % 6);
}

function queue(screen, command, args) {
  const id = screen.id;
  if (id === null) return;
  // Resize, terminal replies, and keystrokes must reach the same PTY in order.
  screen.commands = screen.commands.then(() => {
    if (screen.id === id) return window.__TAURI__.core.invoke(command, { terminalId: id, ...args });
  }).catch(error => {
    console.error('Terminal command failed', error);
    // Surface PTY failures in the GPUI toast layer, not just the console.
    const detail = `终端命令执行失败：${error instanceof Error ? error.message : String(error)}`;
    window.dispatchEvent(new CustomEvent('kwork-runtime-error', { detail }));
  });
}

export function createTerminal() {
  const engine = new Terminal({ cols: 80, rows: 24, scrollback: 1000, allowProposedApi: true });
  const selection = new TerminalSelection(engine);
  const screen = {
    engine, selection, id: null, commands: Promise.resolve(), replies: [],
    cols: engine.cols, rows: engine.rows, pendingWrites: 0,
    cursorVisible: true, cursorStyle: 0, mouseEvents: 0,
  };
  engine._core.coreMouseService.onProtocolChange(events => {
    screen.mouseEvents = events;
    if (events !== 0) selection.clear();
  });
  engine.onData(data => {
    if (screen.restoring) return;
    if (screen.id !== null) queue(screen, 'write_terminal', { data });
    else if (screen.replies.length < 64) screen.replies.push(data);
  });
  for (const [final, visible] of [['h', true], ['l', false]]) {
    engine.parser.registerCsiHandler({ prefix: '?', final }, params => {
      if (params.includes(25)) screen.cursorVisible = visible;
      return false;
    });
  }
  engine.parser.registerCsiHandler({ intermediates: ' ', final: 'q' }, params => {
    screen.cursorStyle = params[0];
    return false;
  });
  engine.parser.registerEscHandler({ final: 'c' }, () => {
    selection.clear();
    screen.cursorVisible = true;
    screen.cursorStyle = 0;
    return false;
  });
  return screen;
}

export function bindTerminal(screen, id) {
  screen.id = id;
  for (const data of screen.replies.splice(0)) queue(screen, 'write_terminal', { data });
}

export function unbindTerminal(screen) {
  screen.id = null;
  screen.replies.length = 0;
}

export function writeScreen(screen, data, afterWrite) {
  screen.pendingWrites += 1;
  return new Promise(resolve => screen.engine.write(data, () => {
    screen.pendingWrites -= 1;
    afterWrite?.();
    resolve();
  }));
}

export async function restoreTerminal(screen, cols, rows, snapshot) {
  screen.restoring = true;
  try {
    screen.selection.clear();
    screen.replies.length = 0;
    screen.cursorVisible = true;
    screen.cursorStyle = 0;
    screen.engine.reset();
    await resizeScreen(screen, cols, rows);
    if (snapshot !== '') await writeScreen(screen, snapshot);
  } finally {
    screen.restoring = false;
  }
}

export function resizeScreen(screen, cols, rows) {
  screen.cols = cols;
  screen.rows = rows;
  // xterm parses writes asynchronously. Keep the old grid until earlier output is applied.
  if (screen.pendingWrites !== 0) {
    return writeScreen(screen, '', () => screen.engine.resize(cols, rows));
  }
  screen.engine.resize(cols, rows);
  return Promise.resolve();
}

export function fitTerminal(screen, cols, rows) {
  if (screen.cols === cols && screen.rows === rows) return Promise.resolve();
  const resized = resizeScreen(screen, cols, rows);
  queue(screen, 'resize_terminal', { cols, rows });
  return resized;
}

export function terminalInput(screen, data) {
  if (screen.id !== null) {
    screen.selection.clear();
    screen.engine.input(data, true);
  }
}

export function terminalPaste(screen, text) {
  const data = text.replace(/\r?\n/g, '\r');
  terminalInput(screen, screen.engine.modes.bracketedPasteMode
    ? `\x1b[200~${data.replaceAll('\x1b[201~', '')}\x1b[201~`
    : data);
}

export function terminalScroll(screen, lines) {
  screen.engine.scrollLines(lines);
}

export function terminalMode(screen, mode) {
  return Boolean(screen.engine.modes[mode]);
}

export function terminalSelectStart(screen, col, row, clicks, extend, column) {
  screen.selection.start(col, row, clicks, extend, column);
}

export function terminalSelectMove(screen, col, row, edge = 0) {
  screen.selection.move(col, row, edge);
}

export function terminalSelectAll(screen) {
  screen.selection.all();
}

export function terminalCopy(screen) {
  return screen.selection.text(globalThis.navigator?.platform?.startsWith('Win') ?? false);
}

export function terminalMouseActive(screen) {
  return screen.id !== null && screen.engine.modes.mouseTrackingMode !== 'none';
}

export function terminalFocus(screen, focused) {
  const changed = screen.focused !== focused;
  screen.focused = focused;
  if (changed && screen.engine.modes.sendFocusMode) {
    queue(screen, 'write_terminal', { data: focused ? '\x1b[I' : '\x1b[O' });
  }
}

export function terminalMouse(screen, col, row, x, y, button, action, modifiers) {
  if (!terminalMouseActive(screen)) return;
  // The pinned engine owns the same mode restrictions, deduplication and
  // X10/SGR/SGR-pixel encoding used by the browser xterm frontend.
  screen.engine._core.coreMouseService.triggerMouseEvent({
    col, row, x, y, button, action,
    ctrl: (modifiers & 1) !== 0, alt: (modifiers & 2) !== 0, shift: (modifiers & 4) !== 0,
  });
}

export function terminalWheel(screen, col, row, x, y, delta, cellHeight, precise, modifiers) {
  if (screen.id === null) return false;
  const mouseWheel = (screen.mouseEvents & 16) !== 0;
  if (!mouseWheel && screen.engine._core._bufferService.buffer.hasScrollback) return false;
  const lines = screen.engine._core.coreMouseService.consumeWheelEvent({
    deltaY: delta, deltaMode: precise ? 0 : 1,
    ctrlKey: (modifiers & 1) !== 0, altKey: (modifiers & 2) !== 0, shiftKey: (modifiers & 4) !== 0,
  }, cellHeight, 1);
  if (lines !== 0) {
    if (mouseWheel) terminalMouse(screen, col, row, x, y, 4, delta < 0 ? 0 : 1, modifiers);
    else terminalInput(screen, `\x1b${screen.engine.modes.applicationCursorKeysMode ? 'O' : '['}${delta < 0 ? 'A' : 'B'}`);
  }
  return true;
}

export function terminalFrame(screen) {
  const { engine } = screen;
  const buffer = engine.buffer.active;
  const selection = screen.selection.range();
  const lines = [];
  let cell;
  for (let y = 0; y < engine.rows; y++) {
    const row = buffer.viewportY + y;
    const line = buffer.getLine(row);
    const selected = screen.selection.columns(row, selection);
    const runs = [];
    if (line) {
      for (let x = 0; x < engine.cols; x++) {
        cell = line.getCell(x, cell);
        const width = cell.getWidth();
        if (width === 0) continue;
        let text = cell.isInvisible() ? '' : cell.getChars();
        const fgIndex = cell.getFgColor();
        const fg = cell.isFgPalette() ? palette(fgIndex < 8 && cell.isBold() ? fgIndex + 8 : fgIndex)
          : cell.isFgRGB() ? fgIndex : null;
        const bg = cell.isBgPalette() ? palette(cell.getBgColor()) : cell.isBgRGB() ? cell.getBgColor() : null;
        const inSelection = selected !== null && x < selected[1] && x + width > selected[0];
        const flags = Number(Boolean(cell.isBold())) | (Number(Boolean(cell.isItalic())) << 1)
          | (Number(Boolean(cell.isUnderline())) << 2) | (Number(Boolean(cell.isStrikethrough())) << 3)
          | (Number(Boolean(cell.isInverse())) << 4) | (Number(Boolean(cell.isDim())) << 5)
          | (Number(inSelection) << 6);
        if (text === '' && bg === null && (flags & 0b1011100) === 0) continue;
        text ||= ' '.repeat(width);
        const previous = runs.at(-1);
        // Keep wide and combined glyphs independently positioned in the GPUI grid.
        if (width === 1 && text.length === 1 && previous && previous[2].length === previous[1]
          && previous[0] + previous[1] === x && previous[3] === fg && previous[4] === bg && previous[5] === flags) {
          previous[1] += width;
          previous[2] += text;
        } else {
          runs.push([x, width, text, fg, bg, flags]);
        }
      }
    }
    lines.push(runs);
  }
  const cursorRow = buffer.baseY - buffer.viewportY + buffer.cursorY;
  const cursorCol = Math.min(buffer.cursorX, engine.cols - 1);
  const cursor = screen.cursorVisible && cursorRow >= 0 && cursorRow < engine.rows
    ? [cursorCol, cursorRow, screen.cursorStyle] : null;
  return { lines, cursor };
}
