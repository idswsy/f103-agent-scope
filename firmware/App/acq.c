/* App/acq.c —— 采集状态机的实现。设计说明见 acq.h。 */

#include "acq.h"

#include <string.h>

/* ── 内部工具 ─────────────────────────────────────────────────── */

/* 状态是否允许改配置。
 *
 * 与 `proto_cmd_allowed()` 是**两个层次的同一件事**：那边管「哪些命令能发进来」，
 * 这里管「这个状态下的对象能不能被改」。两层都要有 ——
 * 协议层可能被绕过（比如从调试口直接调），而状态机自己不能靠别人守规矩。 */
static bool config_allowed(const acq_t *a)
{
    return a->state != STATE_ARMED && a->state != STATE_STREAMING;
}

/* 采集窗要采够多少样点（触发点之前 + 之后）。 */
static uint32_t window_len_of(const acq_t *a)
{
    return a->capture_samples;
}

void acq_init(acq_t *a, const hal_t *hal)
{
    memset(a, 0, sizeof(*a));
    a->hal = hal;
    a->state = STATE_IDLE;

    /* 上电默认值。必须与 host/crates/sim 的 Config::default() 一致 ——
     * 主机侧有一批「省略就沿用现值」的语义，两边不一致会让
     * 「什么都没配」和「配过」产生不同的行为。
     *
     * ⚠ **触发模式是 `TRIG_MODE_AUTO`，不是 `NORMAL`。**
     * 边沿触发要求信号**先跌破 `level-16` 再升破 `level+16`**（见 trigger.c），
     * 所以一个**直流信号在任何电平下都不可能触发**：电平在它下方时它始终在
     * 高处、不会"从下往上穿过"；在它上方时它始终在低处、升不上去。
     * 上电默认成 `NORMAL` 会让「接上板子、什么都没接、点采集」这件事
     * **必然 2 s 超时**（2026-10-05 真机实测）。AUTO 约 200 ms 无触发即强制完成，
     * 先给出波形，用户再按需要切 `NORMAL` 去等真正的边沿。
     *
     * `protocol.h` 里 `TRIG_MODE_AUTO` 的注释本来就是「避免 Agent 永久阻塞」——
     * 默认成它是那个设计意图的落地。 */
    a->requested_rate_hz = ACQ_RATE_MAX_HZ;
    a->rate_hz = ACQ_RATE_MAX_HZ;
    a->capture_samples = 4096;
    a->acq_mode = ACQ_MODE_SINGLE;
    a->format = FMT_RAW16;
    a->decimation = 1;

    a->trig_cfg.mode = TRIG_MODE_AUTO;
    a->trig_cfg.source = 0;
    a->trig_cfg.edge = TRIG_EDGE_RISING;
    a->trig_cfg.level_lsb = 2048;
    a->trig_cfg.pre_samples = 2048;
    a->trig_cfg.holdoff_us = 1000;

    a->trigger_at = ACQ_NO_TRIGGER;
    a->next_capture_id = 1;
}

scope_state_t acq_state(const acq_t *a)
{
    return a->state;
}

/* ── 配置 ─────────────────────────────────────────────────────── */

acq_status_t acq_set_rate(acq_t *a, uint32_t requested_hz, uint32_t *actual_out)
{
    if (!config_allowed(a)) {
        return ACQ_ERR_BUSY;
    }
    if (requested_hz == 0) {
        return ACQ_ERR_PARAM;
    }

    uint32_t actual = a->hal->quantize_rate_hz(requested_hz);
    if (actual == 0) {
        /* 硬件说这个请求达不到。**不静默吸附到最近的档位** ——
         * 那会让「我设了 1 MHz」变成「设成了 857 kHz」而调用方不知道。 */
        return ACQ_ERR_PARAM;
    }
    a->requested_rate_hz = requested_hz;
    a->rate_hz = actual;
    if (actual_out) {
        *actual_out = actual;
    }
    return ACQ_OK;
}

acq_status_t acq_set_trigger(acq_t *a, const set_trigger_req_t *req)
{
    if (!config_allowed(a)) {
        return ACQ_ERR_BUSY;
    }
    if (req->level_lsb > TRIG_ADC_MAX_LSB) {
        /* 12-bit 比较器的量程。超了不夹住 —— 夹住会让「设 5000」变成
         * 「设 4095」而调用方不知道，然后奇怪为什么触发点不对。 */
        return ACQ_ERR_PARAM;
    }
    if (req->pre_samples > a->capture_samples) {
        /* 预触发样点比整个采集窗还长 —— 窗口里放不下，触发点会跑到窗外面。 */
        return ACQ_ERR_PARAM;
    }
    a->trig_cfg = *req;
    return ACQ_OK;
}

acq_status_t acq_set_acq(acq_t *a, const set_acq_req_t *req)
{
    if (!config_allowed(a)) {
        return ACQ_ERR_BUSY;
    }
    if (req->capture_samples == 0 || req->capture_samples > ACQ_RING_SAMPLES) {
        return ACQ_ERR_PARAM;
    }
    if (req->decimation == 0) {
        return ACQ_ERR_PARAM;
    }
    a->acq_mode = req->mode;
    a->capture_samples = req->capture_samples;
    a->format = req->format;
    a->decimation = req->decimation;
    return ACQ_OK;
}

acq_status_t acq_set_channel(acq_t *a, const set_channel_req_t *req)
{
    if (!config_allowed(a)) {
        return ACQ_ERR_BUSY;
    }
    /* 本板只有 `ch` 是有效的（决定下面两个作用在哪条线上）。
     * `enable` / `offset_lsb` 的**实际作用**在 Hardware/ 的采样路径里，
     * 不在这里 —— 采集状态机只记录，不假装自己能改模拟前端。
     * `range_idx` / `coupling` 是手拨开关，这里连记录都不做承诺。 */
    (void)req;
    return ACQ_OK;
}

/* ── 采集 ─────────────────────────────────────────────────────── */

acq_status_t acq_arm(acq_t *a)
{
    if (a->state != STATE_IDLE && a->state != STATE_DONE) {
        return ACQ_ERR_STATE;
    }

    /* 先把要交付的采集窗算清楚，再开 DMA —— 顺序反了的话，
     * 第一条块中断可能在我们还没准备好时就到了。 */
    a->window_len = window_len_of(a);
    a->window_start = 0;
    a->trigger_at = ACQ_NO_TRIGGER;
    a->total_samples = 0;
    a->overrun = false;
    a->capture_ready = false;

    a->hal->acq_start(a->rate_hz);

    /* 触发搜索要等**第一块真数据**到手才能初始化：初始置位状态由
     * 「第一个样点落在迟滞带哪一侧」决定，而此刻一个样点都还没有。
     *
     * 回归：这里曾经直接 `trig_reset(..., first_sample = 0)` —— 而 0 一定
     * 在 lo 以下，于是**每次搜索都凭空置位**，「一开始就在高电平」的信号
     * 会被当成一次上升沿。 */
    a->trig_started = false;

    a->armed_ms = a->hal->now_ms();
    a->state = STATE_ARMED;
    return ACQ_OK;
}

acq_status_t acq_stop(acq_t *a)
{
    /* 协议规定：**已停也回 Ack，不报错**。所以这里对状态不挑。 */
    if (a->state == STATE_ARMED || a->state == STATE_STREAMING) {
        a->hal->acq_stop(0);
    }
    a->state = STATE_IDLE;
    return ACQ_OK;
}

/* 收尾：停 DMA、冻结采集窗、算出窗口里的触发下标。 */
static void finish_capture(acq_t *a, bool triggered)
{
    a->hal->acq_stop(0);

    uint32_t end = a->total_samples;
    /* 窗口是「最后 window_len 个样点」——采样已经停了，环里最新的就是这些。
     * 但只有从触发点往前数得够长，触发点才在窗口里。 */
    uint32_t start = (end >= a->window_len) ? (end - a->window_len) : 0;
    a->window_start = start;
    a->window_len = end - start;
    a->capture_ready = (a->window_len > 0);

    a->capture_id = a->next_capture_id;
    a->next_capture_id = (uint16_t)(a->next_capture_id + 1u);
    if (a->next_capture_id == 0) {
        a->next_capture_id = 1; /* id 0 保留给「没有采集」 */
    }

    if (triggered && a->trigger_at != ACQ_NO_TRIGGER) {
        /* 触发点相对窗口起点的位置。**夹到窗口内** ——
         * 触发点可能落在窗口之前（预触发样点比实际采到的还多），
         * 那时它在窗里根本不存在，报出去会让主机去索引一个不存在的位置。 */
        a->trigger_index_in_window =
            (a->trigger_at >= start) ? (uint16_t)(a->trigger_at - start) : 0;
    } else {
        a->trigger_index_in_window = 0;
        a->trigger_at = ACQ_NO_TRIGGER;
    }

    a->state = STATE_DONE;
}

acq_event_t acq_poll(acq_t *a, acq_out_t *out)
{
    if (a->state != STATE_ARMED) {
        return ACQ_EV_NONE;
    }

    const uint16_t *ring = a->hal->ring_base();
    uint8_t published = a->hal->take_published_halves();

    /* **按顺序**逐块处理。先处理旧的那一半 —— 反过来的话，算出来的
     * 绝对样点号会跳，触发点在窗口里的位置就错了。 */
    for (int half = 0; half < 2; half++) {
        uint8_t bit = (half == 0) ? ACQ_HALF_FIRST : ACQ_HALF_SECOND;
        if (!(published & bit)) {
            continue;
        }

        const uint16_t *block = ring + (uint32_t)half * ACQ_HALF_SAMPLES;

        if (!a->trig_started) {
            /* 第一块数据到手 —— 现在才谈得上初始化。用这一块的第一个样点，
             * 不是缓冲区开头那个（两者在第二块之后就没关系了）。 */
            trig_reset(&a->trig, a->trig_cfg.edge == TRIG_EDGE_RISING,
                       a->trig_cfg.level_lsb, block[0]);
            a->trig_started = true;
        }

        if (a->trigger_at == ACQ_NO_TRIGGER) {
            /* 还没触发：在**这一块**里接着找。
             *
             * 注意状态是跨块保留的（`a->trig`）—— 触发点可能前半块置位、
             * 后半块才跨过。每块重置的话这种触发会被整块丢掉，
             * 而那条路径在 PC 上完全可测（见 tests/test_trigger.c 第五组）。 */
            uint32_t idx = 0;
            if (trig_scan(&a->trig, block, ACQ_HALF_SAMPLES, a->trig_cfg.level_lsb, &idx)) {
                a->trigger_at = a->total_samples + idx;
            }
        }

        a->total_samples += ACQ_HALF_SAMPLES;

        /* 已经触发：等采够「后置样点 + 停驻余量」就收尾。
         *
         * 后置样点 = 采集窗长度 − 预触发样点。 */
        if (a->trigger_at != ACQ_NO_TRIGGER) {
            uint32_t post = a->capture_samples - a->trig_cfg.pre_samples;
            uint32_t need = a->trigger_at + post + ACQ_STOP_MARGIN_SAMPLES;
            if (a->total_samples >= need) {
                finish_capture(a, true);
                out->kind = ACQ_EV_TRIGGERED;
                out->trigger.capture_id = a->capture_id;
                out->trigger.trigger_index = a->trigger_index_in_window;
                out->trigger.tick_us = a->hal->tick_us();
                out->trigger.rate_hz = a->rate_hz;
                out->trigger.n_samples = a->window_len;
                return ACQ_EV_TRIGGERED;
            }
        }

        /* 溢出：还没收尾，而已经推进的样点超过了环能装下的量 ——
         * 最早那些已经被 DMA 覆盖掉了，这份采集不再完整。 */
        if (!a->overrun && a->total_samples > ACQ_RING_SAMPLES) {
            uint32_t lost = a->total_samples - ACQ_RING_SAMPLES;
            a->overrun = true;
            a->overrun_samples += lost;
            out->kind = ACQ_EV_OVERRUN;
            out->overrun.dropped_samples = lost;
            out->overrun.capture_id = a->capture_id;
            /* 状态保持 ARMED —— 溢出不是错误，采集继续（协议里 OVERRUN
             * 是**事件**不是错误码）。 */
            return ACQ_EV_OVERRUN;
        }
    }

    /* auto 模式：约 200 ms 无触发就强制完成一次采集。
     *
     * **这一条必须有。** 没有它，总线静默时 Agent 的 capture 会永久阻塞 ——
     * 而「永远不返回」和「返回一个空采集」相比，后者对 Agent 有用得多。 */
    if (a->trig_cfg.mode == TRIG_MODE_AUTO &&
        a->hal->now_ms() - a->armed_ms >= ACQ_AUTO_TIMEOUT_MS) {
        finish_capture(a, false);
        out->kind = ACQ_EV_TRIGGERED;
        out->trigger.capture_id = a->capture_id;
        out->trigger.trigger_index = ACQ_NO_TRIGGER; /* 「没有触发过」的哨兵 */
        out->trigger.tick_us = a->hal->tick_us();
        out->trigger.rate_hz = a->rate_hz;
        out->trigger.n_samples = a->window_len;
        return ACQ_EV_TRIGGERED;
    }

    return ACQ_EV_NONE;
}

/* ── 采集窗访问 ───────────────────────────────────────────────── */

uint8_t acq_channel_count(const acq_t *a)
{
    /* 本板在 P0–P3 是「一路模拟 + 一路数字」——模拟通路给 1 条线，
     * 数字通路给的是**边沿时间戳**不是样点，所以采集窗里只有 1 个通道。
     * 真双通道要等 P4 改板（见 ADR-010）。 */
    (void)a;
    return 1;
}

uint16_t acq_sample(const acq_t *a, uint8_t ch, uint32_t i)
{
    if (!a->capture_ready || ch >= acq_channel_count(a) || i >= a->window_len) {
        return 0;
    }
    const uint16_t *ring = a->hal->ring_base();
    uint32_t abs = a->window_start + i;
    return ring[abs % ACQ_RING_SAMPLES];
}
