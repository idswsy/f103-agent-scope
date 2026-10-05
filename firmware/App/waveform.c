/* App/waveform.c —— 见 waveform.h 的说明。 */

#include "waveform.h"

/* ⚠ 与 `App/local_input.c` 同一处坑，见那里的长注释：
 * 用 `NULL` 就必须自己 include —— ARMCC5 不会从 `<stdint.h>` 里
 * 传递给你，而 gcc（本机与 CI）会。 */
#include <stddef.h>

/* ── 环内游标 ─────────────────────────────────────────────────── */

/* 在环里逐点前进。
 *
 * 不用「每点一次取模」的写法：`ACQ_RING_SAMPLES` 是 4096，编译器会把
 * `% 4096` 折成 `& 0xFFF`，但窗口可能跨过环尾，回绕那一步仍要在循环里判。
 * 游标把这一判断收在一处。 */
typedef struct {
    const uint16_t *ring;
    uint32_t len;
    uint32_t idx;
} wave_cursor_t;

static void cursor_seek(wave_cursor_t *c, const uint16_t *ring, uint32_t len, uint32_t abs_idx)
{
    c->ring = ring;
    c->len = len;
    c->idx = (len != 0u) ? (abs_idx % len) : 0u;
}

static uint16_t cursor_take(wave_cursor_t *c)
{
    uint16_t v = c->ring[c->idx];
    c->idx++;
    if (c->idx >= c->len) {
        c->idx = 0u;
    }
    return v;
}

/* ── 列映射 ───────────────────────────────────────────────────── */

/* 第 `c` 列覆盖的样点区间 `[*s, *e)`。
 *
 * 短窗（`n < WAVE_COLUMNS`）时公式会给出空区间，这里退化成「一列一个样点」，
 * 把窗口**拉宽**铺满整屏。不这么做的话右边会留一大片空白，而那片空白
 * 在屏幕上读起来就是「信号掉到 0」—— 一个由渲染方式制造的假象。 */
static void column_range(uint32_t c, uint32_t n, uint32_t *s, uint32_t *e)
{
    uint32_t a = (uint32_t)(((uint64_t)c * (uint64_t)n) / (uint64_t)WAVE_COLUMNS);
    uint32_t b = (uint32_t)(((uint64_t)(c + 1u) * (uint64_t)n) / (uint64_t)WAVE_COLUMNS);

    if (b <= a) {
        b = a + 1u;
    }
    if (a >= n) {
        a = n - 1u;
    }
    if (b > n) {
        b = n;
    }
    *s = a;
    *e = b;
}

/* ── 主实现 ───────────────────────────────────────────────────── */

void wave_build_raw(const uint16_t *ring, uint32_t ring_len,
                    uint32_t window_start, uint32_t window_len,
                    uint32_t rate_hz, wave_frame_t *out)
{
    wave_cursor_t cur;
    uint32_t i;
    uint32_t c;
    uint16_t gmin = 0xFFFFu;
    uint16_t gmax = 0u;
    uint32_t span;

    out->valid = false;
    out->vpp_lsb = 0u;
    out->freq_hz = 0u;

    if (ring == NULL || ring_len == 0u || window_len == 0u) {
        return;
    }

    /* ── 第一遍：全局 min/max —— 它同时是纵轴刻度和 Vpp ── */
    cursor_seek(&cur, ring, ring_len, window_start);
    for (i = 0u; i < window_len; i++) {
        uint16_t v = cursor_take(&cur);
        if (v < gmin) {
            gmin = v;
        }
        if (v > gmax) {
            gmax = v;
        }
    }
    span = (uint32_t)gmax - (uint32_t)gmin;
    out->vpp_lsb = (uint16_t)span;

    /* ── 第二遍：逐列 min/max → 上下沿 ── */
    for (c = 0u; c < WAVE_COLUMNS; c++) {
        uint32_t s;
        uint32_t e;
        uint32_t k;
        uint16_t cmin = 0xFFFFu;
        uint16_t cmax = 0u;
        uint32_t top;
        uint32_t bot;

        column_range(c, window_len, &s, &e);
        cursor_seek(&cur, ring, ring_len, window_start + s);
        for (k = s; k < e; k++) {
            uint16_t v = cursor_take(&cur);
            if (v < cmin) {
                cmin = v;
            }
            if (v > cmax) {
                cmax = v;
            }
        }

        if (span == 0u) {
            /* 直流：画在**中线**。贴底边是「信号为 0」的视觉语言，
             * 一个 2.6 V 的直流贴底会被读成"没接信号"。 */
            top = WAVE_ROWS / 2u;
            bot = top;
        } else {
            bot = (((uint32_t)(cmin - gmin)) * (WAVE_ROWS - 1u)) / span;
            top = (((uint32_t)(cmax - gmin)) * (WAVE_ROWS - 1u)) / span;
        }
        out->bot[c] = (uint8_t)bot;
        out->top[c] = (uint8_t)top;
    }

    /* ── 第三遍：迟滞过零计频 ──
     *
     * 用迟滞而不是简单比中点：直流信号上叠一点噪声就会让「过零」每秒
     * 发生上百次，报出一个根本不存在的高频。迟滞带取峰峰值的 1/16。 */
    if (span >= WAVE_MIN_SPAN_LSB && window_len >= 2u) {
        uint32_t mid = (uint32_t)gmin + span / 2u;
        uint32_t hys = span / 16u;
        uint32_t lo;
        uint32_t hi;
        bool armed = false;
        bool have_first = false;
        uint32_t n_cross = 0u;
        uint32_t first_q4 = 0u;
        uint32_t last_q4 = 0u;
        uint16_t prev = 0u;
        bool have_prev = false;

        if (hys < 1u) {
            hys = 1u;
        }
        lo = (mid > hys) ? (mid - hys) : 0u;
        hi = mid + hys;

        cursor_seek(&cur, ring, ring_len, window_start);
        for (i = 0u; i < window_len; i++) {
            uint16_t v = cursor_take(&cur);

            if ((uint32_t)v <= lo) {
                armed = true;
            } else if ((uint32_t)v >= hi && armed) {
                armed = false;
                if (have_prev) {
                    /* 在 prev→v 之间线性插值出穿过 mid 的位置，量化到 1/16 样点。
                     * 不做插值的话，窗里只有两三个周期时量化误差就是百分之几。 */
                    uint32_t dv = (uint32_t)v - (uint32_t)prev;
                    uint32_t q4 = 0u;

                    if (dv != 0u && (uint32_t)prev < mid) {
                        q4 = (((mid - (uint32_t)prev) * 16u) + dv / 2u) / dv;
                        if (q4 > 15u) {
                            q4 = 15u;
                        }
                    }

                    {
                        uint32_t pos = (i - 1u) * 16u + q4;
                        if (!have_first) {
                            first_q4 = pos;
                            have_first = true;
                        }
                        last_q4 = pos;
                    }
                }
                n_cross++;
            }

            prev = v;
            have_prev = true;
        }

        if (n_cross >= 2u && last_q4 > first_q4) {
            /* 首末两次穿越之间跨了 `n_cross - 1` 个周期、`last - first` 个
             * 1/16 样点。中间量必须 64 位：4096 × 857142 × 16 ≈ 5.6e10 > 2^32。 */
            unsigned long long num =
                (unsigned long long)(n_cross - 1u) * (unsigned long long)rate_hz * 16ull;
            unsigned long long den = (unsigned long long)(last_q4 - first_q4);

            out->freq_hz = (uint32_t)(num / den);
        }
    }

    out->valid = true;
}

void wave_build(const acq_t *a, wave_frame_t *out)
{
    if (out == NULL) {
        return;
    }
    out->valid = false;
    out->vpp_lsb = 0u;
    out->freq_hz = 0u;

    if (a == NULL || !a->capture_ready || a->window_len == 0u) {
        return;
    }

    /* 直接读环是**安全**的，理由与 `acq_sample()`（acq.c）相同：`capture_ready`
     * 为真意味着 `finish_capture` 已经停掉 DMA 并把窗口冻结了，此后环的内容
     * 不再变。注意这里**没有**领任何半区 —— `take_published_halves` 的所有权
     * 只属于 `acq_poll`，这里的 `capture_ready` 判断就是全部的准入条件。 */
    wave_build_raw(a->hal->ring_base(), ACQ_RING_SAMPLES,
                   a->window_start, a->window_len, a->rate_hz, out);
}
