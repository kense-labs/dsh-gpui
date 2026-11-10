import assert from 'node:assert/strict';
import test from 'node:test';
import { parseFrontmatter } from '../ui/document-frontmatter.js';

test('retains field order and the authored values of non-string YAML nodes', async () => {
  const source = [
    'title: >-', '  多行', '  text', '10: ten', '2: two',
    'version: 1.0', 'tags: &list [one, two]', 'copy: *list', 'empty:',
  ].join('\r\n');
  assert.deepEqual(await parseFrontmatter(source), [
    { key: 'title', value: '多行 text', block: false },
    { key: '10', value: 'ten', block: false },
    { key: '2', value: 'two', block: false },
    { key: 'version', value: '1.0', block: false },
    { key: 'tags', value: '[one, two]', block: true },
    { key: 'copy', value: '*list', block: true },
    { key: 'empty', value: '', block: false },
  ]);
});

test('distinguishes empty metadata from YAML that needs a verbatim fallback', async () => {
  assert.deepEqual(await parseFrontmatter('# only a comment'), []);
  for (const source of ['x: [unterminated', 'x: 1\nx: 2', '~', '- first\n- second']) {
    assert.equal(await parseFrontmatter(source), undefined, source);
  }
});
