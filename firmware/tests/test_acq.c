/* tests/test_acq.c —— 采集状态机的测试（用假 HAL，不需要板子）
 *
 * 这个文件本身就是 ADR-008 那条纪律的证据：整个采集状态机 —— 状态迁移、
 * 触发搜索、跨块、auto 超时、溢出 —— 全都在 PC 上跑得起来，
 * 而 `App/acq.c` 里**一个寄存器都没碰**。
 *
 * 三个人里只有一个人拿得到板子，另外两个人靠这个文件照样能改采集逻辑。
 */

#include "acq.h"
#include "test_util.h"

#include <string.h>

/* ══════════════════════════════════════════════════════════════════
 * 假 HAL
 * ══════════════════════════════════════════════════════════════════ */

static uint16_t g_ring[ACQ_RING_SAMPLES];
static uint8_t g_pub;
static uint32_t g_ms;
static uint32_t g_tick;
static bool g_dma_running;
static uint32_t g_started_rate;
static uint32_t g_stop_extra;
static uint32_t g_next_half; /* 下一次 feed 该写哪个半区 */

static void m_write(const uint8_t *d, uint32_t n) { (void)d; (void)n; }
static uint32_t m_read(uint8_t *b, uint32_t c) { (void)b; (void)c; return 0; }
static uint32_t m_now_ms(void) { return g_ms; }
static uint32_t m_tick_us(void) { return g_tick; }
static void m_start(uint32_t hz) { g_dma_running = true; g_started_rate = hz; }
static void m_stop(uint32_t extra) { g_dma_running = false; g_stop_extra = extra; }
static const uint16_t *m_ring(void) { return g_ring; }

/* 领取已发布的半区。真实现里这一步要在关中断的临界区里做；
 * 单线程的测试里读写本身已经是原子的。 */
static uint8_t m_take(void)
{
    uint8_t v = g_pub;
    g_pub = 0;
    return v;
}

/* 量化模型与真硬件同构：72 MHz / (ARR+1)，只出整数分频档位。
 *
 * 取**最近的**周期，不是向下取整 —— 向下取整会让「请求 900000」
 * 落到 878048 Hz…… 那还好，但「请求 100001」会落到 100000 或 101408，
 * 少一步取整就吸附到次近的档位去了。 */
static uint32_t m_quantize(uint32_t hz)
{
    if (hz == 0 || hz > ACQ_RATE_MAX_HZ) {
        return 0;
    }
    /* 四舍五入而不是截断；+hz/2 是整数版的四舍五入 */
    uint32_t period = (72000000u + hz / 2u) / hz;
    if (period == 0) {
        period = 1;
    }
    uint32_t actual = 72000000u / period;
    /* 吸附之后仍然超出物理上限 → 达不到，报错（不静默给个更快的档位） */
    if (actual == 0 || actual > ACQ_RATE_MAX_HZ) {
        return 0;
    }
    return actual;
}

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

static void mock_reset(void)
{
    memset(g_ring, 0, sizeof(g_ring));
    g_pub = 0;
    g_ms = 0;
    g_tick = 1000;
    g_dma_running = false;
    g_started_rate = 0;
    g_stop_extra = 0;
    g_next_half = 0;
}

/* 往环里填一个半区并「发布」它。
 * 交替填 0/1 —— 与 acq_poll 的处理顺序（先前半再后半）一致。 */
static void feed(uint16_t (*gen)(uint32_t i))
{
    uint32_t base = g_next_half * ACQ_HALF_SAMPLES;
    for (uint32_t i = 0; i < ACQ_HALF_SAMPLES; i++) {
        g_ring[base + i] = gen(i);
    }
    g_pub |= (g_next_half == 0) ? ACQ_HALF_FIRST : ACQ_HALF_SECOND;
    g_next_half ^= 1u;
}

/* 一直跑 poll 直到拿到事件或轮数用尽。 */
static acq_event_t pump(acq_t *a, acq_out_t *out, int max_rounds)
{
    for (int i = 0; i < max_rounds; i++) {
        acq_event_t e = acq_poll(a, out);
        if (e != ACQ_EV_NONE) {
            return e;
        }
    }
    return ACQ_EV_NONE;
}

/* ── 生成器 ───────────────────────────────────────────────────── */

static uint16_t gen_flat_low(uint32_t i) { (void)i; return 1000; }
static uint16_t gen_flat_high(uint32_t i) { (void)i; return 3000; }
/* 前半低、后半高：在一个半区内部完成一次上升沿 */
static uint16_t gen_step_up(uint32_t i) { return (i < ACQ_HALF_SAMPLES / 2) ? 1000 : 3000; }

/* ══════════════════════════════════════════════════════════════════
 * 用例
 * ══════════════════════════════════════════════════════════════════ */

static void case_arm_and_trigger(void)
{
    GROUP("ARM → 找到触发 → DONE");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);
    acq_out_t out;

    CHECK(acq_state(&a) == STATE_IDLE, "上电应当是 IDLE");

    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 2048,
                             .format = FMT_RAW16,
                             .decimation = 1};
    CHECK(acq_set_acq(&a, &acq_req) == ACQ_OK, "设置采集参数应当成功");

    set_trigger_req_t tr = {.mode = TRIG_MODE_NORMAL,
                            .source = TRIG_SRC_CH1,
                            .edge = TRIG_EDGE_RISING,
                            .level_lsb = 2048,
                            .pre_samples = 1024,
                            .holdoff_us = 1000};
    CHECK(acq_set_trigger(&a, &tr) == ACQ_OK, "设置触发应当成功");

    CHECK(acq_arm(&a) == ACQ_OK, "ARM 应当成功");
    CHECK(acq_state(&a) == STATE_ARMED, "ARM 后应当是 ARMED");
    CHECK(g_dma_running, "ARM 应当真的把 DMA 开起来");

    /* 喂几块上升沿数据 */
    feed(gen_step_up);
    feed(gen_step_up);
    feed(gen_step_up);
    feed(gen_step_up);

    acq_event_t e = pump(&a, &out, 16);
    CHECK(e == ACQ_EV_TRIGGERED, "喂了上升沿却没触发（拿到 %d）", (int)e);
    if (e == ACQ_EV_TRIGGERED) {
        CHECK(!g_dma_running, "触发收尾时应当停掉 DMA");
        CHECK(out.trigger.n_samples == 2048, "窗口长度应为 2048，实测 %u",
              out.trigger.n_samples);
        CHECK(out.trigger.rate_hz == ACQ_RATE_MAX_HZ, "速率应当回填实际值");
        CHECK(acq_state(&a) == STATE_DONE, "收尾后应当是 DONE");
    }
}

/* ── 一开始就在高电平的信号不得触发 ─────────────────────────────
 *
 * 回归：`acq_arm` 曾经直接 `trig_reset(..., first_sample = 0)` ——
 * 而 0 一定在 lo 以下，于是**每次搜索都凭空置位**。
 * 结果是「信号一直待在高处、压根没升上来」被当成一次上升沿。
 */
static void case_already_high_never_triggers(void)
{
    GROUP("一开始就在高电平 → 不触发");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);
    acq_out_t out;

    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 2048,
                             .format = FMT_RAW16,
                             .decimation = 1};
    acq_set_acq(&a, &acq_req);
    set_trigger_req_t tr = {.mode = TRIG_MODE_NORMAL,
                            .source = TRIG_SRC_CH1,
                            .edge = TRIG_EDGE_RISING,
                            .level_lsb = 2048,
                            .pre_samples = 1024,
                            .holdoff_us = 1000};
    acq_set_trigger(&a, &tr);
    acq_arm(&a);

    for (int i = 0; i < 4; i++) {
        feed(gen_flat_high);
    }
    CHECK(pump(&a, &out, 16) == ACQ_EV_NONE,
          "一直在高电平的信号被当成了上升沿 —— 初始置位算错了");
    CHECK(acq_state(&a) == STATE_ARMED, "应当还在等触发");
}

/* ── 跨块的触发 ─────────────────────────────────────────────────
 *
 * 前半块信号跌到 lo 以下置位、后半块才升过 hi。
 * 每块重置触发状态的实现会把这个触发**整块丢掉**。
 */
static void case_trigger_spanning_two_halves(void)
{
    GROUP("触发跨块");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);
    acq_out_t out;

    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 2048,
                             .format = FMT_RAW16,
                             .decimation = 1};
    acq_set_acq(&a, &acq_req);
    set_trigger_req_t tr = {.mode = TRIG_MODE_NORMAL,
                            .source = TRIG_SRC_CH1,
                            .edge = TRIG_EDGE_RISING,
                            .level_lsb = 2048,
                            .pre_samples = 1024,
                            .holdoff_us = 1000};
    acq_set_trigger(&a, &tr);
    acq_arm(&a);

    /* 块 A：先高后低。末尾跌到 lo 以下 → **置位**，但不触发
     * （置位之后没有再升过 hi）。
     * 块 B：一直高 → 第一个样点就跨过 hi，触发点在**块 B 内**。
     *
     * 这就是「一次触发跨块」：置位在块 A、跨过在块 B。
     * 每块重置触发状态的实现会把这个触发整块丢掉。 */
    {
        for (uint32_t i = 0; i < ACQ_HALF_SAMPLES; i++) {
            g_ring[i] = (i < ACQ_HALF_SAMPLES / 2) ? 3000 : 1000;
        }
        g_pub |= ACQ_HALF_FIRST;
        g_next_half = 1;
    }
    acq_poll(&a, &out); /* 处理块 A：只置位 */
    CHECK(acq_state(&a) == STATE_ARMED, "块 A 不该触发");

    feed(gen_flat_high); /* 块 B：跨过 hi */
    acq_event_t e = pump(&a, &out, 64);
    CHECK(e == ACQ_EV_TRIGGERED,
          "跨块触发丢了 —— 状态没有跨块保留（拿到 %d）", (int)e);

    if (e == ACQ_EV_TRIGGERED) {
        CHECK(out.trigger.trigger_index != ACQ_NO_TRIGGER, "应当是一次真触发");
        CHECK(out.trigger.trigger_index < out.trigger.n_samples,
              "触发下标 %u 超出了窗口长度 %u",
              out.trigger.trigger_index, out.trigger.n_samples);
    }
}

static void case_auto_mode_forces_completion(void)
{
    GROUP("auto 模式超时强制完成");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);
    acq_out_t out;

    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 1024,
                             .format = FMT_RAW16,
                             .decimation = 1};
    acq_set_acq(&a, &acq_req);
    /* auto 模式 —— 总线静默时的逃生通道 */
    set_trigger_req_t tr = {.mode = TRIG_MODE_AUTO,
                            .source = TRIG_SRC_CH1,
                            .edge = TRIG_EDGE_RISING,
                            .level_lsb = 2048,
                            .pre_samples = 512,
                            .holdoff_us = 1000};
    acq_set_trigger(&a, &tr);
    acq_arm(&a);

    /* 喂一块平的（不触发），时间往前走 */
    feed(gen_flat_low);
    CHECK(pump(&a, &out, 4) == ACQ_EV_NONE, "超时之前不该完成");

    g_ms += ACQ_AUTO_TIMEOUT_MS; /* 超时 */
    acq_event_t e = pump(&a, &out, 8);
    CHECK(e == ACQ_EV_TRIGGERED, "auto 模式超时应当强制完成（拿到 %d）", (int)e);
    if (e == ACQ_EV_TRIGGERED) {
        CHECK(out.trigger.trigger_index == ACQ_NO_TRIGGER,
              "强制完成的采集不该谎报一个触发点（实测 %u）",
              out.trigger.trigger_index);
        CHECK(acq_state(&a) == STATE_DONE, "应当进 DONE");
    }
}

static void case_stop_is_always_allowed(void)
{
    GROUP("STOP 任何时候都允许");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);

    /* 没 ARM 过就 STOP —— 协议规定「已停也回 Ack，不报错」 */
    CHECK(acq_stop(&a) == ACQ_OK, "IDLE 下 STOP 不该报错");
    CHECK(acq_state(&a) == STATE_IDLE, "STOP 后应当是 IDLE");

    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 1024,
                             .format = FMT_RAW16,
                             .decimation = 1};
    acq_set_acq(&a, &acq_req);
    acq_arm(&a);
    CHECK(acq_state(&a) == STATE_ARMED, "ARM 后应当是 ARMED");

    CHECK(acq_stop(&a) == ACQ_OK, "ARMED 下 STOP 应当成功");
    CHECK(acq_state(&a) == STATE_IDLE, "STOP 后应当回 IDLE");
    CHECK(!g_dma_running, "STOP 应当真的把 DMA 关掉");
}

static void case_config_rejected_while_armed(void)
{
    GROUP("ARMED 时改配置 → BUSY");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);

    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 1024,
                             .format = FMT_RAW16,
                             .decimation = 1};
    acq_set_acq(&a, &acq_req);
    acq_arm(&a);

    set_trigger_req_t tr = {.mode = TRIG_MODE_AUTO,
                            .source = TRIG_SRC_CH1,
                            .edge = TRIG_EDGE_RISING,
                            .level_lsb = 1000,
                            .pre_samples = 512,
                            .holdoff_us = 1000};
    CHECK(acq_set_trigger(&a, &tr) == ACQ_ERR_BUSY, "ARMED 下改触发应当回 BUSY");
    CHECK(acq_set_acq(&a, &acq_req) == ACQ_ERR_BUSY, "ARMED 下改采集应当回 BUSY");

    uint32_t actual = 0;
    CHECK(acq_set_rate(&a, 100000, &actual) == ACQ_ERR_BUSY,
          "ARMED 下改采样率应当回 BUSY");

    acq_stop(&a);
    CHECK(acq_set_trigger(&a, &tr) == ACQ_OK, "STOP 之后应当能改了");
}

static void case_bad_params_rejected(void)
{
    GROUP("参数越界被拒（不静默夹取）");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);

    /* 采集点数超环容量 */
    set_acq_req_t bad = {.mode = ACQ_MODE_SINGLE,
                         .capture_samples = ACQ_RING_SAMPLES + 1,
                         .format = FMT_RAW16,
                         .decimation = 1};
    CHECK(acq_set_acq(&a, &bad) == ACQ_ERR_PARAM, "超环容量的点数应当被拒");

    /* 抽点倍数为 0 */
    bad.capture_samples = 1024;
    bad.decimation = 0;
    CHECK(acq_set_acq(&a, &bad) == ACQ_ERR_PARAM, "decimation=0 应当被拒");

    /* 触发电平超出 12-bit */
    set_trigger_req_t tr = {.mode = TRIG_MODE_NORMAL,
                            .source = TRIG_SRC_CH1,
                            .edge = TRIG_EDGE_RISING,
                            .level_lsb = 5000,
                            .pre_samples = 512,
                            .holdoff_us = 1000};
    CHECK(acq_set_trigger(&a, &tr) == ACQ_ERR_PARAM, "电平 5000 应当被拒");

    /* 预触发比整个窗口还长。
     * 先把窗口缩到 512 —— 上面那条 decimation=0 是被拒的，
     * 所以此刻 capture_samples 还是默认的 4096，直接拿 4096 比是比不出来的。 */
    set_acq_req_t small = {.mode = ACQ_MODE_SINGLE,
                           .capture_samples = 512,
                           .format = FMT_RAW16,
                           .decimation = 1};
    CHECK(acq_set_acq(&a, &small) == ACQ_OK, "先设一个小窗口");
    tr.level_lsb = 2048;
    tr.pre_samples = 4096;
    CHECK(acq_set_trigger(&a, &tr) == ACQ_ERR_PARAM,
          "预触发（4096）超过窗口（512）应当被拒");

    /* 达不到的采样率 */
    uint32_t actual = 0;
    CHECK(acq_set_rate(&a, 10000000, &actual) == ACQ_ERR_PARAM,
          "超过硬件上限的采样率应当被拒，而不是吸附到最近的档位");
}

static void case_rate_is_quantized_and_echoed(void)
{
    GROUP("采样率量化后回显");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);

    /* 用一个**在量程内、但落在两个定时器档位之间**的值。
     * 857143 不行 —— 它超出 ADC 上限，按协议应当直接被拒（主机侧也是这么做的，
     * 见 host/crates/mcp 的 configure_rejects_a_rate_above_the_single_adc_limit）。 */
    uint32_t actual = 0;
    CHECK(acq_set_rate(&a, 100001, &actual) == ACQ_OK, "100001 应当被接受");
    CHECK(actual == 100000, "100001 应当量化到 100000，实测 %u", actual);
    CHECK(actual != 100001, "这个值应当真的被量化过");

    /* 窗口里用的必须是**量化后**的值 —— 时间轴由它推导 */
    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 1024,
                             .format = FMT_RAW16,
                             .decimation = 1};
    acq_set_acq(&a, &acq_req);
    acq_arm(&a);
    CHECK(g_started_rate == 100000, "开 DMA 时用的应当是量化值，实测 %u",
          g_started_rate);
}

static void case_capture_window_is_addressable(void)
{
    GROUP("采集窗可寻址");

    mock_reset();
    acq_t a;
    acq_init(&a, &MOCK);
    acq_out_t out;

    set_acq_req_t acq_req = {.mode = ACQ_MODE_SINGLE,
                             .capture_samples = 1024,
                             .format = FMT_RAW16,
                             .decimation = 1};
    acq_set_acq(&a, &acq_req);
    set_trigger_req_t tr = {.mode = TRIG_MODE_AUTO,
                            .source = TRIG_SRC_CH1,
                            .edge = TRIG_EDGE_RISING,
                            .level_lsb = 2048,
                            .pre_samples = 512,
                            .holdoff_us = 1000};
    acq_set_trigger(&a, &tr);
    acq_arm(&a);

    feed(gen_flat_low);
    g_ms += ACQ_AUTO_TIMEOUT_MS;
    CHECK(pump(&a, &out, 8) == ACQ_EV_TRIGGERED, "应当完成");

    CHECK(acq_channel_count(&a) == 1, "本板只有 1 路模拟通道（真双通道要等 P4）");
    /* 窗内任意下标都不越界；越界不崩、返回 0 */
    CHECK(acq_sample(&a, 0, 0) == 1000, "窗内样点应当可读");
    CHECK(acq_sample(&a, 0, (1u << 30)) == 0, "越界下标应当返回 0 而不是崩");
    CHECK(acq_sample(&a, 5, 0) == 0, "不存在的通道应当返回 0");
}

int main(void)
{
    case_arm_and_trigger();
    case_already_high_never_triggers();
    case_trigger_spanning_two_halves();
    case_auto_mode_forces_completion();
    case_stop_is_always_allowed();
    case_config_rejected_while_armed();
    case_bad_params_rejected();
    case_rate_is_quantized_and_echoed();
    case_capture_window_is_addressable();
    return test_summary();
}
