set dotenv-load := true

# KWork's build chain is Bun-only. Node is NOT a build dependency: the only
# Node process is the external dsh-acp / credential helper child (see below).
bun := env_var_or_default("KWORK_BUN", "bun")

default:
    @just --list

# Fail early with an actionable message instead of "bun: command not found".
_require-bun:
    #!/usr/bin/env bash
    if ! command -v "{{bun}}" >/dev/null 2>&1; then
        echo "找不到 Bun：请安装 Bun，或用 KWORK_BUN 指定可执行文件。KWork 构建链不再依赖 Node/npm。" >&2
        exit 1
    fi

# 构建 KUI/GPUI 前端并启动 Tauri 开发客户端
dev: _require-bun
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$(dirname "$(rustup which rustc)"):$PATH"
    export KWORK_BUN="{{bun}}"
    # Make nested `bun run ui:build` from package.json resolve even when the
    # override points at a Bun binary that is not on PATH.
    export PATH="$(dirname "$(command -v "{{bun}}")"):$PATH"
    # This PATH entry is for resolve_node() in the running host: dsh-acp and
    # the credential helper are separate Node processes. It is NOT used to
    # build the frontend.
    for node_bin in "{{justfile_directory()}}"/.tools/node-*/bin; do
        if [[ -x "$node_bin/node" ]]; then
            export PATH="$node_bin:$PATH"
            break
        fi
    done
    export DSH_ACP_PATH="${DSH_ACP_PATH:-{{justfile_directory()}}/../dsh-acp}"
    if [[ "$(uname -s)" == "Darwin" ]]; then
        bash scripts/dev-app.sh
    else
        "{{bun}}" run dev
    fi

# 只构建快速 WASM 前端（dev-wasm，跳过 LTO 和 wasm-opt）
ui-fast: _require-bun
    KWORK_UI_FAST=1 "{{bun}}" scripts/build-ui.mjs

# 用快速前端构建并启动 macOS 开发应用（产物未优化，打包前需跑 bun run ui:build）
dev-fast: _require-bun
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$(dirname "$(rustup which rustc)"):$PATH"
    export KWORK_BUN="{{bun}}"
    # Make nested `bun run ui:build` from package.json resolve even when the
    # override points at a Bun binary that is not on PATH.
    export PATH="$(dirname "$(command -v "{{bun}}")"):$PATH"
    for node_bin in "{{justfile_directory()}}"/.tools/node-*/bin; do
        if [[ -x "$node_bin/node" ]]; then
            export PATH="$node_bin:$PATH"
            break
        fi
    done
    export DSH_ACP_PATH="${DSH_ACP_PATH:-{{justfile_directory()}}/../dsh-acp}"
    export KWORK_UI_FAST=1
    if [[ "$(uname -s)" == "Darwin" ]]; then
        bash scripts/dev-app.sh
    else
        "{{bun}}" run dev
    fi

# 仅用 blaze 增量构建原生 Host（失败自动回退 cargo，结果缓存在 .tools/）
host-blaze: _require-bun
    "{{bun}}" scripts/blaze-build.mjs

# 使用 blaze 构建原生 Host 并启动 macOS 开发应用
dev-blaze: _require-bun
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$(dirname "$(rustup which rustc)"):$PATH"
    export KWORK_BUN="{{bun}}"
    # Make nested `bun run ui:build` from package.json resolve even when the
    # override points at a Bun binary that is not on PATH.
    export PATH="$(dirname "$(command -v "{{bun}}")"):$PATH"
    for node_bin in "{{justfile_directory()}}"/.tools/node-*/bin; do
        if [[ -x "$node_bin/node" ]]; then
            export PATH="$node_bin:$PATH"
            break
        fi
    done
    export DSH_ACP_PATH="${DSH_ACP_PATH:-{{justfile_directory()}}/../dsh-acp}"
    if [[ "$(uname -s)" == "Darwin" ]]; then
        KWORK_USE_BLAZE=1 bash scripts/dev-app.sh
    else
        echo "dev-blaze 仅用于 macOS 开发应用；其它平台请使用 just host-blaze。"
        "{{bun}}" run dev
    fi
