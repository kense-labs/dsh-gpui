// The embedded runtime only provides the event loop needed by headless xterm.
globalThis.process = { title: 'dsh-terminal' };
globalThis.performance = { now: Date.now };
globalThis.queueMicrotask = callback => Promise.resolve().then(callback);

const timers = new Map();
let nextTimer = 0;
globalThis.setTimeout = callback => {
  const id = ++nextTimer;
  timers.set(id, callback);
  return id;
};
globalThis.clearTimeout = id => timers.delete(id);

function drainTimers() {
  for (const [id, callback] of timers) {
    timers.delete(id);
    callback();
  }
}

const screens = new Map();
globalThis.terminalScreens = {
  open(id, cols, rows) {
    const terminal = new Terminal({ cols, rows, scrollback: 1000, allowProposedApi: true });
    const serializer = new SerializeAddon();
    terminal.loadAddon(serializer);
    screens.set(id, { terminal, serializer });
  },
  write(id, data) {
    let written = false;
    screens.get(id).terminal.write(data, () => { written = true; });
    drainTimers();
    if (!written) throw new Error('Terminal output was not applied');
  },
  resize(id, cols, rows) {
    screens.get(id).terminal.resize(cols, rows);
    drainTimers();
  },
  snapshot(id) {
    return screens.get(id).serializer.serialize();
  },
  close(id) {
    const screen = screens.get(id);
    screens.delete(id);
    screen.terminal.dispose();
  },
};
