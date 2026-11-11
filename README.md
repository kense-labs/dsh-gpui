# KWork

English · [简体中文](README.zh.md) · [MIT License](LICENSE)

KWork is a desktop client for DeepSeek Harness ACP. Its Rust interface uses KUI and GPUI, compiles to WebAssembly, and runs in a Tauri WebView. Tauri also manages the native window, system tray, and local Harness processes.

## Features

- Organize conversations by workspace, search local history, and create, resume, or archive sessions.
- Configure DeepSeek or another OpenAI-compatible model provider. Switch models and reasoning effort from the composer.
- Send and cancel tasks, inspect tool calls and results, and respond to one-time permission requests.
- Create, edit, pause, and run scheduled tasks. Delivery history opens the session that received the task.
- Browse and manage plugins, configure web search, and use the Simplified Chinese interface in light or dark mode.

## Quick start

Install Node.js 24.9 or later, pnpm 12.6.0, Rust stable with `rustup`, and the Tauri prerequisites for your platform. On macOS, install the Xcode Command Line Tools and `just`.

Clone KWork and its companion [dsh-acp](https://github.com/kense-labs/dsh-acp) service into the same parent directory.

```sh
git clone https://github.com/kense-labs/kwork.git
git clone https://github.com/kense-labs/dsh-acp.git
cd dsh-acp
pnpm install
pnpm run build
cd ../kwork
npm ci
```

On macOS, run `just dev` to build and launch `.tools/dev-app/KWork.app`. On other platforms, use `npm run dev` to start the Tauri development client.

You can add a DeepSeek API key on first launch or select **Set up later** (`稍后配置`) to enter the workspace. To manage the key later, open **More → Settings → API Key** from the sidebar. KWork also accepts the `DEEPSEEK_API_KEY` environment variable and uses the `deepseek-official` route for that key.

## Model providers

Open **Settings → Model Providers** (`设置 → 模型服务`) to choose a preset for SiliconFlow, DeepSeek, OpenRouter, OpenAI, or Ollama. For a custom provider, choose Chat Completions or Responses, enter the API base URL and any required key, then sync the model list or enter a model ID. Select a saved model from the composer menu.

The Tauri host writes keys through the local Harness credential service. The frontend reads credential status. Harness stores credentials in `.credentials.yaml` under its home directory.

## Configuration

| Variable | Purpose |
| --- | --- |
| `DSH_ACP_PATH` | Path to the `dsh-acp` checkout. The default lookup includes `../dsh-acp`. |
| `DSH_NODE` | Path to the Node.js executable used by the host. |
| `DSH_HOME` | Harness data directory. Defaults to `~/.dsh`. |

The workspace directory must exist on the machine running KWork. ACP uses its canonical absolute path. Resuming a session reattaches to the Harness agent. KWork loads persisted history separately from the ACP update stream.

## Build and verify

```sh
npm run ui:build
cargo fmt --all --check
cargo test --workspace
node --test scripts/*.test.mjs
```

`npm run build` builds the release host executable. To package a macOS application, build the frontend first and run `npm exec -- tauri build --bundles app`. The frontend build installs the `wasm-bindgen-cli` version pinned in `Cargo.lock` under `.tools/`.

| Path | Contents |
| --- | --- |
| `ui/` | Rust UI, WebAssembly entry point, and assets |
| `src-tauri/` | Native shell, IPC, ACP process management, and application icons |
| `services/` | Credential and ACP bridges, plus bundled Harness services |
| `scripts/` | Build tools and JavaScript tests |

## Project status

The macOS development build, API key management, ACP automation delivery, delivery-history navigation, and session archival and restoration have been tested. Cold delivery and reserved sessions require the companion `dsh-acp` build with `ctx.acpSessionHost`. Scheduled sessions with an explicit Agent preset, full workspace management, and visual and animation parity with DeepSeek Harness remain in progress.

## License

KWork is licensed under the [MIT License](LICENSE). Bundled third-party components retain their own license notices.

- DeepSeek Harness [assets](ui/assets/DeepSeek-LICENSE.txt), [Schedule and Workspace runtime](services/vendor/dsh-schedule/LICENSES.txt), and [Ask User tool](services/vendor/dsh-tool-ask-user/LICENSE)
- [GPUI renderer](vendor/gpui-pre-wgpu/LICENSE-APACHE)
- [Noto Sans](ui/assets/NotoSans-LICENSE) and [xterm.js](ui/assets/xterm-LICENSE.txt)
