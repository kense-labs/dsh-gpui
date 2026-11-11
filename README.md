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

`npm run build` builds the release host executable. To package a macOS application, build the frontend first and run `npm exec -- tauri build --bundles app`. The frontend build installs the `wasm-bindgen-cli` version pinned in `Cargo.lock` and a platform-matched `wasm-opt` under `.tools/`, without adding them to `package.json`.

| Path | Contents |
| --- | --- |
| `ui/` | Rust UI, WebAssembly entry point, and assets |
| `src-tauri/` | Native shell, IPC, ACP process management, and application icons |
| `services/` | Credential and ACP bridges, plus bundled Harness services |
| `scripts/` | Build tools and JavaScript tests |

## Build speed and bundle size

### blaze (native host dev loop only)

[blaze](https://github.com/indexable-inc/blaze) is a resident incremental Rust compiler: replacing `cargo` with `blaze` rebuilds only the functions you edited. It is **not** a size optimiser — it refuses `lto`, does no cross-function optimisation, and its wasm32 modules are documented as 8–50x larger than rustc's. KWork therefore keeps the WebAssembly frontend and every release build on cargo; blaze is only an optional accelerator for the native host dev loop.

```sh
curl -fsSL https://ix.dev/blaze | sh       # ~/.cargo/bin must be on PATH
just host-blaze                            # build the native host only
just dev-blaze                             # build and launch the macOS dev app
```

`scripts/blaze-build.mjs` probes compatibility once per (blaze version, `Cargo.lock`, platform) and caches the answer in `.tools/blaze-compat.json`. A failed probe falls back to `cargo build -p dsh-gpui` permanently, so the default `just dev` never gets slower or breaks. Re-probe after an upstream fix with `KWORK_BLAZE=force node scripts/blaze-build.mjs`; `KWORK_BLAZE=off` forces cargo. blaze v2 writes outside the checkout by default and needs `KWORK_BLAZE_ARGS='--target-dir target'`; set `BLAZE_SANDBOX=off` in containers where the sandbox is unavailable.

**Measured on macOS arm64 (2026-11-11)**: stable `0.20261027.0` cannot compile the `objc2-exception-helper` build script while loading the workspace; canary `0.20261105.4` fails on proc-macro expansion in `icu_collections` / `icu_provider` / `cargo_metadata`. Do not use `blaze enable`, which changes the user-level `~/.cargo/config.toml`: KWork builds with stable 1.99.1 while blaze uses its own nightly-2026-10-27, so in wrapper mode the daemon links zero programs and incremental builds are slower than plain cargo. `cargo check` and CI remain the final word.

### Bundle size

The WebAssembly frontend is KWork's largest artifact. `npm run ui:build` applies three steps:

1. `[profile.release-wasm]` in `Cargo.toml` (inherits release, `opt-level = "z"`, `lto = true`, `panic = "abort"`, `strip = false`). `strip = false` keeps the `target_features` custom section; without it wasm-bindgen cannot detect the bulk-memory / reference-types features rustc enabled.
2. `wasm-opt -Oz` on the wasm-bindgen output — the replacement recommended by wasm-bindgen's own `strip = true` warning — with a content-addressed cache in `.tools/wasm-opt-cache/`.
3. Content-based incremental sync into `ui/dist`: unchanged files are never rewritten and only genuinely stale content-hashed chunks are removed.

Measured on this machine:

| Artifact | Before | After |
| --- | ---: | ---: |
| `ui/dist/pkg/dsh_gpui_ui_bg.wasm` | 18,650,990 B | 13,413,708 B (−28.1%) |
| `ui/dist/` total | 23 MB | 18 MB (−22%) |

The native host's `[profile.release]` keeps `lto = "thin"` + `codegen-units = 1` + `strip = true`. Set `KWORK_WASM_OPT=off` to skip optimisation during development. The optimised wasm passes `WebAssembly.validate`, instantiates, and keeps all 28 exports of the unoptimised wasm-bindgen output.

## Project status

The macOS development build, API key management, ACP automation delivery, delivery-history navigation, and session archival and restoration have been tested. Cold delivery and reserved sessions require the companion `dsh-acp` build with `ctx.acpSessionHost`. Scheduled sessions with an explicit Agent preset, full workspace management, and visual and animation parity with DeepSeek Harness remain in progress.

## License

KWork is licensed under the [MIT License](LICENSE). Bundled third-party components retain their own license notices.

- DeepSeek Harness [assets](ui/assets/DeepSeek-LICENSE.txt), [Schedule and Workspace runtime](services/vendor/dsh-schedule/LICENSES.txt), and [Ask User tool](services/vendor/dsh-tool-ask-user/LICENSE)
- [GPUI renderer](vendor/gpui-pre-wgpu/LICENSE-APACHE)
- [Noto Sans](ui/assets/NotoSans-LICENSE) and [xterm.js](ui/assets/xterm-LICENSE.txt)
