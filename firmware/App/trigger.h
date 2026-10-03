/* App/trigger.h —— 触发搜索（施密特迟滞状态机）
 *
 * # 为什么必须是状态机
 *
 * 不能写成「相邻两点跨越整条迟滞带」：1 kHz 正弦在 857 kSPS 下每样点只变化
 * 约 14 LSB，而迟滞带是 32 LSB 宽 —— 单步根本跨不过去，缓变信号**永远触发不了**。
 *
 * 正确语义（与真实示波器的施密特触发一致）：
 *   1. 信号必须先跌到 `lo` 以下置位（primed）
 *   2. 之后升到 `hi` 以上才算一次上升沿
 *   3. 落在带内的值既不置位也不触发 —— 这就是迟滞抑制抖动的原理
 *
 * 参考实现在 host/crates/sim/src/device.rs 的 `find_trigger()`，
 * 本文件与它**语义逐条对应**（那边的四个边界用例也被搬了过来）。
 *
 * # 为什么状态要跨调用保留
 *
 * 主循环是在**已发布的半区**上搜触发的（每 2048 个样点一次，见
 * firmware/README.md 的采样架构）。一次触发可能**跨块** ——
 * 前半块信号跌到 lo 以下置位、后半块才升过 hi。
 * 每块重置状态的话，这个触发会被整块丢掉。
 *
 * 所以对外只有「增量扫描」这一种用法，没有「扫一整块缓冲区」的一步到位版 ——
 * 后者是个陷阱：调用方很容易在每块开头都重建状态，而那样写**看起来是对的**。
 */

#ifndef APP_TRIGGER_H
#define APP_TRIGGER_H

#include <stdbool.h>
#include <stdint.h>

/* 触发比较的迟滞带宽（ADC LSB）。
 * 与 docs/04-performance.md、host/crates/sim 的 HYST 保持一致。 */
#define TRIG_HYSTERESIS_LSB 16u

/* 12-bit ADC 满量程。 */
#define TRIG_ADC_MAX_LSB 4095u

/* 触发搜索的状态。**放在调用方**（`acq` 的上下文里），不放在模块里 ——
 * 模块级静态变量会让「两块并行搜索」变成不可能，而回调式驱动下那是常态。
 */
typedef struct {
    /* 是否已经置位（信号到过迟滞带的另一侧）。 */
    bool primed;
    /* true = 找上升沿。 */
    bool rising;
} trig_state_t;

/* 开始一次新的搜索。
 *
 * `level` 与 `first_sample` **都是必需的**：初始置位状态由「第一个样点落在
 * 迟滞带的哪一侧」决定，所以既要知道电平（才知门限），也要知道首样点。
 * 少了任一个都只能猜，而猜错的表现是「明明有信号却不触发」—— 很难查。
 *
 * `first_sample` 要传**缓冲区的第一个样点**，不是 0。传 0 会让每次搜索
 * 都凭空置位（0 在 lo 以下）。
 */
void trig_reset(trig_state_t *st, bool rising, uint16_t level, uint16_t first_sample);

/* 在 `samples[0..n)` 里继续搜索。
 *
 * 找到就返回 `true` 并把下标写进 `*index`；否则返回 `false`，
 * **状态就地保留**，下次调用接着这次的位置继续。
 *
 * 前置条件：已经用当前的 `(rising, level)` 调过 [`trig_reset`]。
 * `n == 0` 直接返回 `false`，不碰状态。
 */
bool trig_scan(trig_state_t *st, const uint16_t *samples, uint32_t n, uint16_t level,
               uint32_t *index);

#endif /* APP_TRIGGER_H */
