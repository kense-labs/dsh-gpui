import init, { start } from './pkg/dsh_gpui_ui.js';

const boot = document.getElementById('boot');

try {
  await init();
  start();
  boot?.remove();
} catch (error) {
  if (boot) {
    boot.textContent = `客户端启动失败：${error instanceof Error ? error.message : String(error)}`;
    boot.dataset.error = 'true';
  }
  console.error(error);
}
