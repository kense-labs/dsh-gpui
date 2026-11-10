import subsetWasm from 'harfbuzzjs/dist/harfbuzz-subset.wasm';

const tag = name => Array.from(name).reduce((value, character) => value * 256 + character.charCodeAt(0), 0);
const padded = length => Math.ceil(length / 4) * 4;
const styleNames = { 400: 'Regular', 500: 'Medium', 600: 'Semibold', 700: 'Bold' };

// The document's prose is 12–18px and its large headings can reach 25px.
// macOS clamps the system font's optical size to 17 for text below that size.
const instances = [17, 18].flatMap(size => [400, 500, 600, 700].map(weight => [size, weight]));
for (let size = 19; size <= 25; size++) instances.push([size, 700]);

function checksum(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let sum = 0;
  for (let offset = 0; offset < bytes.length; offset += 4) sum = (sum + view.getUint32(offset)) >>> 0;
  return sum;
}

function instanceNames(table, size, weight) {
  const original = new DataView(table.buffer, table.byteOffset, table.byteLength);
  const family = `DSH System ${size}`;
  const style = styleNames[weight];
  const names = new Map([
    [1, family], [2, style], [4, `${family} ${style}`],
    [6, `DSHSystem${size}-${style}`], [16, family], [17, style],
  ]);
  const count = original.getUint16(2);
  const storage = original.getUint16(4);
  const edits = [];
  let length = table.length;
  for (let index = 0; index < count; index++) {
    const offset = 6 + 12 * index;
    const platform = original.getUint16(offset);
    const value = names.get(original.getUint16(offset + 6));
    if (!value || ![0, 1, 3].includes(platform)) continue;
    const unicode = platform !== 1;
    const encoded = Uint8Array.from(Array.from(value).flatMap(character =>
      unicode ? [0, character.charCodeAt(0)] : [character.charCodeAt(0)],
    ));
    edits.push({ offset, at: length, encoded });
    length += encoded.length;
  }
  if (length - storage > 0xffff) throw new Error('Font naming table exceeds its offset range');
  const output = new Uint8Array(padded(length));
  output.set(table);
  const view = new DataView(output.buffer);
  for (const { offset, at, encoded } of edits) {
    view.setUint16(offset + 8, encoded.length);
    view.setUint16(offset + 10, at - storage);
    output.set(encoded, at);
  }
  return { output, length };
}

function nameInstance(bytes, size, weight) {
  const font = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let nameRecord, head;
  for (let index = 0; index < font.getUint16(4); index++) {
    const record = 12 + 16 * index;
    if (font.getUint32(record) === tag('name')) nameRecord = record;
    if (font.getUint32(record) === tag('head')) head = font.getUint32(record + 8);
  }
  if (nameRecord === undefined || head === undefined) throw new Error('Missing font metadata');
  const offset = font.getUint32(nameRecord + 8);
  const table = bytes.subarray(offset, offset + font.getUint32(nameRecord + 12));
  // Keep copyright and license records in the original name table.
  const { output: names, length } = instanceNames(table, size, weight);
  const end = padded(bytes.length);
  const output = new Uint8Array(end + names.length);
  output.set(bytes);
  output.set(names, end);
  const view = new DataView(output.buffer);
  view.setUint32(nameRecord + 4, checksum(names));
  view.setUint32(nameRecord + 8, end);
  view.setUint32(nameRecord + 12, length);
  view.setUint32(head + 8, 0);
  view.setUint32(head + 8, (0xb1b0afba - checksum(output)) >>> 0);
  return output;
}

function instance(hb, face, size, weight) {
  const input = hb.hb_subset_input_create_or_fail();
  if (!input) throw new Error('Unable to allocate font instance');
  let subset = 0;
  let blob = 0;
  try {
    hb.hb_subset_input_keep_everything(input);
    if (!hb.hb_subset_input_pin_all_axes_to_default(input, face)
      || !hb.hb_subset_input_pin_axis_location(input, face, tag('opsz'), size)
      || !hb.hb_subset_input_pin_axis_location(input, face, tag('wght'), weight)) {
      throw new Error('System font lacks the required variation axes');
    }
    subset = hb.hb_subset_or_fail(face, input);
    if (!subset) throw new Error('Unable to instantiate system font');
    blob = hb.hb_face_reference_blob(subset);
    const data = hb.hb_blob_get_data(blob, 0);
    const bytes = new Uint8Array(hb.memory.buffer, data, hb.hb_blob_get_length(blob));
    return nameInstance(bytes, size, weight);
  } finally {
    if (blob) hb.hb_blob_destroy(blob);
    if (subset) hb.hb_face_destroy(subset);
    hb.hb_subset_input_destroy(input);
  }
}

self.onmessage = async ({ data: source }) => {
  try {
    const response = await fetch(new URL(subsetWasm, import.meta.url));
    if (!response.ok) throw new Error('Unable to load the font instancer');
    const { instance: wasm } = await WebAssembly.instantiate(await response.arrayBuffer());
    const hb = wasm.exports;
    hb._initialize();
    const data = hb.malloc(source.length);
    if (!data) throw new Error('Unable to allocate system font bytes');
    new Uint8Array(hb.memory.buffer).set(source, data);
    // The Worker owns the readonly bytes, faces and linear memory until it exits.
    const blob = hb.hb_blob_create(data, source.length, 1, 0, 0);
    const face = hb.hb_face_create(blob, 0);
    const prepared = hb.hb_subset_preprocess(face);
    if (!prepared) throw new Error('Unable to read system font tables');
    const fonts = instances.map(([size, weight]) => instance(hb, prepared, size, weight));
    self.postMessage({ fonts }, fonts.map(font => font.buffer));
  } catch (error) {
    self.postMessage({ error: String(error) });
  }
};
