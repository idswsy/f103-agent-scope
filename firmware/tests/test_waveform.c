/* tests/test_waveform.c —— 「采集窗 → 屏幕一帧」的测试（不需要板子）
 *
 * 这一层全是纯算术，所以边界条件能在 PC 上穷举干净 —— 而它们正是会出错的地方：
 * 窗口跨过环尾回绕、窗口比屏幕列数还短、直流（峰峰值为 0 会让纵轴刻度除零）、
 * 窄尖峰被抽点丢掉。这些在真机上都要靠"摆一个恰好那样的信号"才能碰到。
 */

#include "waveform.h"
#include "test_util.h"

#include <string.h>

/* ══════════════════════════════════════════════════════════════════
 * 假 HAL —— 只为了测 `wave_build()` 这层接线
 * ══════════════════════════════════════════════════════════════════ */

static uint16_t g_ring[ACQ_RING_SAMPLES];

static void m_write(const uint8_t *d, uint32_t n) { (void)d; (void)n; }
static uint32_t m_read(uint8_t *b, uint32_t c) { (void)b; (void)c; return 0; }
static uint32_t m_now_ms(void) { return 0u; }
static uint32_t m_tick_us(void) { return 0u; }
static void m_start(uint32_t hz) { (void)hz; }
static void m_stop(uint32_t extra) { (void)extra; }
static const uint16_t *m_ring(void) { return g_ring; }
static uint8_t m_take(void) { return 0u; }
static uint32_t m_quantize(uint32_t hz) { return hz; }

static const hal_t MOCK = {
    .link_write = m_write,
    .link_read = m_read,
    .now_ms = m_now_ms,
    .quantize_rate_hz = m_quantize,
    .acq_start = m_start,
    .acq_stop = m_stop,
    .ring_base = m_ring,
    .take_published_halves = m_take,
    .tick_us = m_tick_us,
};

/* ══════════════════════════════════════════════════════════════════
 * 小工具
 * ══════════════════════════════════════════════════════════════════ */

/* 用一段样点铺满一个环，窗口从 0 开始。 */
static void load(const uint16_t *v, uint32_t n, uint32_t ring_len)
{
    uint32_t i;
    for (i = 0u; i < ring_len; i++) {
        g_ring[i] = v[i % n];
    }
}

/* 每列的 top 都必须 >= bot，且都落在波形区里。 */
static void check_columns_sane(const wave_frame_t *f, const char *what)
{
    uint32_t c;
    int bad = 0;
    for (c = 0u; c < WAVE_COLUMNS; c++) {
        if (f->top[c] >= WAVE_ROWS || f->bot[c] >= WAVE_ROWS || f->top[c] < f->bot[c]) {
            bad = 1;
        }
    }
    CHECK(!bad, "%s: 每列的上下沿都应在 0..%u 且 top >= bot", what, WAVE_ROWS - 1u);
}

/* ══════════════════════════════════════════════════════════════════
 * 纵轴映射
 * ══════════════════════════════════════════════════════════════════ */

static void test_flat_window_stays_off_the_bottom(void)
{
    uint16_t v[64];
    wave_frame_t f;
    uint32_t i;
    int all_mid = 1;

    GROUP("直流窗口");

    /* 一段 2.6 V 的直流 —— 真机上没接信号时就是这个样子。
     * 它绝不能画在底边：底边是"信号为 0"的视觉语言。 */
    for (i = 0u; i < 64u; i++) {
        v[i] = 3314u;
    }
    load(v, 64u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 64u, 857142u, &f);

    CHECK(f.valid, "直流窗口也应产出一帧");
    CHECK(f.vpp_lsb == 0u, "直流窗口的峰峰值应为 0，实测 %u", (unsigned)f.vpp_lsb);
    CHECK(f.freq_hz == 0u, "直流窗口测不出频率，应报 0，实测 %u", (unsigned)f.freq_hz);
    for (i = 0u; i < WAVE_COLUMNS; i++) {
        if (f.top[i] != WAVE_ROWS / 2u || f.bot[i] != WAVE_ROWS / 2u) {
            all_mid = 0;
        }
    }
    CHECK(all_mid, "直流应画在中线 y=%u 上，而不是贴底边", WAVE_ROWS / 2u);
    check_columns_sane(&f, "直流");
}

static void test_a_single_sample_spike_survives(void)
{
    uint16_t v[1000];
    wave_frame_t f;
    uint32_t i;

    GROUP("窄尖峰");

    for (i = 0u; i < 1000u; i++) {
        v[i] = 1000u;
    }
    v[500] = 4000u; /* 一个样点的毛刺 */

    load(v, 1000u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 1000u, 857142u, &f);

    CHECK(f.valid, "应产出一帧");
    CHECK(f.vpp_lsb == 3000u, "峰峰值应含那个毛刺 = 3000，实测 %u", (unsigned)f.vpp_lsb);
    /* 第 500 号样点落在第 50 列（每列 10 个样点）。取单点的话它会整个消失。 */
    CHECK(f.top[50] == WAVE_ROWS - 1u,
          "毛刺所在列 50 的顶沿应到顶 %u，实测 %u", WAVE_ROWS - 1u, (unsigned)f.top[50]);
    CHECK(f.bot[50] == 0u, "该列底沿应在 0，实测 %u", (unsigned)f.bot[50]);
    CHECK(f.top[49] <= 1u, "相邻列 49 不该被抬高，实测顶沿 %u", (unsigned)f.top[49]);
    check_columns_sane(&f, "窄尖峰");
}

/* ══════════════════════════════════════════════════════════════════
 * 频率
 * ══════════════════════════════════════════════════════════════════ */

static void test_square_wave_frequency(void)
{
    uint16_t v[1000];
    wave_frame_t f;
    uint32_t i;

    GROUP("方波频率");

    /* 1000 个样点、周期 100 → 10 个周期；采样率 100 kHz → 1000 Hz。
     * 全部取整数，所以期望值是精确的，不需要容差。 */
    for (i = 0u; i < 1000u; i++) {
        v[i] = ((i % 100u) < 50u) ? 3000u : 1000u;
    }

    load(v, 1000u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 1000u, 100000u, &f);

    CHECK(f.vpp_lsb == 2000u, "峰峰值应为 2000，实测 %u", (unsigned)f.vpp_lsb);
    CHECK(f.freq_hz == 1000u, "频率应为 1000 Hz，实测 %u", (unsigned)f.freq_hz);
}

static void test_insufficient_periods_reports_zero(void)
{
    uint16_t v[40];
    wave_frame_t f;
    uint32_t i;

    GROUP("不足一个周期");

    /* 40 个样点里只有半个周期 —— 报不出频率时必须报 0，
     * 而不是拿半个周期去外推一个数出来。 */
    for (i = 0u; i < 40u; i++) {
        v[i] = (i < 20u) ? 1000u : 3000u;
    }
    load(v, 40u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 40u, 100000u, &f);

    CHECK(f.freq_hz == 0u, "不足两个穿越点时应报 0，实测 %u", (unsigned)f.freq_hz);
}

static void test_tiny_ripple_is_not_a_frequency(void)
{
    uint16_t v[500];
    wave_frame_t f;
    uint32_t i;

    GROUP("峰峰值太小不测频");

    /* 一条直流上叠 ±8 LSB 的纹波。**这一条测的是峰峰值阈值**，不是迟滞 ——
     * 变异验证：把 `WAVE_MIN_SPAN_LSB` 的判断去掉，它会报出 50000 Hz
     * （正好采样率的一半，即每个样点都算一次穿越）。
     *
     * 迟滞另有专测，见 `test_midband_dither_does_not_fake_crossings`。 */
    for (i = 0u; i < 500u; i++) {
        v[i] = (uint16_t)(2000 + ((i % 2u) ? 8 : -8));
    }
    load(v, 500u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 500u, 100000u, &f);

    CHECK(f.vpp_lsb == 16u, "纹波峰峰值应为 16，实测 %u", (unsigned)f.vpp_lsb);
    CHECK(f.freq_hz == 0u,
          "峰峰值 %u < 阈值 %u，应判为直流不测频，实测报出 %u Hz",
          (unsigned)f.vpp_lsb, (unsigned)WAVE_MIN_SPAN_LSB, (unsigned)f.freq_hz);
}

static void test_midband_dither_does_not_fake_crossings(void)
{
    uint16_t v[1000];
    wave_frame_t f;
    uint32_t i;

    GROUP("中点抖动");

    /* 三角波：周期 500 样点 → 200 Hz，斜率每样点 12 LSB。
     * 叠上 ±60 LSB 的抖动 —— **抖动幅度五倍于斜率**，所以在中点附近
     * 每个样点都在来回穿越。
     *
     * 没有迟滞带的话，`n_cross` 会在中点附近被数出好几倍，而
     * `freq = (n-1) × rate / (last - first)` 里分子暴涨、分母只挪一点，
     * 报出来的频率会是真值的若干倍。
     *
     * 迟滞带 = 峰峰值/16 ≈ 195 LSB，±60 的抖动装在里面，穿越点干净。 */
    for (i = 0u; i < 1000u; i++) {
        uint32_t ph = i % 500u;
        int32_t base = (ph < 250u) ? (int32_t)(1000u + ph * 12u)
                                   : (int32_t)(1000u + (500u - ph) * 12u);
        int32_t dither = (i & 1u) ? 60 : -60;
        v[i] = (uint16_t)(base + dither);
    }

    load(v, 1000u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 1000u, 100000u, &f);

    /* 真值 200 Hz。±5% 的余量给插值量化（1/16 样点）与抖动带来的位置偏移。 */
    CHECK(f.freq_hz >= 190u && f.freq_hz <= 210u,
          "应为 200 Hz ±5%%，实测 %u Hz", (unsigned)f.freq_hz);
}

/* ══════════════════════════════════════════════════════════════════
 * 窗口形状
 * ══════════════════════════════════════════════════════════════════ */

static void test_window_wrapping_the_ring(void)
{
    uint16_t v[200];
    wave_frame_t direct;
    wave_frame_t wrapped;
    uint32_t i;
    int same = 1;

    GROUP("窗口跨环尾回绕");

    for (i = 0u; i < 200u; i++) {
        v[i] = (uint16_t)(1000u + i * 5u);
    }

    /* 参照：同一段样点放在环开头、不跨尾 */
    load(v, 200u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 200u, 100000u, &direct);

    /* 把同样 200 个样点摆到环尾，让窗口正好跨过 4096 的边界 */
    for (i = 0u; i < 200u; i++) {
        g_ring[(4000u + i) % ACQ_RING_SAMPLES] = v[i];
    }
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 4000u, 200u, 100000u, &wrapped);

    CHECK(direct.valid && wrapped.valid, "两种摆法都应产出有效帧");
    CHECK(direct.vpp_lsb == wrapped.vpp_lsb,
          "跨环尾与不跨环尾应得到同一个峰峰值：%u vs %u",
          (unsigned)direct.vpp_lsb, (unsigned)wrapped.vpp_lsb);
    for (i = 0u; i < WAVE_COLUMNS; i++) {
        if (direct.top[i] != wrapped.top[i] || direct.bot[i] != wrapped.bot[i]) {
            same = 0;
        }
    }
    CHECK(same, "跨环尾与不跨环尾应画出逐列相同的波形");
}

static void test_window_shorter_than_columns(void)
{
    uint16_t v[7];
    wave_frame_t f;
    uint32_t i;

    GROUP("窗口比列数还短");

    for (i = 0u; i < 7u; i++) {
        v[i] = (uint16_t)(1000u + i * 100u);
    }
    load(v, 7u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 7u, 100000u, &f);

    CHECK(f.valid, "短窗也应产出一帧（被拉宽铺满，而不是留一片空白）");
    CHECK(f.vpp_lsb == 600u, "峰峰值应为 600，实测 %u", (unsigned)f.vpp_lsb);
    check_columns_sane(&f, "短窗");
}

static void test_single_sample_window(void)
{
    uint16_t v[1];
    wave_frame_t f;

    GROUP("一个样点的窗口");

    v[0] = 2500u;
    load(v, 1u, ACQ_RING_SAMPLES);
    wave_build_raw(g_ring, ACQ_RING_SAMPLES, 0u, 1u, 100000u, &f);

    CHECK(f.valid, "一个样点也应产出有效帧（而不是越界读）");
    CHECK(f.vpp_lsb == 0u, "一个样点的峰峰值应为 0");
    check_columns_sane(&f, "单样点");
}

/* ══════════════════════════════════════════════════════════════════
 * 接线：wave_build 的准入条件
 * ══════════════════════════════════════════════════════════════════ */

static void test_build_refuses_before_capture_is_ready(void)
{
    acq_t a;
    wave_frame_t f;

    GROUP("采集未就绪时");

    memset(&a, 0, sizeof(a));
    a.hal = &MOCK;
    a.capture_ready = false;
    a.window_len = 1024u;

    wave_build(&a, &f);
    CHECK(!f.valid, "capture_ready 为假时不得产出一帧（不能假装有数据）");

    /* 就绪之后同一条路应该走得通，且读的是环里的真样点 */
    memset(g_ring, 0, sizeof(g_ring));
    g_ring[3] = 3000u;
    g_ring[4] = 1000u;
    a.capture_ready = true;
    a.window_start = 3u;
    a.window_len = 2u;
    a.rate_hz = 100000u;

    wave_build(&a, &f);
    CHECK(f.valid, "就绪后应产出一帧");
    CHECK(f.vpp_lsb == 2000u,
          "应从 window_start 处读起：峰峰值期望 2000，实测 %u", (unsigned)f.vpp_lsb);
}

/* ══════════════════════════════════════════════════════════════════ */

int main(void)
{
    test_flat_window_stays_off_the_bottom();
    test_a_single_sample_spike_survives();
    test_square_wave_frequency();
    test_insufficient_periods_reports_zero();
    test_tiny_ripple_is_not_a_frequency();
    test_midband_dither_does_not_fake_crossings();
    test_window_wrapping_the_ring();
    test_window_shorter_than_columns();
    test_single_sample_window();
    test_build_refuses_before_capture_is_ready();

    return test_summary();
}
