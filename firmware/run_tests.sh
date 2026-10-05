#!/usr/bin/env bash
#
# firmware/run_tests.sh —— 在 PC 上编译并运行 App 层的测试
#
# **不需要板子、不需要 Keil、不需要交叉编译。**
# 这正是 ADR-008 那条纪律（App/ 不许 include HAL）换来的东西：
# 三个人里只有一个人拿得到板子，另外两个人靠这个脚本照样能推进。
#
# CI 里跑的就是它 —— 也就是说，这条纪律一旦被破坏（App/ 里 include 了
# 寄存器头），**这里会直接编译失败**，而不只是靠那条 grep 检查。
#
# 用法：
#   ./firmware/run_tests.sh          # 编译 App/ + tests/ 并跑
#   ./firmware/run_tests.sh -v       # 显示完整编译命令
#
set -euo pipefail

cd "$(dirname "$0")"

CC="${CC:-gcc}"
BUILD=build
# 与 proto/run_tests.sh 同一套严格档位：警告即错误。
# 嵌入式代码在 -Werror 下写会啰嗦一点，但换来的是「PC 上编译得过 = 板子上
# 大概率也编译得过」—— 而板子上的调试成本比这里高一个量级。
CFLAGS="-std=c11 -Wall -Wextra -Werror -O2 -g"
# App/ 只许看到自己的头和 proto/ —— **不许有 HAL 的包含路径**。
# 这条不是风格问题：include 路径里没有 HAL，App/ 就算想 include 也找不到。
INC="-I App -I ../proto"

# `proto/protocol.c` 是与主机端共用的编解码（两端跑同一份黄金向量）——
# 设备侧也要编它。这样「主机解出来的」与「设备发出去的」从构造上就是一套。
PROTO_SRC="../proto/protocol.c"

command -v "$CC" >/dev/null 2>&1 || { echo "找不到 C 编译器: $CC" >&2; exit 1; }

mkdir -p "$BUILD"

# App 层里被测试覆盖到的源文件。
APP_SRC="App/trigger.c App/acq.c App/proto_task.c App/waveform.c App/local_input.c App/local_policy.c App/freq_meter.c"

VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1
run() { if [ "$VERBOSE" = "1" ]; then echo "+ $*"; fi; "$@"; }

pass=0
fail=0

for src in tests/test_*.c; do
    name="$(basename "$src" .c)"
    echo "── $name"
    if ! run "$CC" $CFLAGS $INC -o "$BUILD/$name" "$src" $APP_SRC $PROTO_SRC; then
        printf "  \033[31m编译失败\033[0m —— App/ 是不是 include 了 HAL？\n"
        fail=$((fail + 1))
        continue
    fi
    if run "$BUILD/$name"; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1))
    fi
done

echo
if [ "$fail" -eq 0 ]; then
    printf "\033[32m全部通过\033[0m: %d 个测试程序\n" "$pass"
else
    printf "\033[31m%d 个测试程序失败\033[0m / 通过 %d 个\n" "$fail" "$pass"
    exit 1
fi
