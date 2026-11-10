import assert from 'node:assert/strict';
import test from 'node:test';
import { HighlightSession } from '../ui/document-highlight.js';

function coloredText(line, spans, color) {
  const bytes = Buffer.from(line);
  return spans.filter(span => span.color === color)
    .map(span => bytes.subarray(span.start, span.end).toString());
}

test('maps TextMate runs after non-ASCII text to UTF-8 byte ranges', async () => {
  const source = 'let 中 = "你好"; let number = 42;';
  const [line] = await new HighlightSession().highlight(source, 'rust');
  assert.deepEqual(coloredText(source, line, 'keyword').filter(text => text.trim() === 'let'), ['let', 'let']);
  assert.deepEqual(coloredText(source, line, 'string-expression'), ['"你好"']);
  assert.deepEqual(coloredText(source, line, 'constant'), ['42']);
});

test('continues multiline syntax across pages and resets after a replacement', async () => {
  const session = new HighlightSession();
  const first = 'fn main() {\n    /* open';
  await session.highlight(first, 'rust');
  const next = ['     * still a comment', '     */', '    let resumed = 42;', '}'];
  const full = await session.highlight([first, ...next].join('\n'), 'rust');
  assert.match(coloredText(next[0], full[2], 'comment').join(''), /still a comment/);
  assert.deepEqual(coloredText(next[2], full[4], 'constant'), ['42']);

  const replacement = 'fn replacement() {}';
  const reset = await session.highlight(replacement, 'rust');
  assert.equal(reset.length, 1);
  assert.deepEqual(coloredText(replacement, reset[0], 'function'), ['replacement']);
});
