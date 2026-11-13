# KWork

[English](README.md) · 简体中文 · [MIT 许可证](LICENSE)

KWork 是基于 DeepSeek Harness ACP 的桌面客户端。Rust 界面使用 KUI 和 GPUI，编译为 WebAssembly 后在 Tauri WebView 中运行。Tauri 同时管理原生窗口、系统托盘和本地 Harness 进程。

## 功能

- 按工作区整理会话，搜索本地历史，创建、恢复或归档会话。
- 配置 DeepSeek 或其他 OpenAI 兼容模型服务，在输入区切换模型和推理强度。
- 发送和取消任务，查看工具调用与结果，响应一次性权限请求。
- 创建、编辑、暂停和运行自动化任务，从运行记录打开收到投递的会话。
- 浏览和管理插件，配置网络搜索，使用支持明暗主题的简体中文界面。

## 快速开始

安装 [Bun](https://bun.sh)、通过 `rustup` 安装的 Rust stable，以及当前平台的 Tauri 构建依赖。macOS 还需安装 Xcode Command Line Tools 和 `just`。

**KWork 自身不依赖 Node.js，无论构建期还是运行期。** 前端构建脚本（`scripts/*.mjs`）、依赖安装和 Tauri CLI 全部走 Bun。Node.js 24.9+ / pnpm 12.6.0 只用于独立的 [dsh-acp](https://github.com/kense-labs/dsh-acp) 检出：KWork 把它和一个小型凭据服务当作外部 stdio 子进程启动，`DSH_NODE` 指向的正是这个可执行文件。整个项目里只有这两个子进程需要 Node。

将 KWork 和配套的 dsh-acp 服务克隆到同一个父目录下。

```sh
git clone https://github.com/kense-labs/kwork.git
git clone https://github.com/kense-labs/dsh-acp.git
cd dsh-acp
pnpm install          # 仅 dsh-acp 需要
pnpm run build        # 仅 dsh-acp 需要
cd ../kwork
bun install --no-save # 读取已入库的 package-lock.json，避免生成第二份 lockfile
```

macOS 上运行 `just dev`，构建并启动 `.tools/dev-app/KWork.app`。其他平台使用 `bun run dev` 启动 Tauri 开发客户端。若 Bun 不在 `PATH` 中，可用 `KWORK_BUN` 指定可执行文件。

首次启动可以添加 DeepSeek API Key，也可以选择「稍后配置」进入工作区。之后从侧栏打开「更多 → 设置 → API Key」管理密钥。KWork 也支持 `DEEPSEEK_API_KEY` 环境变量，并为该密钥使用 `deepseek-official` 路由。

## 模型服务

打开「设置 → 模型服务」，可选择硅基流动、深度求索、OpenRouter、OpenAI 或 Ollama 的预设。添加自定义服务时，选择 Chat Completions 或 Responses，填写 API 基础地址和所需密钥，再同步模型列表或手动填写模型 ID。保存后可在输入区的模型菜单中切换。

Tauri 主进程通过 Harness 本机凭据服务写入密钥，前端读取凭据状态。Harness 将凭据保存在其数据目录下的 `.credentials.yaml`。

## 配置

| 环境变量 | 用途 |
| --- | --- |
| `DSH_ACP_PATH` | `dsh-acp` 检出的路径，默认查找路径包含 `../dsh-acp`。 |
| `DSH_NODE` | 用于启动外部 dsh-acp / 凭据服务子进程的 Node.js 可执行文件路径，与 Bun 构建链无关。 |
| `DSH_HOME` | Harness 数据目录，默认 `~/.dsh`。 |

工作区目录必须在运行 KWork 的机器上存在。ACP 使用该目录规范化后的绝对路径。恢复会话时，KWork 会重新连接对应的 Harness Agent，并独立于 ACP 更新流读取已保存的历史。

## 构建与检查

```sh
bun run ui:build
cargo fmt --all --check
cargo test --workspace
bun test scripts/*.test.mjs
```

`bun run build` 构建发布版主程序。打包 macOS 应用时，先构建前端，再运行 `bun x tauri build --bundles app`。前端构建会把 `Cargo.lock` 锁定版本的 `wasm-bindgen-cli` 和按平台匹配的 `wasm-opt` 安装到 `.tools/`，不写入 `package.json`。

| 路径 | 内容 |
| --- | --- |
| `ui/` | Rust 界面、WebAssembly 入口和资源 |
| `src-tauri/` | 原生窗口、IPC、ACP 进程管理和应用图标 |
| `services/` | 凭据桥、ACP 桥和内置 Harness 服务 |
| `scripts/` | 构建工具和 JavaScript 测试 |

## Windows

Rust 宿主已包含 Windows 分支：ConPTY 终端、`pwsh` / `powershell` / `cmd` shell 探测、`cmd /c start` 打开链接、`explorer.exe`、Windows 系统字体、Node 可执行文件探测（`node.exe`、fnm、nvm、Program Files），以及通过 PowerShell 调用的系统目录选择器。`src-tauri/tauri.windows.conf.json` 覆盖窗口尺寸并指定 NSIS 打包目标，Tauri 在 Windows 上会自动把它与 `tauri.conf.json` 合并。

在 Windows 机器上：

```sh
bun install --no-save
bun run ui:build
bun x tauri build --bundles nsis
```

`just dev` 和 `scripts/dev-app.sh` 仅适用于 macOS；开发调试请用 `bun run dev`。运行时仍需要配套的 `dsh-acp` 检出，可在「设置」或环境变量中指定 `DSH_ACP_PATH` 与 `DSH_NODE`（例如 `C:\Program Files\nodejs\node.exe`）。

`.github/workflows/windows.yml` 会在 `windows-latest` 上构建 NSIS 安装包和便携版 `target/release/KWork.exe`，并作为 `KWork-Windows-x64` 产物上传。

### 内嵌的 Node 模块

`services/acp-session-bridge.mjs` 与 `services/credential-service.mjs` 通过 `include_str!` 编进主进程二进制，并不作为散落文件分发，也不再通过 `node --eval` 注入（约 90 KB 命令行参数，叠加环境变量后已接近 macOS 的 `ARG_MAX` 上限）。`src-tauri/src/embedded_script.rs` 会在启动子进程前把它们写成私有的 `.mjs` 临时文件：堆栈信息指向真实路径，连接释放时文件随之删除。574 KB 的调度运行时（`services/vendor/dsh-schedule/index.mjs`）则按内容 SHA-256 寻址缓存在用户临时目录，只写一次，供所有 ACP 连接和每次启动复用。两个脚本仍从 `process.argv.slice(2)` 读取参数，只是原本占着 `argv[1]` 的 `[eval]` 标签换成了脚本路径。

## 构建提速与产物体积

### blaze（只用于原生 Host 的开发增量构建）

[blaze](https://github.com/indexable-inc/blaze) 是 Rust 的常驻增量编译器，用 `blaze` 代替 `cargo` 可以在改一个函数后只重编改动部分。它**不是体积优化工具**：拒绝 `lto = true/thin`、不做跨函数优化，官方支持矩阵里 wasm32 模块体积是 rustc 的 8–50 倍。因此 KWork 的 WASM 前端和所有 release 构建**始终走 cargo**，blaze 只作为 macOS 原生 Host 开发循环的可选加速器：

```sh
curl -fsSL https://ix.dev/blaze | sh       # 需要 ~/.cargo/bin 在 PATH 中
just host-blaze                            # 只构建原生 Host
just dev-blaze                             # 构建并启动 macOS 开发应用
```

`scripts/blaze-build.mjs` 会按「blaze 版本 + `Cargo.lock` + 平台」做一次兼容性探测，结果记在 `.tools/blaze-compat.json`：探测失败就永久回退 `cargo build -p dsh-gpui`，所以 `just dev` 默认路径不会变慢也不会失败。上游修复后可用 `KWORK_BLAZE=force bun scripts/blaze-build.mjs` 重新探测；`KWORK_BLAZE=off` 强制只用 cargo。blaze v2 默认把产物写到 checkout 之外，需要 `KWORK_BLAZE_ARGS='--target-dir target'`；容器/CI 里若沙箱不可用可设 `BLAZE_SANDBOX=off`。

**当前实测结论（2026-11-11，macOS arm64）**：stable `0.20261027.0` 在加载工作区时无法编译 `objc2-exception-helper` 的 build script（writable static `MIN` 无 rustc 符号）；canary `0.20261105.4` 在 `icu_collections` / `icu_provider` / `cargo_metadata` 等 crate 的 proc macro 展开上失败（退出码 101）。`blaze enable` 全局接管 cargo 的做法也不要用：它改 `~/.cargo/config.toml`，会影响同机其它项目，而且 KWork 用 stable 1.99.1、blaze 用自己的 nightly-2026-10-27，wrapper 模式下 daemon 实际链接 0 个程序，增量构建反而比纯 cargo 慢。按官方建议，`cargo check` 与 CI 始终是最终裁决。

### 快速开发循环（基于 cargo，实测）

blaze 构建不了本工作区，所以真正落地的提速来自「不让 cargo 重复做不必要的工作」。两者都不会改变默认流程：`just dev` 与 `bun run build` 仍使用同一份体积优化后的前端。

**跳过未变更的前端构建。** `bun run ui:build` 把 `ui/dist` 的所有输入（`ui/**`（排除 `dist/`）、`shared/**`、`vendor/**`、各 manifest 与 lockfile、构建脚本自身）记入 `.tools/ui-build-stamp.json`：先用 `stat` 对约 220 个文件做 mtime + size 快检，只有发生变化时才计算内容哈希。输入未变时 **0.5–0.7s** 直接退出（原来 warm 构建 2.5–3.1s），因此只改原生 Rust 时 `just dev` 只需重建 Host（约 1.7s）。`KWORK_UI_BUILD=always` 可强制重建。

**迭代 WASM 前端时去掉 LTO。** `release-wasm` 开了 `lto = true`，改一行前端 Rust 就要付约 20s 的全程序重链。`KWORK_UI_FAST=1` 切到独立的 `[profile.dev-wasm]`（`opt-level = "z"`、`lto = false`、`codegen-units = 16`、`debug = 0`），并跳过 `wasm-opt`：

```sh
just ui-fast        # 只构建快速前端
just dev-fast       # 构建并启动使用它的 macOS 开发应用
bun run ui:build    # 打包前恢复体积优化产物
```

| 2026-11-11，macOS arm64 | 默认 | `KWORK_UI_FAST=1` |
| --- | ---: | ---: |
| `bun run ui:build`，前端 Rust 改一行 | 28.6s（LTO 重链 20.7s） | 11.1s（cargo 3.0s） |
| warm、源码无改动 | 2.5–3.1s | 0.5–0.7s（指纹跳过） |
| `ui/dist/pkg/dsh_gpui_ui_bg.wasm` | 13,413,708 B | 33,798,006 B |

若改动真的改变了 wasm 字节，默认路径还要再付约 11s 的 `wasm-opt -Oz`，快速路径则不执行这一步。快速产物未优化、体积约 2.5 倍，仅供本地迭代；`bun run build` 会先跑默认 `ui:build`，打包时自动恢复优化产物，切换 profile 也会让指纹自动失效。

**评估过但未采用**：用 rust-lld 替换 ld64（stable 1.99.1 上 `-C linker-features=+lld` 仍是 unstable；LLD 23.1.1 无法解析本机 macOS SDK 的 TAPI `.tbd`）、原生 Host 的 `debug = 0` dev profile（交替增量实测 2.58s vs 3.45s，无稳定收益）、mold（不支持 macOS）。`sccache` 也未接入：cargo 自身的增量缓存已覆盖开发循环，sccache 主要对 CI / clean 重建有价值，后续可通过 `RUSTC_WRAPPER` 再加。

### 产物体积

WASM 前端是 KWork 最大的产物。体积优化分成三段，全部由 `bun run ui:build` 完成：

1. `Cargo.toml` 的 `[profile.release-wasm]`（继承 release，独立设置 `opt-level = "z"`、`lto = true`、`panic = "abort"`、`strip = false`）；`strip = false` 保留 `target_features` 自定义段，否则 wasm-bindgen 无法识别 rustc 启用的 bulk-memory / reference-types 等特性。
2. `wasm-bindgen` 输出到临时目录后，用 `wasm-opt -Oz` 处理（这是官方在 `strip = true` 警告里推荐的替代方案），结果按内容哈希缓存在 `.tools/wasm-opt-cache/`。
3. 生成的文件按内容与 `ui/dist/` 增量同步：内容没变就不重写，只删除真正过时的 content-hash chunk，warm build 不再删除并重建上百个文件。

实测（本机）：

| 产物 | 优化前 | 优化后 |
| --- | ---: | ---: |
| `ui/dist/pkg/dsh_gpui_ui_bg.wasm` | 18,650,990 B | 13,413,708 B（−28.1%） |
| `ui/dist/` 整体 | 23 MB | 18 MB（−22%） |

原生 Host 的 `[profile.release]` 保持 `lto = "thin"` + `codegen-units = 1` + `strip = true`，不受 WASM profile 影响。开发时如需跳过优化步骤可设 `KWORK_WASM_OPT=off`。优化后的 wasm 已验证 `WebAssembly.validate`、imports/exports 与 wasm-bindgen 原始输出一致（28 个导出不变，仅删掉 4 个未使用导入），并可正常实例化。

## 项目状态

已验证 macOS 开发构建、API Key 管理、ACP 自动化投递、从运行记录打开会话，以及会话归档与恢复。GPUI 界面已使用官方明暗两套设计 token、菜单进入/退场与模态时长、指令结果卡片、服务商品牌图标和不闪烁的流式推理扫光。工作区可通过原生目录选择器添加（`Mod+O`），移除后可在「设置 → 通用」恢复；宿主命令失败、冷恢复的待回答问题与终端 PTY 错误都会显示在界面上，不再静默丢弃。

界面按上游 `deepseek-ai/deepseek-harness` 源码（`packages/client/ui-*/README.zh.md`，共 61 个 UI 包）逐包对齐，当前对照版本为标签 `dsh-v0.2.2-rc.3`。逐项状态（已验证 / 近似实现 / ACP 不可达）记录在 [docs/parity-rc3.md](docs/parity-rc3.md)。近几轮已补齐：数学围栏降级展示、类型化工具卡（shell/read/grep/web）、交付物与已编辑文件卡片、Markdown 行内死循环修复、大转录区渲染窗口化、回放重试单行化与历史加载超时/重试。仍按包跟踪的剩余项：工作区树拖拽重排、steering 本地回显、会话日志上传、网络搜索计费、按扩展名的页签图标、终端徽标、轨迹视图、插件卡片徽标、启动页「更多工具」菜单与最小投递间隔提示。

冷启动投递和预留会话需要配套 `dsh-acp` 提供 `ctx.acpSessionHost`。按指定 Agent 预设创建自动化会话需要上游支持：ACP profile 需挂载 `agent-preset-registry`，且 `acpSessionHost.create` 需接受预设参数；在具备该能力之前，KWork 会明确拒绝这类请求，避免用错误的 Agent 配置创建会话。工作区重命名与移除仍是本地别名/隐藏偏好，不改动注册表或磁盘目录。视觉与动画仍在按官方 `dsh-client-ui-*` 各包逐项对齐。

## 许可

KWork 采用 [MIT 许可证](LICENSE)。随项目分发的第三方组件保留各自的许可声明。

- DeepSeek Harness [资源](ui/assets/DeepSeek-LICENSE.txt)、[调度与工作区运行时](services/vendor/dsh-schedule/LICENSES.txt)和 [Ask User 工具](services/vendor/dsh-tool-ask-user/LICENSE)
- [GPUI 渲染器](vendor/gpui-pre-wgpu/LICENSE-APACHE)
- [GPUI Web 平台](vendor/gpui-pre-web/LICENSE-APACHE)
- [Noto Sans](ui/assets/NotoSans-LICENSE)和 [xterm.js](ui/assets/xterm-LICENSE.txt)
