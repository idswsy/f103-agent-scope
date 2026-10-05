/* Hardware/hal_impl.h —— `App/hal.h` 那个 `hal_t` 的真机实现
 *
 * 这个文件是 ADR-008 那条纪律的**另一半**：`App/` 不许碰 HAL，
 * 于是「App 要的每一样硬件能力」都在这里被接到真实的 HAL 上。
 * 装配点只有一处（`hal_impl.c` 的 `HAL_IMPL`），找起来不会有歧义。
 */

#ifndef HAL_IMPL_H
#define HAL_IMPL_H

#include "hal.h"

/** @brief 取装配好的 `hal_t`。生命周期是静态的，随时可用。 */
const hal_t *HalImpl_Get(void);

/**
 * @brief 所有硬件的初始化：时钟外设已由 CubeMX 的 `MX_*_Init()` 配好，
 *        这里只做「让它们开始工作」的部分。
 *
 * 在 `MX_GPIO_Init` / `MX_DMA_Init` / `MX_ADC1_Init` / `MX_TIM4_Init` /
 * `MX_USART1_UART_Init` **之后**调用。
 */
void HalImpl_Init(void);

#endif /* HAL_IMPL_H */
