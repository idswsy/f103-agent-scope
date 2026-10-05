/* tests/test_freq_meter.c —— 输入捕获测频的算术（不需要板子）
 *
 * 这里要钉住的是三类「给出一个看起来合理的错数」的场景：回绕、
 * 分辨率到头、以及凑不够跨度就急着出结果。
 */

#include "freq_meter.h"
#include "test_util.h"

#include <string.h>

/* 造一串上升沿的捕获值：周期为 `period_ticks`，从 `start` 起，共 `n` 个。
 * 自动按 ARR 回绕。 */
static void make_edges(uint32_t start, uint32_t period_ticks, uint32_t n, uint32_t *out)
{
    uint32_t i;
    uint32_t t = start;
    for (i = 0u; i < n; i++) {
        out[i] = t;
        t = (t + period_ticks) % (FREQ_ARR + 1u);
    }
}

/* 一直喂，直到出结果或喂完。返回 true 表示出了结果。 */
static int feed_until_done(freq_meter_t *m, const uint32_t *ticks, uint32_t n, uint32_t *hz)
{
    uint32_t i;
    for (i = 0u; i < n; i++) {
        freq_result_t r = freq_meter_feed(m, ticks[i], hz);
        if (r == FREQ_OK) {
            return 1;
        }
        if (r == FREQ_TOO_FAST) {
            return -1;
        }
    }
    return 0;
}

static void test_one_khz(void)
{
    freq_meter_t m;
    uint32_t ticks[64];
    uint32_t hz = 0u;

    GROUP("1 kHz");

    /* 周期 1000 tick = 1000 µs → 1 kHz */
    make_edges(0u, 1000u, 16u, ticks);
    freq_meter_reset(&m);

    CHECK(feed_until_done(&m, ticks, 16u, &hz) == 1, "应当测得出");
    CHECK(hz == 1000u, "应为 1000 Hz，实测 %u", (unsigned)hz);
}

static void test_hundred_khz(void)
{
    freq_meter_t m;
    uint32_t ticks[64];
    uint32_t hz = 0u;

    GROUP("100 kHz");

    /* 周期 10 tick。凑够 4096 的跨度要 411 个边沿，超过 32 的上限，
     * 所以会在 32 个边沿处出结果：31 个周期 / 310 tick。 */
    make_edges(0u, 10u, 40u, ticks);
    freq_meter_reset(&m);

    CHECK(feed_until_done(&m, ticks, 40u, &hz) == 1, "应当测得出");
    CHECK(hz == 100000u, "应为 100000 Hz，实测 %u", (unsigned)hz);
}

static void test_wraparound(void)
{
    freq_meter_t m;
    uint32_t ticks[64];
    uint32_t hz = 0u;

    GROUP("跨计数器回绕");

    /* 从 65000 起，每 1000 tick 一个边沿，必然跨过 65535 → 0。
     * 不做回绕补偿的话这里会算出一个完全不同的频率。 */
    make_edges(65000u, 1000u, 16u, ticks);
    freq_meter_reset(&m);

    CHECK(feed_until_done(&m, ticks, 16u, &hz) == 1, "跨回绕也应当测得出");
    CHECK(hz == 1000u, "回绕不该影响结果，应为 1000 Hz，实测 %u", (unsigned)hz);
}

static void test_edges_on_the_same_tick_are_too_fast(void)
{
    freq_meter_t m;
    uint32_t hz = 0u;
    freq_result_t r;

    GROUP("超出量程");

    freq_meter_reset(&m);
    (void)freq_meter_feed(&m, 100u, &hz);
    r = freq_meter_feed(&m, 100u, &hz);      /* 同一个 tick */

    CHECK(r == FREQ_TOO_FAST, "同 tick 的两个边沿应报超出量程，实测 %d", (int)r);

    /* 报完之后应当**重新开始**，不是带着一个坏值继续。
     * 从一个已知输入变成"分不出来"是换了一种状态，不是"更准了"。 */
    {
        uint32_t ticks[16];
        make_edges(200u, 1000u, 16u, ticks);
        CHECK(feed_until_done(&m, ticks, 16u, &hz) == 1, "重新起一段之后应当还能测");
        CHECK(hz == 1000u, "恢复后应为 1000 Hz，实测 %u", (unsigned)hz);
    }
}

static void test_a_single_edge_is_not_a_measurement(void)
{
    freq_meter_t m;
    uint32_t hz = 12345u;

    GROUP("只有一个边沿");

    freq_meter_reset(&m);
    CHECK(freq_meter_feed(&m, 500u, &hz) == FREQ_PENDING,
          "一个边沿定不了周期，应报 PENDING");
    CHECK(hz == 12345u, "没出结果时**不得**动 out 参数，实测被改成了 %u", (unsigned)hz);
}

static void test_pending_leaves_the_output_untouched(void)
{
    freq_meter_t m;
    uint32_t ticks[4];
    uint32_t hz = 777u;
    uint32_t i;

    GROUP("凑不够跨度时");

    make_edges(0u, 1000u, 4u, ticks);   /* 只有 3 个周期 = 3000 tick < 4096 */
    freq_meter_reset(&m);
    for (i = 0u; i < 4u; i++) {
        CHECK(freq_meter_feed(&m, ticks[i], &hz) == FREQ_PENDING, "跨度不够应报 PENDING");
    }
    CHECK(hz == 777u, "PENDING 期间不得改写输出，实测 %u", (unsigned)hz);
}

static void test_lowest_measurable_period(void)
{
    freq_meter_t m;
    uint32_t ticks[8];
    uint32_t hz = 0u;

    GROUP("最低可测频率");

    /* ARR=65535 → 一次回绕能覆盖的最大跨度就是 65536 tick，
     * 对应 65.536 ms 的周期 ≈ 15.26 Hz。用 32768 tick（30.5 Hz）
     * 走一遍，确认这一档是对的。 */
    make_edges(0u, 32768u, 8u, ticks);
    freq_meter_reset(&m);

    CHECK(feed_until_done(&m, ticks, 8u, &hz) == 1, "应当测得出");
    /* 2 个周期 = 65536 tick 时会回绕，所以会在第 3 个边沿出结果：
     * 2 个周期跨 65536 tick → 但 65536 不是一个合法跨度（回绕后 first==last）。
     * 实测走的是 3 个边沿那条路，这里只断言它落在合理范围。 */
    CHECK(hz >= 30u && hz <= 31u, "32768 tick 周期应约 30 Hz，实测 %u", (unsigned)hz);
}

/* ══════════════════════════════════════════════════════════════════ */

int main(void)
{
    test_one_khz();
    test_hundred_khz();
    test_wraparound();
    test_edges_on_the_same_tick_are_too_fast();
    test_a_single_edge_is_not_a_measurement();
    test_pending_leaves_the_output_untouched();
    test_lowest_measurable_period();

    return test_summary();
}
