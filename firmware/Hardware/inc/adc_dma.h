/* Hardware/adc_dma.h —— 采集链：TIM4_CC4 → ADC1 → DMA1Ch1 → 8 KB 环
 *
 * 本工程自己的代码（不是上游的）。
 *
 * 与 `App/hal.h` 的关系：本模块提供那几个采集相关的函数指针的实现，
 * 具体的 `hal_t` 装配在 `hal_impl.c`。环的大小等常量**不在这里重复定义** ——
 * 从 `App/hal.h` 取，那里是单一真相源。
 */

#ifndef ADC_DMA_H
#define ADC_DMA_H

#include <stdint.h>

/**
 * @brief 采集链初始化（不启动）
 *
 * 在 `MX_ADC1_Init()` 与 `MX_TIM4_Init()` 之后调用一次。
 * 它只清内部状态 —— ADC/DMA/TIM4 的寄存器配置由 CubeMX 那两个函数负责。
 */
void AdcDma_Init(void);

/**
 * @brief 把请求的采样率吸附到定时器能达到的档位
 *
 * 直接转 `proto_quantize_rate_hz()` —— 那条规则三端共用，见 `proto/protocol.h`。
 *
 * @return 实际采样率（Hz）；**0 表示达不到**（调用方应按参数错误处理，
 *         不要静默换成别的档位）
 */
uint32_t AdcDma_Quantize(uint32_t requested_hz);

/**
 * @brief 以 `rate_hz` 开始采样
 *
 * **必须传已经量化过的值**（`AdcDma_Quantize()` 的输出）。
 * 重设 TIM4 的 ARR/CCR4、复位计数器与已发布位图，然后启动 ADC+DMA。
 *
 * ⚠ 起点是对齐的：DMA 从头写 `ring[0]`，所以 App 的绝对样点号从 0 起算。
 */
void AdcDma_Start(uint32_t rate_hz);

/** @brief 停 DMA 与 TIM4。可重复调用。 */
void AdcDma_Stop(void);

/** @brief 采集环基址，`ACQ_RING_SAMPLES` 个 u16（DMA 循环写）。 */
const uint16_t *AdcDma_Ring(void);

/**
 * @brief **领取**自上次调用以来被中断发布过的半区，返回位图并清零
 *
 * 这是所有权交接点。读位图与清零在**关中断**的临界区里完成 ——
 * 否则「读到 bit → 中断又置 bit → 清零」会丢掉新到的那一次发布。
 *
 * @return `ACQ_HALF_FIRST` / `ACQ_HALF_SECOND` 的按位或
 */
uint8_t AdcDma_TakePublished(void);

/**
 * @brief 启动 1 MHz 的微秒时基（TIM1 + 软件高位）
 *
 * 协议要求 `tick_us` 是 32 位微秒计数（约 71.6 分钟回绕）。TIM1 被借来
 * 做这件事：PSC=71 → 1 MHz，溢出中断累加高 16 位。**不启用它的任何输出脚**，
 * 所以与 USART1 的 PA9/PA10 不冲突。
 */
void AdcDma_TickInit(void);

/** @brief 自开机以来的微秒数（32 位，约 71.6 分钟回绕）。 */
uint32_t AdcDma_TickUs(void);

/**
 * @brief TIM1 更新中断的 ISR 体。由 `stm32f1xx_it.c` 的
 *        `TIM1_UP_IRQHandler` 调用（CubeMX 规定中断入口都放那边）。
 */
void AdcDma_TickIsr(void);

#endif /* ADC_DMA_H */
