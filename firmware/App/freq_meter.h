/* App/freq_meter.h —— 输入捕获测频的**纯算术**部分。
 *
 * # 为什么不直接用上游那套
 *
 * 上游的 `Freq_calibration`（`Hardware/src/scope_ui.c`）把结果**整定到 1 kHz
 * 的整数倍** —— 1.5 kHz 会读成 2 kHz。那是给本机那块小屏的粗显示用的，
 * 不能作为协议里报给主机的数。
 *
 * 而且它逐周期算：`freq = 1e6 / count`，两个问题 ——
 *   - `count == 0`（两次捕获落在同一个 tick）会除零
 *   - 只处理一次回绕（`0xFFFF - a + b`），低于 15.26 Hz 会给一个假数
 *
 * # 这里的做法：**自适应累计**
 *
 * 不逐周期出结果，而是累计若干个上升沿，等**跨度**够了（或边沿数到顶）
 * 再用 `(n-1) 个周期 / 跨度` 算一次。好处是高频段的量化误差不会失控 ——
 * 100 kHz 时单周期只有 10 µs，±1 tick 就是 ±10%；累计到 630 tick 就
 * 降到 ±0.16%。
 *
 * # 量程（由硬件决定，不是调参）
 *
 *   上限：两个边沿落在同一个 tick → 不可测。计时器是 1 MHz，所以
 *         理论极限是 1 MHz，但实际受中断开销限制，见 local_freq.c
 *   下限：**跨度必须容得下且不超过一次回绕**（ARR=65535）
 *         → 周期上限 65.536 ms → 15.26 Hz
 *
 * 低于下限**报"测不出"**，不给一个假的数。
 */

#ifndef APP_FREQ_METER_H
#define APP_FREQ_METER_H

#include <stdbool.h>
#include <stdint.h>

/* TIM3 的计数频率与自动重装值 —— 与 `Core/Src/tim.c` 的 `MX_TIM3_Init` 对应
 * （PSC=71 → 1 MHz，ARR=65535）。改那边就要改这里。 */
#define FREQ_TICK_HZ 1000000u
#define FREQ_ARR     65535u

/* 凑够这么多个 tick 的跨度才出一次结果。
 * 越大越准、越低频越差；4096 在 1 kHz 时约 4 个周期，够用。 */
#define FREQ_MIN_SPAN_TICKS 4096u

/* 边沿数上限。防止极高频时一直凑不满跨度而永远不出结果。 */
#define FREQ_MAX_EDGES 32u

typedef struct {
    uint32_t first_tick;
    uint32_t last_tick;
    uint32_t edges;        /* 已累计的上升沿数（含第一个） */
    bool     have_first;
} freq_meter_t;

typedef enum {
    FREQ_OK = 0,      /* 出结果了 */
    FREQ_PENDING,     /* 还没凑够，继续喂 */
    FREQ_TOO_FAST,    /* 两个边沿落在同一个 tick —— 超出量程 */
} freq_result_t;

void freq_meter_reset(freq_meter_t *m);

/* 喂一个上升沿的捕获值（单位 tick）。 */
freq_result_t freq_meter_feed(freq_meter_t *m, uint32_t tick, uint32_t *out_hz);

#endif /* APP_FREQ_METER_H */
