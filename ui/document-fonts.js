export function prepareDocumentFonts(source) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./document-fonts-worker.js', import.meta.url), { type: 'module' });
    worker.onmessage = ({ data }) => {
      worker.terminate();
      if (data.fonts) resolve(data.fonts);
      else reject(new Error(data.error));
    };
    worker.onerror = event => {
      worker.terminate();
      reject(new Error(event.message));
    };
    worker.postMessage(source, [source.buffer]);
  });
}
