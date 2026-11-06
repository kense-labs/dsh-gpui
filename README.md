# dsh-gpui

DeepSeek Harness 的桌面 GPUI 客户端。Tauri 负责桌面窗口、ACP 子进程和 JSON-RPC 通信；KUI 提供 GPUI 组件，前端编译为 WebAssembly 后在 Tauri WebView 中运行。

## 功能

- 新建、列出、恢复和关闭持久会话
- 发送任务、取消执行、展示助手消息与思考内容
- 展示工具输入、执行状态和结果
- 人工批准或拒绝一次性工具权限请求
- 切换模型和推理强度，显示上下文用量
- 明暗主题、中文界面、工作区与服务路径设置

## 源码分析结论

DeepSeek Harness 采用 Cordis 插件架构。`dsh-base` 提供模型、工具、持久化和审批能力，`dsh-acp-app` 将 Agent 生命周期映射到标准 ACP。一个 ACP 连接可以承载多个独立会话，每个会话同时只允许一个进行中的 prompt。

ACP 服务端接收 `initialize`、`session/new`、`session/list`、`session/resume`、`session/close`、`session/prompt`、`session/cancel` 和 `session/set_config_option`。它通过 `session/update` 发送已提交的助手消息、思考、工具生命周期和上下文用量，通过 `session/request_permission` 请求一次性授权。

本客户端遵循两个重要边界：ACP 恢复会话但不回放历史消息；ACP 发送提交后的语义消息块，而不是模型原始 token 流。因此恢复时界面会显示明确提示，消息也在服务端提交后出现。

## 目录

```text
src-tauri/       Tauri 后端、Node 子进程、NDJSON JSON-RPC 路由
ui/              KUI/GPUI Rust 前端与 WASM 入口
ui/src/state.rs  可测试的 ACP 消息归并逻辑
scripts/         可重复的 WASM 构建脚本
```

## 准备

- Node.js 24.9+
- pnpm 12.6.0
- Rust stable、`wasm32-unknown-unknown` target
- Tauri 的平台依赖；macOS 需要 Xcode Command Line Tools
- 同级目录中的 `../dsh-acp`

安装并构建 ACP 服务：

```sh
cd ../dsh-acp
pnpm install
pnpm run build
export DEEPSEEK_API_KEY=your_key
cd ../dsh-gpui
npm install
```

未构建 `dsh-acp` 时，客户端会尝试通过 `node --import tsx/esm apps/cli/src/bin.ts --profile acp` 从源码启动。生产使用建议先构建服务。

## 运行

```sh
export DEEPSEEK_API_KEY=your_key
npm run dev
```

客户端默认自动寻找同级 `../dsh-acp`，也可以设置：

```sh
export DSH_ACP_PATH=/absolute/path/to/dsh-acp
export DSH_NODE=/absolute/path/to/node       # 可选
export DSH_HOME=/absolute/path/to/harness-home # 可选
```

`DSH_HOME`、DeepSeek 凭据和其他 Harness 环境变量由子进程继承。工作区必须在发起任务的机器上存在；会话使用其规范化绝对路径。

## 构建与检查

```sh
npm run ui:build   # release WASM + wasm-bindgen，工具安装在 .tools/
npm run build      # Tauri 应用，不生成安装包
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

WASM 构建会依据 `Cargo.lock` 安装匹配版本的 `wasm-bindgen-cli` 到项目 `.tools/`，不污染全局 Rust 工具目录。

## ACP 限制

当前服务端不支持会话删除、fork、历史 transcript 回放、计划、终端、客户端文件系统操作或附加工作目录。本客户端不会伪造这些能力。API 密钥只通过环境变量传入 `dsh-acp`，不会写入界面状态或仓库。

## 许可

MIT。KUI 与 DeepSeek Harness 的各自许可和第三方声明仍适用。
