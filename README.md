# KWork

KWork 是基于 DeepSeek Harness ACP 的桌面 GPUI 客户端。Tauri 负责桌面窗口、子进程和 IPC；KUI 提供 GPUI 组件，前端编译为 WebAssembly 后在 Tauri WebView 中运行。

当前实现覆盖 API Key 配置和 ACP 对话主流程，并参照官方 Harness 重构了主工作区和设置模态框。完整的功能与界面一致性仍待验证。

## 已实现

- API Key 配置：密码掩码输入、校验、保存、更新和移除
- 主工作区：可动画折叠的会话侧栏、本地会话搜索、按工作区分组、新建和恢复会话
- Kense 品牌标志、深色界面、欢迎区、工作区与模型菜单、蓝色圆形发送／停止按钮
- 设置模态框：模型、API Key、工作区与连接、外观等页面
- 发送任务、取消执行、展示助手消息与思考内容
- 展示工具输入、执行状态和结果，人工批准或拒绝一次性工具权限请求
- 切换模型和推理强度，显示上下文用量
- 明暗主题、中文界面、工作区与服务路径设置
- 凭据就绪后的服务重建，以及发送失败后的草稿恢复
- `KWork` 应用名称、Kense Dock 与打包图标，以及带打开窗口和退出菜单的状态栏图标
- 插件中心、插件安装与更新、网络搜索和运行时设置
- 自动化任务的创建、编辑、暂停、运行记录与实际 ACP 投递，以及归档会话的确认、恢复和持久化

DeepSeek API Key 使用 `deepseek-official` 路由。

在「设置 → 模型服务」可搜索并选用硅基流动、深度求索、OpenRouter、OpenAI 和 Ollama 的预设，也可添加自定义 OpenAI 兼容服务商。选择 Chat Completions 或 Responses，填写 API 基础地址和所需的 API Key，再同步并筛选模型列表，或手动填写模型 ID。保存后可在对话的模型菜单中切换。API Key 由 Harness 本机凭据服务保存。

Composer 左下角的“＋”打开与官方 Harness 相同层级的指令面板，模型和思考强度使用 ACP 返回的真实选项。ACP 尚未提供的文件、目标、计划、反馈和权限预设会明确标记为不可用，不会伪装成已经支持。

## 架构与凭据

DeepSeek Harness 采用 Cordis 插件架构。`dsh-base` 提供模型、工具、持久化和审批能力，`dsh-acp-app` 将 Agent 生命周期映射到标准 ACP。一个 ACP 连接可以承载多个独立会话，每个会话同时只允许一个进行中的 prompt。

客户端启动两个独立的 Node 子进程：

- `dsh-acp`：标准 ACP stdio 服务，负责会话、模型、工具和权限。
- `services/credential-service.mjs`：Host-only NDJSON 辅助服务，通过同级 `dsh-acp` 的 `LocalCredentialProvider` 管理 API Key。

两个进程共享 `$DSH_HOME/.credentials.yaml`。API Key 只从 Tauri Host 写入官方凭据服务，前端只读取配置状态。凭据文件在隔离测试中权限为 `0600`，但它是凭据文件而非界面加密保险库，应继续保护用户主目录。

ACP 恢复会话但不回放历史消息；ACP 发送提交后的语义消息块，而不是模型原始 token 流。

## 目录

```text
src-tauri/             Tauri 后端、ACP、凭据子进程与原生托盘
src-tauri/icons/       KWork 应用图标和状态栏模板
services/              Host-only 官方凭据桥
ui/                    KUI/GPUI Rust 前端与 WASM 入口
ui/src/web/workspace.rs 主工作区、对话、Composer 与设置模态框
ui/src/welcome.rs      首次配置 API Key 和设置页凭据控件
ui/src/state.rs        可测试的消息归并与凭据草稿校验
scripts/               可重复的 WASM 与开发应用构建脚本
justfile               本地快捷启动
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
cd ../dsh-gpui
npm install
```

未构建 `dsh-acp` 时，客户端会尝试通过 `node --import tsx/esm apps/cli/src/bin.ts --profile acp` 从源码启动。生产使用建议先构建服务。

## 启动

```sh
just dev
```

macOS 上该命令构建前端和 debug Host，并在 `.tools/dev-app/` 生成轻量 `KWork.app` 后运行，使开发模式的 Dock 名称和图标也与正式应用一致。其他平台使用 `npm run dev` 启动 Tauri 开发客户端。

首次启动可直接添加 API Key，也可选择「稍后配置」进入工作区。以后通过侧栏底部「更多」菜单打开设置，在「API Key」页管理密钥。已有的 `DEEPSEEK_API_KEY` 环境变量也可用。

可选环境变量：

```sh
export DSH_ACP_PATH=/absolute/path/to/dsh-acp
export DSH_NODE=/absolute/path/to/node         # 可选
export DSH_HOME=/absolute/path/to/harness-home # 可选，默认 ~/.dsh
```

工作区必须在发起任务的机器上存在；会话使用其规范化绝对路径。

## 构建与检查

```sh
npm run ui:build   # release WASM + wasm-bindgen，工具安装在 .tools/
npm run build      # Tauri release Host，不生成安装包
npm exec -- tauri build --bundles app  # macOS .app，包含名称和图标
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p dsh-gpui-ui --target wasm32-unknown-unknown -- -D warnings
```

WASM 构建会依据 `Cargo.lock` 安装匹配版本的 `wasm-bindgen-cli` 到项目 `.tools/`，不污染全局 Rust 工具目录。

## 尚未对齐与验收边界

当前版本仍在对照本地官方 Harness 逐项验收。自动化按指定 Agent 预设创建会话、完整工作区管理，以及全界面和动画的一致性仍待完成。菜单阴影已在明暗主题和 1×、2× 下与官方 CSS 对照，其余视图需要各自的视觉验收。

隔离测试已验证 API Key 管理、真实 ACP 自动化投递、从运行记录打开同一会话、旧归档迁移、停止并归档以及恢复后的投递。自动化的冷启动恢复和预留会话需要配套 `dsh-acp` 提供 `ctx.acpSessionHost`。当前 macOS 开发应用已完成构建和隔离启动检查。

## 许可

MIT。KUI 与 DeepSeek Harness 的各自许可和第三方声明仍适用。
