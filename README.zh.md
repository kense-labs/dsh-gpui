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

安装 Node.js 24.9 或更高版本、pnpm 12.6.0、通过 `rustup` 安装的 Rust stable，以及当前平台的 Tauri 构建依赖。macOS 还需安装 Xcode Command Line Tools 和 `just`。

将 KWork 和配套的 [dsh-acp](https://github.com/kense-labs/dsh-acp) 服务克隆到同一个父目录下。

```sh
git clone https://github.com/kense-labs/kwork.git
git clone https://github.com/kense-labs/dsh-acp.git
cd dsh-acp
pnpm install
pnpm run build
cd ../kwork
npm ci
```

macOS 上运行 `just dev`，构建并启动 `.tools/dev-app/KWork.app`。其他平台使用 `npm run dev` 启动 Tauri 开发客户端。

首次启动可以添加 DeepSeek API Key，也可以选择「稍后配置」进入工作区。之后从侧栏打开「更多 → 设置 → API Key」管理密钥。KWork 也支持 `DEEPSEEK_API_KEY` 环境变量，并为该密钥使用 `deepseek-official` 路由。

## 模型服务

打开「设置 → 模型服务」，可选择硅基流动、深度求索、OpenRouter、OpenAI 或 Ollama 的预设。添加自定义服务时，选择 Chat Completions 或 Responses，填写 API 基础地址和所需密钥，再同步模型列表或手动填写模型 ID。保存后可在输入区的模型菜单中切换。

Tauri 主进程通过 Harness 本机凭据服务写入密钥，前端读取凭据状态。Harness 将凭据保存在其数据目录下的 `.credentials.yaml`。

## 配置

| 环境变量 | 用途 |
| --- | --- |
| `DSH_ACP_PATH` | `dsh-acp` 检出的路径，默认查找路径包含 `../dsh-acp`。 |
| `DSH_NODE` | 主进程使用的 Node.js 可执行文件路径。 |
| `DSH_HOME` | Harness 数据目录，默认 `~/.dsh`。 |

工作区目录必须在运行 KWork 的机器上存在。ACP 使用该目录规范化后的绝对路径。恢复会话时，KWork 会重新连接对应的 Harness Agent，并独立于 ACP 更新流读取已保存的历史。

## 构建与检查

```sh
npm run ui:build
cargo fmt --all --check
cargo test --workspace
node --test scripts/*.test.mjs
```

`npm run build` 构建发布版主程序。打包 macOS 应用时，先构建前端，再运行 `npm exec -- tauri build --bundles app`。前端构建会将 `Cargo.lock` 锁定版本的 `wasm-bindgen-cli` 安装到 `.tools/`。

| 路径 | 内容 |
| --- | --- |
| `ui/` | Rust 界面、WebAssembly 入口和资源 |
| `src-tauri/` | 原生窗口、IPC、ACP 进程管理和应用图标 |
| `services/` | 凭据桥、ACP 桥和内置 Harness 服务 |
| `scripts/` | 构建工具和 JavaScript 测试 |

## 项目状态

已验证 macOS 开发构建、API Key 管理、ACP 自动化投递、从运行记录打开会话，以及会话归档与恢复。冷启动投递和预留会话需要配套 `dsh-acp` 提供 `ctx.acpSessionHost`。按指定 Agent 预设创建自动化会话、完整工作区管理，以及与 DeepSeek Harness 的视觉和动画对齐仍在进行。

## 许可

KWork 采用 [MIT 许可证](LICENSE)。随项目分发的第三方组件保留各自的许可声明。

- DeepSeek Harness [资源](ui/assets/DeepSeek-LICENSE.txt)、[调度与工作区运行时](services/vendor/dsh-schedule/LICENSES.txt)和 [Ask User 工具](services/vendor/dsh-tool-ask-user/LICENSE)
- [GPUI 渲染器](vendor/gpui-pre-wgpu/LICENSE-APACHE)
- [Noto Sans](ui/assets/NotoSans-LICENSE)和 [xterm.js](ui/assets/xterm-LICENSE.txt)
