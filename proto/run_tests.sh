#!/usr/bin/env bash
#
# proto/run_tests.sh —— 不依赖 make 的 C 端测试入口
#
# Windows 的 Git Bash 默认不带 make，这个脚本让本地开发也能一行跑完。
# CI / Linux / macOS 上优先用 `make -C proto test`（等价）。
#
# 用法：
#   ./proto/run_tests.sh          # 生成向量 → 编译 → 运行
#   ./proto/run_tests.sh --regen  # 强制重新生成黄金向量
#
set -euo pipefail

cd "$(dirname "$0")"

CC="${CC:-gcc}"
BUILD=build
CFLAGS="-std=c11 -Wall -Wextra -Werror -O2 -g -DPROTO_PARSER_BUF_SIZE=2072"

# ── 工具检查 ─────────────────────────────────────────────────────────
command -v "$CC" >/dev/null 2>&1 || { echo "找不到 C 编译器: $CC" >&2; exit 1; }

# Python 解释器名跨平台不一致：Ubuntu 只有 python3，Windows 安装器给的是 python。
# 不能写死其中一个，否则另一半团队跑不起来。
if [ -z "${PYTHON:-}" ]; then
    for candidate in python3 python; do
        if command -v "$candidate" >/dev/null 2>&1; then
            PYTHON="$candidate"
            break
        fi
    done
fi
[ -n "${PYTHON:-}" ] || { echo "找不到 Python（试过 python3 和 python）。装一个：https://www.python.org" >&2; exit 1; }

mkdir -p "$BUILD"

# ── 1. 黄金向量 ──────────────────────────────────────────────────────
if [[ "${1:-}" == "--regen" || ! -f tests/vectors.json \
      || tests/gen_vectors.py -nt tests/vectors.json ]]; then
    echo "→ 重新生成 tests/vectors.json"
    PYTHONIOENCODING=utf-8 "$PYTHON" tests/gen_vectors.py > tests/vectors.json
fi

# ── 2. JSON → C 头文件 ───────────────────────────────────────────────
echo "→ tests/vectors.json → $BUILD/vectors.h"
PYTHONIOENCODING=utf-8 "$PYTHON" tests/gen_header.py tests/vectors.json > "$BUILD/vectors.h"

# ── 3. 编译 ──────────────────────────────────────────────────────────
echo "→ 编译 ($CC)"
# shellcheck disable=SC2086
"$CC" $CFLAGS -I. -I"$BUILD" -o "$BUILD/test_vectors" tests/test_vectors.c protocol.c

# ── 4. 运行 ──────────────────────────────────────────────────────────
echo "→ 运行"
exec "./$BUILD/test_vectors"
