# ─────────────────────────────────────────────────────────────────────
# proto/ —— 协议层纯 C 构建
#
#   make test     生成黄金向量 → 编译 → 运行 C 端测试
#   make vectors  只重新生成 vectors.json
#   make clean
#
# 依赖：C99 编译器 + python3（用于 vectors.json ↔ vectors.h 转换）
# 不依赖任何 HAL / 寄存器头文件 —— 这就是它能在 PC 上跑的原因
# ─────────────────────────────────────────────────────────────────────

CC      ?= gcc

# Python 解释器名跨平台不一致：Ubuntu 只有 python3，Windows 安装器给的是 python。
# 自动挑第一个能找到的，避免"在 Linux CI 上跑不起来"。
PYTHON  ?= $(shell command -v python3 2>/dev/null || command -v python 2>/dev/null || echo python3)

BUILD   := build
TESTS   := tests

# C11 是硬要求：protocol.h 用 _Static_assert 锁结构体布局偏移。
# 不加 -pedantic：test_vectors.c 用了 __VA_ARGS__ 宏扩展（GCC/Clang 通用）。
CFLAGS  ?= -std=c11 -Wall -Wextra -Werror -O2 -g
CFLAGS  += -DPROTO_PARSER_BUF_SIZE=2072   # 主机侧要收设备发来的 2060B payload（含 12B 分片头）

ifeq ($(OS),Windows_NT)
  EXE := .exe
  RM  := rm -rf
  MKDIR := mkdir -p
else
  EXE :=
  RM  := rm -rf
  MKDIR := mkdir -p
endif

.PHONY: all test vectors clean

all: test

$(BUILD):
	$(MKDIR) $(BUILD)

# 黄金向量：由 gen_vectors.py 独立生成（含 CRC 标准值自检）
$(TESTS)/vectors.json: $(TESTS)/gen_vectors.py
	$(PYTHON) $(TESTS)/gen_vectors.py > $@
	@echo "  regenerated $@"

# JSON → C 头文件（C 侧不解析 JSON）
$(BUILD)/vectors.h: $(TESTS)/vectors.json $(TESTS)/gen_header.py | $(BUILD)
	$(PYTHON) $(TESTS)/gen_header.py $(TESTS)/vectors.json > $@

$(BUILD)/test_vectors$(EXE): $(TESTS)/test_vectors.c protocol.c protocol.h $(BUILD)/vectors.h
	$(CC) $(CFLAGS) -I. -I$(BUILD) -o $@ $(TESTS)/test_vectors.c protocol.c

test: $(BUILD)/test_vectors$(EXE)
	@./$(BUILD)/test_vectors$(EXE)

vectors: $(TESTS)/vectors.json

clean:
	$(RM) $(BUILD)
