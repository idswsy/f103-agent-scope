/* App/waveform.h —— 把一帧采集压成屏幕能直接画的东西。
 *
 * # 为什么单独一个模块
 *
 * 「窗口 → 每列的上下沿 / Vpp / 频率」全是**纯算术**，没有一行碰硬件。
 * 把它放在 App 层，就能在 PC 上把边界条件测干净（窗口在环里回绕、
 * 窗口比屏幕列数还短、全零窗口、直流、窄尖峰……）—— 这些正是会出错的地方，
 * 而它们在真机上很难复现。画屏幕那部分在 `Hardware/src/display.c`。
 *
 * # 它不做的事
 *
 * 不碰 `take_published_halves`（那个的所有权归 `acq_poll`），不读未发布的半区，
 * 不调 `hal->ring_base()` —— 读采集窗一律走 `acq_sample()`，那是 `App/acq.c`
 * 对「什么时候能读」的唯一判定点。
 */

#ifndef APP_WAVEFORM_H
#define APP_WAVEFORM_H

#include <stdbool.h>
#include <stdint.h>

#include "acq.h"

/* 屏幕波形区的列数。与 `Hardware/src/display.c` 的布局常量一致。 */
#define WAVE_COLUMNS 100u

/* 每列的纵坐标行数（0 = 波形区底行，`WAVE_ROWS - 1` = 顶行）。 */
#define WAVE_ROWS 51u

/* 峰峰值低于这个值就当直流处理，不测频。 */
#define WAVE_MIN_SPAN_LSB 32u

/* 一帧画面上要用的全部数据。 */
typedef struct {
    /* 每列的上下沿（0 起，`top >= bot`）。
     *
     * 取该列样点的 min/max 双端点，**不是抽一个样点**：100 列压 4096 点时
     * 每列平均 41 个样点，单点采样会让只占几个样点的窄尖峰整帧消失 ——
     * 而「抓毛刺」正是示波器最不该丢的东西。 */
    uint8_t top[WAVE_COLUMNS];
    uint8_t bot[WAVE_COLUMNS];

    /* 窗口内的峰峰值，ADC LSB 整数（不是伏特 —— 换算在显示层，见 ADR 纪律）。 */
    uint16_t vpp_lsb;

    /* 窗口内的信号频率（Hz）。0 = 测不出（直流，或不足两个周期）。 */
    uint32_t freq_hz;

    /* 这份帧是否可用。为假时上面三个字段无意义，显示层应保持上一帧或画空。 */
    bool valid;
} wave_frame_t;

/* 把采集窗压成一帧。
 *
 * `a` 必须处于 `capture_ready` 为真的状态（DONE 之后）。
 * 条件不满足时 `out->valid` 置假 —— **不假装有一帧**。 */
void wave_build(const acq_t *a, wave_frame_t *out);

/* `wave_build` 的实现本体，直接吃裸样点。
 *
 * 单独导出的理由：PC 上的测试要能造窗口（尤其是**跨环回绕**的窗口），
 * 而不必去搭一个假的 `hal_t`。生产代码走 `wave_build`。
 *
 * `ring_len` 是环长，`window_start` 是窗口起点在环里的绝对下标。 */
void wave_build_raw(const uint16_t *ring, uint32_t ring_len,
                    uint32_t window_start, uint32_t window_len,
                    uint32_t rate_hz, wave_frame_t *out);

#endif /* APP_WAVEFORM_H */
