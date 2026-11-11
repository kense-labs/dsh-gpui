set dotenv-load := true

default:
    @just --list

# 构建 KUI/GPUI 前端并启动 Tauri 开发客户端
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$(dirname "$(rustup which rustc)"):$PATH"
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
        npm run dev
    fi

# 仅用 blaze 增量构建原生 Host（失败自动回退 cargo，结果缓存在 .tools/）
host-blaze:
    node scripts/blaze-build.mjs

# 使用 blaze 构建原生 Host 并启动 macOS 开发应用
dev-blaze:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$(dirname "$(rustup which rustc)"):$PATH"
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
        npm run dev
    fi
