/* Hardware/inc/local_freq.h —— 用 TIM3 的输入捕获测频（比较器输出 → PA6）。
 *
 * 算法在 `App/freq_meter.c`（纯算术、PC 可测）；这里只负责
 * 「读捕获寄存器 → 喂进去 → 存下结果」，以及**中断优先级**。
 *
 * # 这个模块会占掉 TIM3 的每一个上升沿
 *
 * 一次上升沿 = 一次中断。100 kHz 的输入就是每秒十万次 —— 那不是能忽略的
 * 负载。`Core/Src/tim.c` 把 TIM3 的 NVIC 优先级配成了 **1**，高于 µs 时基
 * （3）和串口（5）：一个高频输入会把采集与串口一起压住，而**症状是采集
 * 丢半区、串口丢字节，看起来与测频毫无关系**。
 *
 * 所以 `LocalFreq_Init()` 在运行时把它降到 6。不去改 `tim.c` 的生成区
 * ——那里会被 CubeMX 下次重新生成覆盖掉。
 */

#ifndef HARDWARE_LOCAL_FREQ_H
#define HARDWARE_LOCAL_FREQ_H

#include <stdbool.h>
#include <stdint.h>

/* 结果超过这么久没有更新就算失效。
 *
 * 没有这一条的话，信号撤掉之后 `LocalFreq_GetHz` 会**永远返回最后一个读数**
 * —— 一台仪器报告一个已经不存在的信号，比报告"测不出"坏得多。
 * 1 s 对最低档（15.26 Hz，一次测量要 65 ms）也很宽裕。 */
#define LOCAL_FREQ_STALE_MS 1000u

/* 启动输入捕获、降中断优先级。在 `MX_TIM3_Init()` 之后调用一次。 */
void LocalFreq_Init(void);

/* 取最近一次测到的频率。
 *
 * 返回 `false` 表示**测不出** —— 信号太快（超出计时器分辨率）、太慢
 * （周期超过一次回绕）、还没凑够一次测量、或者结果已经过期。
 * **测不出时不要拿 0 去顶替** —— 那是"信号是直流"，是另一回事。 */
bool LocalFreq_GetHz(uint32_t *out_hz);

/* 累计的「超出量程」次数（诊断用）。 */
uint32_t LocalFreq_TooFastCount(void);

#endif /* HARDWARE_LOCAL_FREQ_H */
