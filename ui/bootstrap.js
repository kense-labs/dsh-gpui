import init, { start } from './pkg/dsh_gpui_ui.js';

const boot = document.getElementById('boot');

function enableMacWindowMaterial() {
  if (!window.__TAURI_INTERNALS__ || !navigator.platform.startsWith('Mac')) return;
  document.documentElement.dataset.nativeMacMaterial = '';

  // wgpu 30 advertises an opaque browser surface. Its straight-alpha blend
  // still produces premultiplied pixels over its transparent clear color.
  // WebGL already uses an alpha-enabled, premultiplied drawing buffer.
  const prototype = globalThis.GPUCanvasContext?.prototype;
  if (!prototype) return;
  const configure = prototype.configure;
  prototype.configure = function (options) {
    const canvas = this.canvas;
    if (canvas.parentElement !== document.body || canvas.style.touchAction !== 'none') {
      return configure.call(this, options);
    }
    const configureSurface = (options) => configure.call(this, {
      ...options, alphaMode: 'premultiplied',
    });
    configureSurface(options);
    this.configure = configureSurface;
    prototype.configure = configure;
  };
}

try {
  await init();
  enableMacWindowMaterial();
  start();
  boot?.remove();
} catch (error) {
  if (boot) {
    boot.textContent = `客户端启动失败：${error instanceof Error ? error.message : String(error)}`;
    boot.dataset.error = 'true';
  }
  console.error(error);
}
