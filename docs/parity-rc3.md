# 与官方 DSH rc.3 的逐项对齐记录

对照对象：`deepseek-harness` 标签 `dsh-v0.2.2-rc.3`
（commit `38a6a5c965aa99ab525161efb7cc5cf1fe62fced`，快照位于 `/tmp/dsh-rc3`）。
KWork 是 Rust/GPUI 客户端，通过 ACP 与 Host 通信；官方客户端是 React/Web
（`packages/client/*`）。两边共享同一 Host 协议，差别集中在**客户端渲染与本地能力**。

本文记录三类内容：

- **已对齐并验证**：行为与官方一致，且有真机或测试证据；
- **近似实现**：官方能力依赖 Web/浏览器或第三方库，GPUI 客户端用等价降级方案；
- **不可达 / 不适用**：ACP 协议没有暴露所需数据，或功能依赖官方 Web 端专有能力。

## 已对齐并验证

| 领域 | 官方来源 | 说明与验证方式 |
| --- | --- | --- |
| 会话转录 | `ui-chat` | 用户/助手/思考/工具/系统行、轮次分隔、错误横幅、历史分页（加载更早/重新加载/超时保护）；真机 + 224 UI 测试 |
| Markdown 渲染 | `ui-primitives/markdown` | 标题、列表、表格、代码围栏与高亮、diff 围栏、图片、引用、链接、行内代码；未配对 `**` 死循环已修（`4bc8a33`） |
| 数学围栏 | `ui-primitives/markdown` KaTeX | 降级为居中 TeX 源码 + 「TeX 公式源码」标注（见“近似实现”） |
| 类型化工具卡 | `ui-tool-*` | shell（`$ 命令` + 描述）、read（行号 + 文件尾注）、grep（文件分组）、web_fetch（URL + HTTP 状态 + 正文）；暗色/浅色均验证 |
| 交付出物卡片 | `ui-sidebar` DeliverablesTail | 路径 + 描述、2 列网格、全部/收起；点击预览在侧栏打开文档、默认应用打开、Finder 显示；真机验证 |
| 已编辑文件卡 | `changed-files` | 文件列表行点击在侧栏打开；行数 `+N/-M` 不可达（见下） |
| 审批流 | `ui-approvals` | 命令摘要、工具标签本地化、拒绝/允许一次/始终允许；真实审批流程验证过 |
| 计划/待办 | `ui-chat` plan card、`todo/write` | 计划卡与待办行；ACP 事件回放验证 |
| 子代理 | `subagent/*` | 子代理目录、描述行、只读子会话视图；回放与真实会话验证 |
| 后台任务 | `ui-jobs` | 注册/结算行、时长、退出码；真实会话验证 |
| 模型重试 | `llm/retry*` | 单行 upsert、延迟/失败原因；回放不显示永动等待点 |
| 工作区侧栏 | `ui-sidebar` | 工作区分组、置顶/隐藏/折叠、会话归档与重命名、排序、分组切换 |
| 文件侧栏 | `ui-sidebar-files` | 搜索、按工作区持久化的排除规则、显示隐藏项、忽略文件开关 |
| 内置浏览器 | `ui-sidebar-browser` | 右栏标签、地址栏、前进后退刷新、恢复上次页面 |
| 设置 | `ui-settings` | 服务目录、快捷键、外观、正文偏好、开发者工具、隐藏工作区 |
| 命令面板 | `ui-command-picker` | 命令检索与执行 |
| 输入区统计 | `ui-composer` | 轮/步/Token/缓存命中/耗时，点击展开统计弹窗；真机验证 |
| 快捷键 | `ui-shortcuts` | 可录制、平台适配、冲突提示 |
| 空/错误状态 | 多处 | 欢迎页、历史失败与超时、文件不可读、目录不存在等文案对齐 |

## 近似实现

| 功能 | 官方实现 | KWork 方案 | 原因 |
| --- | --- | --- | --- |
| 数学公式 | KaTeX 排版 | 居中 TeX 源码 + 标注 | GPUI 无 TeX 引擎，引入 KaTeX 需 WebView/JS 桥 |
| 内置浏览器 | Electron WebContentsView | WKWebView（Tauri） | 平台差异；标签/恢复/前进后退已对齐 |
| 外部链接 | 系统浏览器 | 本机 open_url（http/https/file） | 已按官方“侧栏或外部”偏好分流 |
| Markdown 图片 | `<img>` 懒加载 | 原生 image 元素 + 本地相对路径解析 | 依赖本机文件读取 |
| 代码高亮 | shiki | 轻量关键词/字符串/数字/注释着色 | 不引入 JS 依赖；主要语言可读性足够 |
| 会话列表实时性 | 服务端事件推送 | Host 事件 + 本地 projcache 扫描合并 | ACP 列表接口不含全部本地会话 |

## 不可达 / 不适用（ACP 协议或 Host 限制）

| 功能 | 官方数据来源 | 现状 | 说明 |
| --- | --- | --- | --- |
| 文件 diff 卡 | `meta.diffs` / Host 路由 | 只显示变更文件列表与打开动作 | 实时 ACP 工具更新不带 `meta.diffs`，也没有 diff 路由 |
| bash 退出码胶囊 | 工具 meta | 部分工具结果文本含 `exit code`，无独立胶囊 | ACP 实时 meta 只有 `cwd` 等有限字段 |
| read_image 图片卡 | 工具 meta 图片数据 | 图片类工具按普通文本行展示 | ACP 不传图片数据 |
| Agent 终端徽标 | `ui-sidebar-terminal` | 不支持 | KWork 无 agent/沙箱终端；本机 shell 仅限开发者工具 |
| 账号通知 | `ui-account-notifications` | 不适用 | 官方 Web 端账号体系能力 |
| 插件管理状态 | 设置 schema 下发 | 只展示 Host 提供的信息 | Host 未提供完整插件 schema/版本字段 |
| 上下文用量环 | 客户端估算 | 使用 Host 提供的 usage | 估算口径不同，仅展示 Host 数据 |
| 数学 KaTeX 排版 | KaTeX | 近似实现 | 见上表 |

## 验证方式

- **原生测试**：`cargo test --workspace`（56 宿主 + 225 UI）。
- **wasm 检查**：`cargo check -p dsh-gpui-ui --target wasm32-unknown-unknown`（0 warning）。
- **脚本测试**：`bun test scripts/*.test.mjs`（12 项）。
- **真机验证**：`scripts/dev-app.sh` 构建 dev app，配合：
  - 真实会话（只读回放、分页、审批、作业、子代理）；
  - debug 构建的渲染演示快捷键 `Cmd/Ctrl+Shift+D`（数学围栏、类型化工具卡、
    交付卡片、已编辑文件卡、Markdown 链接与 web 卡片），可在浅色/深色主题下截图核对；
  - 窗口级截图 + OCR 坐标自动化（`/tmp/kwork-att/*` 工具，非仓库内容）。
- **CI**：GitHub Actions `windows.yml` 在每次 push 到 main 后构建 Windows 安装包。

最后更新：与 rc.3 对齐验证进行中（本轮为第 20 轮），新增差异会补充到对应表格。
