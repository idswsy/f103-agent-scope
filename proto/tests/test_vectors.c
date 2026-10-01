/**
 * @file    test_vectors.c
 * @brief   C 端黄金向量测试 —— 与 Rust 端跑同一份 proto/tests/vectors.json
 *
 * 这是「跨语言协议一致」的门禁。任何一端改了编解码逻辑，
 * 只要与 vectors.json 不符，本测试立刻失败。
 *
 * 构建：make -C proto test
 * 依赖：gcc（或任意 C99 编译器）+ python3（用于 json → header 转换）
 */

#include "../protocol.h"
#include "vectors.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ══════════════════════════════════════════════════════════════════
 * 极简测试框架
 * ══════════════════════════════════════════════════════════════════ */

static int g_pass = 0;
static int g_fail = 0;
static const char *g_group = "";

#define OK()    do { g_pass++; } while (0)

#define FAIL(fmt, ...)                                                   \
    do {                                                                 \
        g_fail++;                                                        \
        fprintf(stderr, "  \033[31mFAIL\033[0m [%s] " fmt "\n",          \
                g_group, ##__VA_ARGS__);                                 \
    } while (0)

#define CHECK(cond, fmt, ...)                                            \
    do {                                                                 \
        if (cond) { OK(); } else { FAIL(fmt, ##__VA_ARGS__); }           \
    } while (0)

#define GROUP(name) do { g_group = (name); printf("\n[%s]\n", g_group); } while (0)

/* ══════════════════════════════════════════════════════════════════
 * hex 工具
 * ══════════════════════════════════════════════════════════════════ */

static int hex_nibble(char c)
{
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

/** hex 字符串 → 字节数组；返回字节数，-1 表示非法输入 */
static int hex2bin(const char *hex, uint8_t *out, size_t out_cap)
{
    if (hex == NULL) return -1;
    const size_t n = strlen(hex);
    if (n % 2 != 0) return -1;
    const size_t bytes = n / 2;
    if (bytes > out_cap) return -1;

    for (size_t i = 0; i < bytes; i++) {
        const int hi = hex_nibble(hex[i * 2]);
        const int lo = hex_nibble(hex[i * 2 + 1]);
        if (hi < 0 || lo < 0) return -1;
        out[i] = (uint8_t)((hi << 4) | lo);
    }
    return (int)bytes;
}

static void bin2hex(const uint8_t *bin, size_t len, char *out, size_t out_cap)
{
    static const char *D = "0123456789abcdef";
    size_t w = 0;
    for (size_t i = 0; i < len && w + 2 < out_cap; i++) {
        out[w++] = D[(bin[i] >> 4) & 0x0F];
        out[w++] = D[bin[i] & 0x0F];
    }
    out[w] = '\0';
}

/* ══════════════════════════════════════════════════════════════════
 * 1. CRC 自检
 * ══════════════════════════════════════════════════════════════════ */

static void test_crc(void)
{
    GROUP("CRC-16/CCITT-FALSE");

    const uint16_t got = proto_crc16((const uint8_t *)VEC_CRC_INPUT,
                                     strlen(VEC_CRC_INPUT));
    CHECK(got == VEC_CRC_EXPECTED,
          "standard check value: got 0x%04X, expected 0x%04X", got, VEC_CRC_EXPECTED);

    CHECK(proto_crc16(NULL, 0) == 0xFFFF,
          "empty input should yield init value 0xFFFF, got 0x%04X",
          proto_crc16(NULL, 0));
}

/* ══════════════════════════════════════════════════════════════════
 * 2. 帧编码：逐字节比对 frame_hex
 * ══════════════════════════════════════════════════════════════════ */

static void test_encode(void)
{
    GROUP("帧编码 (proto_encode)");

    uint8_t payload[PROTO_MAX_PAYLOAD_TX];
    uint8_t expect[PROTO_MAX_FRAME_TX];
    uint8_t actual[PROTO_MAX_FRAME_TX];
    char    actual_hex[PROTO_MAX_FRAME_TX * 2 + 1];

    for (size_t i = 0; i < VEC_FRAME_COUNT; i++) {
        const vec_frame_t *v = &VEC_FRAMES[i];

        const int pl_len = hex2bin(v->payload_hex, payload, sizeof(payload));
        const int ex_len = hex2bin(v->frame_hex, expect, sizeof(expect));
        if (pl_len < 0 || ex_len < 0) {
            FAIL("%s: 向量 hex 非法", v->name);
            continue;
        }

        size_t out_len = 0;
        const proto_status_t st = proto_encode(actual, sizeof(actual),
                                               v->flags, v->seq, v->cmd,
                                               pl_len > 0 ? payload : NULL,
                                               (uint16_t)pl_len, &out_len);
        if (st != PROTO_OK) {
            FAIL("%s: proto_encode 返回 %d", v->name, (int)st);
            continue;
        }
        if ((int)out_len != ex_len) {
            FAIL("%s: 帧长 %zu, 期望 %d", v->name, out_len, ex_len);
            continue;
        }
        if (memcmp(actual, expect, out_len) != 0) {
            bin2hex(actual, out_len, actual_hex, sizeof(actual_hex));
            FAIL("%s:\n    got  %s\n    want %s", v->name, actual_hex, v->frame_hex);
            continue;
        }
        OK();
        printf("  ok  %-28s (%d bytes)\n", v->name, ex_len);
    }
}

/* ══════════════════════════════════════════════════════════════════
 * 3. 解析器：粘包 / 拆包 / 重同步
 * ══════════════════════════════════════════════════════════════════ */

/** 解析逗号分隔的整数串，返回元素个数 */
static int parse_csv_u32(const char *s, uint32_t *out, int cap)
{
    if (s == NULL || *s == '\0') return 0;
    int n = 0;
    const char *p = s;
    while (*p != '\0' && n < cap) {
        char *end = NULL;
        const unsigned long v = strtoul(p, &end, 10);
        if (end == p) break;
        out[n++] = (uint32_t)v;
        p = (*end == ',') ? end + 1 : end;
        if (*end == '\0') break;
    }
    return n;
}

static void test_parser(void)
{
    GROUP("解析器 (proto_parser_feed)");

    static proto_parser_t parser;

    for (size_t i = 0; i < VEC_PARSER_COUNT; i++) {
        const vec_parser_t *v = &VEC_PARSER[i];

        uint8_t stream[8192];
        const int slen = hex2bin(v->stream_hex, stream, sizeof(stream));
        if (slen < 0) { FAIL("%s: stream hex 非法", v->name); continue; }

        uint32_t e_cmd[16], e_seq[16], e_flags[16];
        const int n_cmd = parse_csv_u32(v->expect_cmd, e_cmd, 16);
        const int n_seq = parse_csv_u32(v->expect_seq, e_seq, 16);
        const int n_flg = parse_csv_u32(v->expect_flags, e_flags, 16);
        (void)n_seq; (void)n_flg;

        proto_parser_init(&parser);

        int got = 0;
        int mismatches = 0;

        /* 拆包场景：先喂前 split_at 字节。用 NULL 长度触发"仅排空缓冲"。 */
        int fed = 0;
        int chunk = (v->split_at > 0) ? v->split_at : slen;

        for (;;) {
            proto_frame_t fr;
            proto_status_t st;

            if (fed < slen) {
                const int n = (chunk < (slen - fed)) ? chunk : (slen - fed);
                st = proto_parser_feed(&parser, &stream[fed], (size_t)n, &fr);
                fed += n;
            } else {
                /* 数据喂完，排空缓冲里剩余帧 */
                st = proto_parser_feed(&parser, NULL, 0, &fr);
            }

            if (st == PROTO_OK || st == PROTO_BAD_VER) {
                if (got < n_cmd) {
                    if (fr.hdr.cmd != e_cmd[got]) {
                        FAIL("%s: 第 %d 帧 cmd=0x%04X, 期望 0x%04X",
                             v->name, got, fr.hdr.cmd, e_cmd[got]);
                        mismatches++;
                    } else if (got < n_flg && fr.hdr.flags != e_flags[got]) {
                        FAIL("%s: 第 %d 帧 flags=0x%02X, 期望 0x%02X",
                             v->name, got, fr.hdr.flags, e_flags[got]);
                        mismatches++;
                    }
                } else {
                    FAIL("%s: 多解析出第 %d 帧 (cmd=0x%04X)",
                         v->name, got, fr.hdr.cmd);
                    mismatches++;
                }
                got++;
                continue;
            }

            if (st == PROTO_NEED_MORE && fed >= slen) {
                break;
            }
            if (st == PROTO_CRC_ERR || st == PROTO_BAD_LEN) {
                continue;   /* 解析器已自行重同步 */
            }
        }

        if (got != n_cmd) {
            FAIL("%s: 解出 %d 帧, 期望 %d 帧", v->name, got, n_cmd);
            mismatches++;
        }
        if (parser.crc_err_count != v->expect_crc_err) {
            FAIL("%s: crc_err=%u, 期望 %u",
                 v->name, parser.crc_err_count, v->expect_crc_err);
            mismatches++;
        }
        if (v->expect_dropped > 0 && parser.dropped_count != v->expect_dropped) {
            FAIL("%s: dropped=%u, 期望 %u",
                 v->name, parser.dropped_count, v->expect_dropped);
            mismatches++;
        }

        if (mismatches == 0) {
            OK();
            printf("  ok  %-28s (%d 帧)\n", v->name, got);
        }
    }
}

/* ══════════════════════════════════════════════════════════════════
 * 4. PACK12
 * ══════════════════════════════════════════════════════════════════ */

static void test_pack12(void)
{
    GROUP("PACK12 (2 样点 → 3 字节)");

    for (size_t i = 0; i < VEC_PACK12_COUNT; i++) {
        const vec_pack12_t *v = &VEC_PACK12[i];
        uint8_t expect[3];
        if (hex2bin(v->bytes_hex, expect, sizeof(expect)) != 3) {
            FAIL("pack12[%zu]: 向量 hex 非法", i);
            continue;
        }

        uint8_t got[3];
        proto_pack12_pair(v->s0, v->s1, got);
        if (memcmp(got, expect, 3) != 0) {
            FAIL("pack12(%03X,%03X): got %02X%02X%02X, want %s",
                 v->s0, v->s1, got[0], got[1], got[2], v->bytes_hex);
            continue;
        }

        uint16_t r0 = 0, r1 = 0;
        proto_unpack12_pair(got, &r0, &r1);
        if (r0 != v->s0 || r1 != v->s1) {
            FAIL("unpack12(%03X,%03X) → (%03X,%03X) 往返失败",
                 v->s0, v->s1, r0, r1);
            continue;
        }
        OK();
        printf("  ok  pack12(%03X, %03X) → %s\n", v->s0, v->s1, v->bytes_hex);
    }
}

/* ══════════════════════════════════════════════════════════════════
 * 5. 状态机命令合法性
 * ══════════════════════════════════════════════════════════════════ */

static void test_state_matrix(void)
{
    GROUP("状态机 (proto_cmd_allowed)");

    for (int s = 0; s < 5; s++) {
        int bad = 0;
        for (int c = 0; c < VEC_STATE_CMD_COUNT; c++) {
            const bool got = proto_cmd_allowed((scope_state_t)s, VEC_STATE_CMDS[c]);
            const bool want = VEC_STATE_EXPECT[s][c] != 0;
            if (got != want) {
                FAIL("state=%d cmd=0x%04X: got %d, want %d",
                     s, VEC_STATE_CMDS[c], (int)got, (int)want);
                bad++;
            }
        }
        if (bad == 0) {
            OK();
            printf("  ok  state %d (%d 条命令)\n", s, VEC_STATE_CMD_COUNT);
        }
    }
}

/* ══════════════════════════════════════════════════════════════════
 * 6. 结构体布局断言（编译期 + 运行期双重保险）
 * ══════════════════════════════════════════════════════════════════ */

static void test_layout(void)
{
    GROUP("结构体布局");

    CHECK(sizeof(proto_header_t) == 10, "proto_header_t = %zu, 期望 10", sizeof(proto_header_t));
    CHECK(sizeof(chunk_header_t) == 12, "chunk_header_t = %zu, 期望 12", sizeof(chunk_header_t));
    CHECK(offsetof(proto_header_t, seq) == 4, "seq 偏移 = %zu, 期望 4", offsetof(proto_header_t, seq));
    CHECK(offsetof(proto_header_t, len) == 8, "len 偏移 = %zu, 期望 8", offsetof(proto_header_t, len));
}

/* ══════════════════════════════════════════════════════════════════
 * main
 * ══════════════════════════════════════════════════════════════════ */

int main(void)
{
    printf("── C 端黄金向量测试 ──────────────────────────────\n");
    printf("协议版本 0x%02X | 帧头 %d 字节 | payload 上限 tx/rx %d/%d\n",
           VEC_PROTO_VER, PROTO_HEADER_LEN,
           PROTO_MAX_PAYLOAD_TX, PROTO_MAX_PAYLOAD_RX);

    test_crc();
    test_layout();
    test_encode();
    test_parser();
    test_pack12();
    test_state_matrix();

    printf("\n─────────────────────────────────────────────────\n");
    if (g_fail == 0) {
        printf("\033[32m全部通过\033[0m: %d 项\n", g_pass);
        return 0;
    }
    printf("\033[31m失败 %d 项\033[0m, 通过 %d 项\n", g_fail, g_pass);
    return 1;
}
