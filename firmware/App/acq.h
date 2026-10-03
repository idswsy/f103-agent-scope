/* App/acq.h —— 采集状态机（IDLE / ARMED / DONE / STREAMING / FAULT）
 *
 * # 它管什么
 *
 * 从「收到 ARM」到「发 EVENT_TRIGGER」之间的全部过程：
 *   1. 把配置下发给 Hardware/（采样率、触发、采集长度）
 *   2. 逐块领取已发布的半区，在里面搜触发点
 *   3. 找到之后继续采够后置样点，停 DMA，交出采集窗
 *   4. auto 模式超时则强制完成（**不能让它永久阻塞**）
 *
 * # 它不管什么
 *
 * 帧的解析与编码（那是 `proto/protocol.c`）、以及「哪个命令在哪个状态
 * 合法」（那是 `proto_cmd_allowed`）。这里只做采集本身。
 *
 * # 为什么状态要显式建模
 *
 * 真实的采集是**异步**的：ARM 之后要等硬件产出数据。把它写成阻塞等待
 * （「while(!triggered) {}」）会让整个设备失去响应 —— 连 STOP 都收不到。
 * 所以这里是一个**轮询式状态机**：`acq_poll()` 由主循环反复调用，
 * 每次都只做「现在能做的那么一点」。
 */

#ifndef APP_ACQ_H
#define APP_ACQ_H

#include "hal.h"
#include "protocol.h"
#include "trigger.h"

/* auto 模式约 200 ms 无触发就强制完成。
 *
 * 这个数字是**协议语义**，不是调优参数：没有它，Agent 的 capture 会永久阻塞。
 * 见 docs/03-protocol.md 的 `SET_TRIGGER.mode` 说明与 firmware/README.md。 */
#define ACQ_AUTO_TIMEOUT_MS 200u

/* 停 DMA 之前多采的样点数 —— 吸收停驻延迟，防止最后几个样点正被写就被读。
 * 见 firmware/README.md「关键数字」表。 */
#define ACQ_STOP_MARGIN_SAMPLES 512u

/* `event_trigger_t.trigger_index` 的「没有触发过」哨兵（协议规定）。 */
#define ACQ_NO_TRIGGER 0xFFFFFFFFu

/* 操作结果。 */
typedef enum {
    ACQ_OK = 0,
    /* ARMED / STREAMING 下不许改配置 —— 对应协议里的 BUSY 错误码。 */
    ACQ_ERR_BUSY,
    /* 参数越界。 */
    ACQ_ERR_PARAM,
    /* 当前状态不允许这个操作。 */
    ACQ_ERR_STATE,
} acq_status_t;

/* 采集状态机。调用方（`main`）持有一个实例。 */
typedef struct {
    const hal_t *hal;

    /* ── 状态 ── */
    scope_state_t state;

    /* ── 配置（SET_* 写入，GET_CONFIG 读出）── */
    uint32_t requested_rate_hz;
    uint32_t rate_hz;          /* 实际生效（量化后） */
    uint16_t capture_samples;
    uint8_t acq_mode;          /* acq_mode_t */
    uint8_t format;            /* wf_format_t */
    uint16_t decimation;
    set_trigger_req_t trig_cfg;

    /* ── 运行态 ── */
    trig_state_t trig;
    /* 触发搜索是否已经按**真样点**初始化过。
     *
     * ARM 的那一刻还没有样点，所以初始置位状态只能等第一块数据到手再定 ——
     * 而初始置位状态是「第一个样点落在迟滞带哪一侧」决定的，猜不得。
     * 回归：这里曾经在 ARM 时就 `trig_reset(..., first_sample=0)`，
     * 而 0 一定在 lo 以下 → **每次搜索都凭空置位**，于是「一开始就在高电平」
     * 的信号会被当成一次上升沿。 */
    bool trig_started;
    /* 自 ARM 以来累计推进的样点数（绝对坐标）。 */
    uint32_t total_samples;
    /* 这次采集的 id。 */
    uint16_t capture_id;
    uint16_t next_capture_id;
    /* 触发点在绝对坐标里的位置；`ACQ_NO_TRIGGER` = 还没触发。 */
    uint32_t trigger_at;
    /* 采集窗的起点（绝对坐标）与长度。 */
    uint32_t window_start;
    uint32_t window_len;
    /* 触发点在**窗口内**的相对下标（0 起）。窗口里没有触发点时为 0 ——
     * 那种情况靠 `trigger_at == ACQ_NO_TRIGGER` 区分，不是靠这个 0。 */
    uint16_t trigger_index_in_window;
    /* ARM 的时刻，用于 auto 模式超时。 */
    uint32_t armed_ms;
    /* 这次采集有没有溢出过。 */
    bool overrun;
    /* 累计溢出样点数（GET_STATUS 上报）。 */
    uint32_t overrun_samples;
    /* 最近一次错误码（GET_STATUS 上报）。 */
    uint16_t last_error_code;
    /* 已冻结的采集是否可用（DONE 之后为真）。 */
    bool capture_ready;
} acq_t;

/* `acq_poll()` 的产出：有没有事情要上报给协议层。 */
typedef enum {
    ACQ_EV_NONE = 0,
    /* 采集完成 —— 发 EVENT_TRIGGER。 */
    ACQ_EV_TRIGGERED,
    /* 采集期间发生溢出 —— 发 EVENT_OVERRUN。 */
    ACQ_EV_OVERRUN,
} acq_event_t;

/* 事件载荷。`kind` 决定哪个字段有效。 */
typedef struct {
    acq_event_t kind;
    event_trigger_t trigger;
    event_overrun_t overrun;
} acq_out_t;

/* 初始化（上电状态：IDLE，配置取默认值）。 */
void acq_init(acq_t *a, const hal_t *hal);

/* 当前状态。 */
scope_state_t acq_state(const acq_t *a);

/* `SET_SAMPLE_RATE`。把请求值交给硬件量化，回填实际生效值。
 * 返回后 `*actual_out` 一定有值（失败时是保持不变的旧值）。 */
acq_status_t acq_set_rate(acq_t *a, uint32_t requested_hz, uint32_t *actual_out);

/* `SET_TRIGGER`。校验参数，写进配置。 */
acq_status_t acq_set_trigger(acq_t *a, const set_trigger_req_t *req);

/* `SET_ACQ`。校验参数，写进配置。 */
acq_status_t acq_set_acq(acq_t *a, const set_acq_req_t *req);

/* `SET_CHANNEL`。本板只有 `enable` 与 `offset_lsb` 是软件可控的，
 * 耦合与量程是手拨开关（见 docs/02-hardware.md §5）—— 这一层照单收下并记录，
 * 但**不做任何承诺**。 */
acq_status_t acq_set_channel(acq_t *a, const set_channel_req_t *req);

/* `ARM`。启动一次采集。 */
acq_status_t acq_arm(acq_t *a);

/* `STOP`。任何时候都能停，停完回 IDLE —— **不报错**（协议规定）。 */
acq_status_t acq_stop(acq_t *a);

/* 主循环反复调用：领取已发布的半区、搜触发、必要时收尾。
 *
 * 返回 `ACQ_EV_NONE` 表示这次没事。同一轮里只会返回一个事件 ——
 * 调用方处理完再调一次即可（那时会把剩下的事件取出来）。 */
acq_event_t acq_poll(acq_t *a, acq_out_t *out);

/* 取采集窗里某通道的第 `i` 个样点。
 *
 * `ch` 对应当前硬件采到的通道数；超出返回 0。
 * 只应在 `capture_ready` 为真时调用。 */
uint16_t acq_sample(const acq_t *a, uint8_t ch, uint32_t i);

/* 采集窗里有多少个通道。 */
uint8_t acq_channel_count(const acq_t *a);

#endif /* APP_ACQ_H */
