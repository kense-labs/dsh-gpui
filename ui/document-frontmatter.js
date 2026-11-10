// Keep YAML interpretation separate from the GPUI view, using the same parser as Harness.
export async function parseFrontmatter(source) {
  const { isMap, isNode, isScalar, parseDocument } = await import('yaml');
  const document = parseDocument(source);
  if (document.errors.length) return undefined;
  if (document.contents === null) return [];
  if (!isMap(document.contents)) return undefined;

  const text = node => {
    if (isScalar(node) && typeof node.value === 'string') return node.value.trimEnd();
    if (!isNode(node)) return '';
    // Use source ranges for numbers, collections, and aliases. No alias is expanded.
    const [start, end] = node.range ?? [0, 0];
    const indent = start - source.lastIndexOf('\n', start - 1) - 1;
    return source.slice(start, end).replace(new RegExp(`\\n {0,${indent}}`, 'gu'), '\n').trimEnd();
  };
  return document.contents.items.map(({ key, value }) => ({
    key: text(key),
    value: text(value),
    block: isNode(value) && !isScalar(value),
  }));
}
