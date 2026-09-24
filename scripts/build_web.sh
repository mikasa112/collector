#!/usr/bin/env bash
# 一条命令构建前端并嵌入后端，产出单一可执行文件。
set -euo pipefail
cd "$(dirname "$0")/../web"
npm install
npm run build
cd ..
cargo build --release -p collector-cmd
