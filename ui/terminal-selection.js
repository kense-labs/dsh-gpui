const compare = (a, b) => a[1] - b[1] || a[0] - b[0];

function logicalLine(buffer, cols, row) {
  let first = row;
  let last = row;
  while (first > 0 && buffer.getLine(first)?.isWrapped) first--;
  while (buffer.getLine(last + 1)?.isWrapped) last++;
  return [[0, first], [cols, last]];
}

function wordAt(buffer, cols, [col, row], separators) {
  const [start, end] = logicalLine(buffer, cols, row);
  const glyphAt = index => {
    const line = buffer.getLine(Math.floor(index / cols));
    const col = index % cols;
    const offset = line.getCell(col).getWidth() === 0 ? -1 : 0;
    const cell = line.getCell(col + offset);
    return { start: index + offset, width: cell.getWidth(), text: cell.getChars() || ' ' };
  };
  const limit = (end[1] + 1) * cols;
  const glyph = glyphAt(row * cols + Math.min(col, cols - 1));
  const matches = glyph.text === ' '
    ? text => text === ' '
    : text => text !== ' ' && !separators.includes(text);
  let first = glyph.start;
  let last = glyph.start + glyph.width;
  while (first > start[1] * cols && matches(glyphAt(first - 1).text)) first = glyphAt(first - 1).start;
  while (last < limit && matches(glyphAt(last).text)) last += glyphAt(last).width;
  return [[first % cols, Math.floor(first / cols)], [(last - 1) % cols + 1, Math.floor((last - 1) / cols)]];
}

export class TerminalSelection {
  constructor(engine) {
    this.engine = engine;
    this.clear();
    // A buffer switch and a height change clear xterm's selection. Trimming
    // scrollback moves the remaining selection by the number of removed rows.
    let rows = engine.rows;
    engine.onResize(size => {
      if (rows !== size.rows) this.clear();
      rows = size.rows;
    });
    this.observeTrim();
    engine.buffer.onBufferChange(() => {
      this.clear();
      this.observeTrim();
    });
  }

  observeTrim() {
    this.trim?.dispose();
    this.trim = this.engine._core._bufferService.buffer.lines.onTrim(amount => {
      if (this.anchor === null) return;
      for (const [start, end] of [this.anchor, this.head]) {
        start[1] -= amount;
        end[1] -= amount;
      }
      if (this.anchor[1][1] < 0 && this.head[1][1] < 0) this.clear();
    });
  }

  clear() {
    this.anchor = null;
    this.head = null;
    this.kind = 'normal';
  }

  pointRange(col, row) {
    const { cols, buffer: { active: buffer }, options } = this.engine;
    row = Math.max(0, Math.min(buffer.length - 1, buffer.viewportY + row));
    col = Math.max(0, Math.min(cols, col));
    if (this.kind === 'word') return wordAt(buffer, cols, [col, row], options.wordSeparator);
    if (this.kind === 'line') return logicalLine(buffer, cols, row);
    if (this.kind !== 'column' && buffer.getLine(row)?.getCell(col)?.getWidth() === 0) col++;
    return [[col, row], [col, row]];
  }

  start(col, row, clicks, extend, column) {
    if (extend && this.anchor !== null) {
      this.head = this.pointRange(col, row);
      return;
    }
    this.kind = clicks === 2 ? 'word' : clicks >= 3 ? 'line' : column ? 'column' : 'normal';
    this.anchor = this.pointRange(col, row);
    this.head = this.anchor.map(point => point.slice());
  }

  move(col, row, edge) {
    if (edge !== 0 && this.kind !== 'column') col = edge < 0 ? 0 : this.engine.cols;
    if (this.anchor !== null) this.head = this.pointRange(col, row);
  }

  all() {
    this.clear();
    this.kind = 'all';
  }

  range() {
    const { cols, rows, buffer: { active: buffer } } = this.engine;
    if (this.kind === 'all') return [[0, 0], [cols, buffer.baseY + rows - 1]];
    if (this.anchor === null) return null;
    const [a, b] = this.anchor;
    const [c, d] = this.head;
    if (this.kind === 'column') {
      if (a[0] === c[0]) return null;
      return [[Math.min(a[0], c[0]), Math.max(0, Math.min(a[1], c[1]))],
        [Math.max(a[0], c[0]), Math.max(a[1], c[1])]];
    }
    const start = compare(a, c) <= 0 ? a : c;
    const end = compare(b, d) >= 0 ? b : d;
    if (compare(start, end) === 0) return null;
    return [start[1] < 0 ? [0, 0] : start, end];
  }

  columns(row, range = this.range()) {
    if (range === null) return null;
    const [[startCol, startRow], [endCol, endRow]] = range;
    if (row < startRow || row > endRow) return null;
    if (this.kind === 'column') return [startCol, endCol];
    return [row === startRow ? startCol : 0, row === endRow ? endCol : this.engine.cols];
  }

  text(windows) {
    const range = this.range();
    if (range === null) return '';
    const [start, end] = range;
    const buffer = this.engine.buffer.active;
    const output = [];
    for (let row = start[1]; row <= end[1]; row++) {
      const line = buffer.getLine(row);
      const text = line?.translateToString(true, ...this.columns(row, range)) ?? '';
      if (row > start[1] && this.kind !== 'column' && line?.isWrapped) output[output.length - 1] += text;
      else output.push(text);
    }
    return output.join(windows ? '\r\n' : '\n').replaceAll('\u00a0', ' ');
  }
}
