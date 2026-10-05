/* Hardware/inc/local_io.h —— 板上的按键、编码器与 LED。
 *
 * # 为什么是轮询，不是中断
 *
 * 按键与编码器的消抖逻辑在 `App/local_input.c`（纯逻辑、PC 可测），
 * 这一层只负责**采一次电平**再交过去。
 *
 * 那为什么不用 EXTI 中断？主循环空闲时一轮只要几十到几百微秒，而人手
 * 转编码器再快也是每个边沿几毫秒 —— 轮询裕量有三个数量级。用中断换来的
 * 是「主循环停顿 22 ms（`LinkUart_Write` 的上限）时不漏边沿」这一条，
 * 但代价是每个边沿一次 ISR，而正交译码本来还要读另一相，省不下多少。
 *
 * ⚠ **所以 `Core/Src/gpio.c` 里给 PB13/14/15 与 PB4 配的 EXTI 中断，
 * 在本模块初始化时会被显式关掉**（`HAL_NVIC_DisableIRQ`）。那两组中断的
 * 优先级分别是 2 和 11 —— 2 高于 µs 时基（3）和串口（5），
 * 让一个抖动的按键去抢这两样没有任何道理。
 *
 * 将来若发现快速连转会丢格，再改成「A 相边沿中断 + 在 ISR 里读 B 相」。
 */

#ifndef HARDWARE_LOCAL_IO_H
#define HARDWARE_LOCAL_IO_H

#include <stdbool.h>
#include <stdint.h>

#include "local_input.h"

/* 板上的两颗 LED（`PC14` / `PC15`，**低电平点亮**）。 */
typedef enum {
    LOCAL_LED1 = 0,
    LOCAL_LED2 = 1,
} local_led_t;

/* 初始化：关掉不用的 EXTI、按当前电平给消抖器定初值、点亮两颗 LED 各一次
 * 作为自检（能看见就说明这两条线是通的）。 */
void LocalIo_Init(void);

/* 主循环每轮调用一次：采一次引脚、喂给消抖器。**不阻塞。** */
void LocalIo_Poll(void);

/* 取一个本地事件。返回 `false` 表示没有。 */
bool LocalIo_PopEvent(local_event_t *out);

/* 因事件队列满而丢弃的事件数（透传 `App/local_input.c` 的计数）。 */
uint32_t LocalIo_Dropped(void);

void LocalIo_LedSet(local_led_t led, bool on);
void LocalIo_LedToggle(local_led_t led);

#endif /* HARDWARE_LOCAL_IO_H */
