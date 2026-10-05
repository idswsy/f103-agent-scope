/* Hardware/inc/local_gen.h —— 本机函数发生器（TIM2_CH3 → PA2）。
 *
 * 底板把 PA2 引到一个独立的排针，所以它既能当信号源喂自己的模拟前端，
 * 也能当一台小信号源给别的东西用。
 *
 * # 与上游的差别
 *
 * 上游的按键只给三档（1k / 2k / 4kHz），因为它是在 `timerPeriod` 上做
 * 加减（1000 → 500 → 250 tick）。这里改成按**频率**设定并做量化 ——
 * 协议命令要的是「把输出设成 3.3 kHz」，不是「退一档」。
 *
 * # 量程不是调参，是硬件决定的
 *
 * 计时器 1 MHz（PSC=71），16 位 ARR：
 *   上限 500 kHz  —— ARR=2，再快占空比只剩 0/50/100% 三档，没有意义
 *   下限 15.3 Hz  —— ARR=65535
 * 超出量程**返回 0**，不悄悄吸附到边界值。
 *
 * # 上电默认是关的
 *
 * `HAL_TIM_PWM_ConfigChannel` 会把 `CC3E` 清掉，所以 `MX_TIM2_Init()` 跑完之后
 * PA2 并不输出 —— 正合上游 `ouptputbit = 0` 的语义。
 */

#ifndef HARDWARE_LOCAL_GEN_H
#define HARDWARE_LOCAL_GEN_H

#include <stdbool.h>
#include <stdint.h>

/* 可设的频率范围。 */
#define LOCAL_GEN_MIN_HZ 16u
#define LOCAL_GEN_MAX_HZ 500000u

/* 占空比的千分比范围。留出两端是因为：CCR=0 输出恒低、CCR=ARR 恒高，
 * 两端都不是"方波"。 */
#define LOCAL_GEN_DUTY_MIN_PERMILLE 10u
#define LOCAL_GEN_DUTY_MAX_PERMILLE 990u

void LocalGen_Init(void);

/* 开 / 关输出。关的时候 PA2 的实际电平**需实测**（通道关掉之后引脚仍配着
 * 复用推挽，具体是低还是高阻取决于定时器与 GPIO 的交互）。 */
void LocalGen_SetEnabled(bool on);
bool LocalGen_IsEnabled(void);

/* 设频率。返回**实际生效**的值（会被 1 MHz 计时器量化），达不到则回 0 且不改动。 */
uint32_t LocalGen_SetHz(uint32_t hz);
uint32_t LocalGen_GetHz(void);

/* 设占空比（千分比）。超范围会被夹到 [MIN, MAX]。 */
void LocalGen_SetDutyPermille(uint16_t permille);
uint16_t LocalGen_GetDutyPermille(void);

#endif /* HARDWARE_LOCAL_GEN_H */
