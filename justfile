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
