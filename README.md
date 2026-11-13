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

Install [Bun](https://bun.sh), Rust stable with `rustup`, and the Tauri prerequisites for your platform. On macOS, also install the Xcode Command Line Tools and `just`.

**KWork has no Node.js build or runtime dependency of its own.** The frontend build scripts (`scripts/*.mjs`), the dependency install and the Tauri CLI all run on Bun. Node.js 24.9+ / pnpm 12.6.0 are needed only for the separate [dsh-acp](https://github.com/kense-labs/dsh-acp) checkout, which KWork launches as an external stdio process together with a small credential helper; `DSH_NODE` points at that executable. Those two child processes are the only place Node appears.

Clone KWork and its companion dsh-acp service into the same parent directory.

```sh
git clone https://github.com/kense-labs/kwork.git
git clone https://github.com/kense-labs/dsh-acp.git
cd dsh-acp
pnpm install          # dsh-acp only
pnpm run build        # dsh-acp only
cd ../kwork
bun install --no-save # reads the tracked package-lock.json; keeps it the single lockfile
```

On macOS, run `just dev` to build and launch `.tools/dev-app/KWork.app`. On other platforms, use `bun run dev` to start the Tauri development client. `KWORK_BUN` can point at a Bun executable that is not on `PATH`.

You can add a DeepSeek API key on first launch or select **Set up later** (`稍后配置`) to enter the workspace. To manage the key later, open **More → Settings → API Key** from the sidebar. KWork also accepts the `DEEPSEEK_API_KEY` environment variable and uses the `deepseek-official` route for that key.

## Model providers

Open **Settings → Model Providers** (`设置 → 模型服务`) to choose a preset for SiliconFlow, DeepSeek, OpenRouter, OpenAI, or Ollama. For a custom provider, choose Chat Completions or Responses, enter the API base URL and any required key, then sync the model list or enter a model ID. Select a saved model from the composer menu.

The Tauri host writes keys through the local Harness credential service. The frontend reads credential status. Harness stores credentials in `.credentials.yaml` under its home directory.

## Configuration

| Variable | Purpose |
| --- | --- |
| `DSH_ACP_PATH` | Path to the `dsh-acp` checkout. The default lookup includes `../dsh-acp`. |
| `DSH_NODE` | Path to the Node.js executable used for the external dsh-acp / credential helper processes. Not used by the Bun build chain. |
| `DSH_HOME` | Harness data directory. Defaults to `~/.dsh`. |

The workspace directory must exist on the machine running KWork. ACP uses its canonical absolute path. Resuming a session reattaches to the Harness agent. KWork loads persisted history separately from the ACP update stream.

## Build and verify

```sh
bun run ui:build
cargo fmt --all --check
cargo test --workspace
bun test scripts/*.test.mjs
```

`bun run build` builds the release host executable. To package a macOS application, build the frontend first and run `bun x tauri build --bundles app`. The frontend build installs the `wasm-bindgen-cli` version pinned in `Cargo.lock` and a platform-matched `wasm-opt` under `.tools/`, without adding them to `package.json`.

| Path | Contents |
| --- | --- |
| `ui/` | Rust UI, WebAssembly entry point, and assets |
| `src-tauri/` | Native shell, IPC, ACP process management, and application icons |
| `services/` | Credential and ACP bridges, plus bundled Harness services |
| `scripts/` | Build tools and JavaScript tests |

## Windows

The Rust host has Windows branches for ConPTY terminals, `pwsh` / `powershell` / `cmd` shell discovery, `cmd /c start` links, `explorer.exe`, Windows system fonts, Node detection (`node.exe`, fnm, nvm, Program Files) and a native folder picker through PowerShell. `src-tauri/tauri.windows.conf.json` overrides the window size and selects the NSIS bundle target; Tauri merges it with `tauri.conf.json` on Windows.

On a Windows machine:

```sh
bun install --no-save
bun run ui:build
bun x tauri build --bundles nsis
```

`just dev` and `scripts/dev-app.sh` are macOS-only; use `bun run dev` for the development client. The app still needs a companion `dsh-acp` checkout at runtime — set `DSH_ACP_PATH` and `DSH_NODE` (for example `C:\Program Files\nodejs\node.exe`) in Settings or the environment.

`.github/workflows/windows.yml` builds the NSIS installer and the portable `target/release/KWork.exe` on a `windows-latest` runner and uploads both as the `KWork-Windows-x64` artifact.

### Embedded Node modules

`services/acp-session-bridge.mjs` and `services/credential-service.mjs` are compiled into the host binary with `include_str!`. They are **not** shipped as loose files and are no longer passed through `node --eval` (~90 KB of command line, close to the macOS `ARG_MAX` budget). `src-tauri/src/embedded_script.rs` materialises each as a private `.mjs` temp file before spawn, so stack traces point at a real path and the file is deleted when the connection is dropped. The 574 KB scheduler bundle (`services/vendor/dsh-schedule/index.mjs`) is instead content-addressed by SHA-256 in the per-user temp directory: it is written once and reused by every ACP connection and every app launch. Both scripts still read their arguments from `process.argv.slice(2)`; the script path simply replaces the former `[eval]` label at `argv[1]`.

## Build speed and bundle size

### blaze (native host dev loop only)

[blaze](https://github.com/indexable-inc/blaze) is a resident incremental Rust compiler: replacing `cargo` with `blaze` rebuilds only the functions you edited. It is **not** a size optimiser — it refuses `lto`, does no cross-function optimisation, and its wasm32 modules are documented as 8–50x larger than rustc's. KWork therefore keeps the WebAssembly frontend and every release build on cargo; blaze is only an optional accelerator for the native host dev loop.

```sh
curl -fsSL https://ix.dev/blaze | sh       # ~/.cargo/bin must be on PATH
just host-blaze                            # build the native host only
just dev-blaze                             # build and launch the macOS dev app
```

`scripts/blaze-build.mjs` probes compatibility once per (blaze version, `Cargo.lock`, platform) and caches the answer in `.tools/blaze-compat.json`. A failed probe falls back to `cargo build -p dsh-gpui` permanently, so the default `just dev` never gets slower or breaks. Re-probe after an upstream fix with `KWORK_BLAZE=force bun scripts/blaze-build.mjs`; `KWORK_BLAZE=off` forces cargo. blaze v2 writes outside the checkout by default and needs `KWORK_BLAZE_ARGS='--target-dir target'`; set `BLAZE_SANDBOX=off` in containers where the sandbox is unavailable.

**Measured on macOS arm64 (2026-11-11)**: stable `0.20261027.0` cannot compile the `objc2-exception-helper` build script while loading the workspace; canary `0.20261105.4` fails on proc-macro expansion in `icu_collections` / `icu_provider` / `cargo_metadata`. Do not use `blaze enable`, which changes the user-level `~/.cargo/config.toml`: KWork builds with stable 1.99.1 while blaze uses its own nightly-2026-10-27, so in wrapper mode the daemon links zero programs and incremental builds are slower than plain cargo. `cargo check` and CI remain the final word.

### Fast development loop (cargo, measured)

blaze cannot build this workspace, so the speedups that actually landed come from avoiding work cargo does not need to redo. Neither one changes the default path: `just dev` and `bun run build` keep the same size-optimized frontend.

**Skip unchanged frontend builds.** `bun run ui:build` fingerprints every input of `ui/dist` (`ui/**` except `dist/`, `shared/**`, `vendor/**`, manifests, lockfiles and the build script itself) into `.tools/ui-build-stamp.json`. A cheap `stat` pass over ~220 files runs first; the content hash is only computed when an mtime or size changed. An unchanged tree exits in **0.5–0.7s** instead of 2.5–3.1s, so a native-Rust-only edit in `just dev` rebuilds only the host (~1.7s). `KWORK_UI_BUILD=always` forces a rebuild.

**Iterate on the WASM frontend without LTO.** `release-wasm` uses `lto = true`, so a one-line frontend Rust edit pays for a ~20s whole-program link. `KWORK_UI_FAST=1` switches to the separate `[profile.dev-wasm]` (`opt-level = "z"`, `lto = false`, `codegen-units = 16`, `debug = 0`) and skips `wasm-opt`:

```sh
just ui-fast        # fast frontend only
just dev-fast       # build + launch the macOS dev app with it
bun run ui:build    # restore the size-optimized frontend before packaging
```

| macOS arm64, 2026-11-11 | default | `KWORK_UI_FAST=1` |
| --- | ---: | ---: |
| `bun run ui:build`, one frontend Rust line changed | 28.6s (20.7s LTO link) | 11.1s (3.0s cargo) |
| warmed, no source change | 2.5–3.1s | 0.5–0.7s (fingerprint skip) |
| `ui/dist/pkg/dsh_gpui_ui_bg.wasm` | 13,413,708 B | 33,798,006 B |

If the edit changes the wasm bytes, the default path additionally pays ~11s of `wasm-opt -Oz`; the fast path never runs it. The fast wasm is unoptimized and ~2.5x larger — local iteration only. `bun run build` runs the default `ui:build` first, so packaging always restores the optimized artifact, and switching profiles invalidates the fingerprint automatically.

**Evaluated, not adopted**: rust-lld as a faster linker (`-C linker-features=+lld` is unstable on stable 1.99.1; LLD 23.1.1 cannot parse this macOS SDK's TAPI `.tbd` files), a `debug = 0` native dev profile (alternating incremental runs: 2.58s vs 3.45s, no consistent win), and mold (no macOS support). `sccache` is not wired in either: cargo's incremental cache already covers the dev loop, and sccache mainly pays off for CI / clean rebuilds; it can be added later with `RUSTC_WRAPPER`.

### Bundle size

The WebAssembly frontend is KWork's largest artifact. `bun run ui:build` applies three steps:

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

The macOS development build, API key management, ACP automation delivery, delivery-history navigation, and session archival and restoration have been tested. The GPUI layer uses the official design tokens for both themes, the official menu enter/exit and modal timings, command result cards, provider marks and a non-flickering live reasoning shimmer. Workspaces can be added with the native directory picker (`Mod+O`), and removed groups can be restored from **Settings → General**. Host command failures, cold-resume questions and terminal PTY errors are surfaced in the UI instead of being discarded.

The UI is aligned package by package against the upstream `deepseek-ai/deepseek-harness` source (`packages/client/ui-*/README.zh.md`, 61 UI packages), currently against tag `dsh-v0.2.2-rc.3`. The per-area status (verified, approximated, unreachable over ACP) lives in [docs/parity-rc3.md](docs/parity-rc3.md). Recent passes added the math-fence fallback, typed shell/read/grep/web tool cards, deliverable and changed-files cards, a markdown inline-parser infinite-loop fix, transcript render windowing, single-row replay retries and history-load timeouts. Still tracked per package: workspace tree drag reordering, session-log upload, search billing, terminal badges, the trajectory view, plugin card badges, the start-page "more tools" menu and delivery-limit hints.

Cold delivery and reserved sessions require the companion `dsh-acp` build with `ctx.acpSessionHost`. Creating scheduled sessions with an explicit Agent preset needs upstream support: the ACP profile must mount `agent-preset-registry` and `acpSessionHost.create` must accept a preset. KWork rejects such requests explicitly rather than creating a session with the wrong configuration. Workspace rename and removal are local alias/hide preferences, not registry or filesystem operations. Visual and animation parity is kept incrementally against the official `dsh-client-ui-*` packages.

## License

KWork is licensed under the [MIT License](LICENSE). Bundled third-party components retain their own license notices.

- DeepSeek Harness [assets](ui/assets/DeepSeek-LICENSE.txt), [Schedule and Workspace runtime](services/vendor/dsh-schedule/LICENSES.txt), and [Ask User tool](services/vendor/dsh-tool-ask-user/LICENSE)
- [GPUI renderer](vendor/gpui-pre-wgpu/LICENSE-APACHE)
- [GPUI web platform](vendor/gpui-pre-web/LICENSE-APACHE)
- [Noto Sans](ui/assets/NotoSans-LICENSE) and [xterm.js](ui/assets/xterm-LICENSE.txt)
