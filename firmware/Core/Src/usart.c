/* CubeMX 风格的 USART1 初始化 —— 本工程加的（上游没有串口）。
 *
 * 链路：USART1 (PA9 = TX / PA10 = RX) ↔ **外接 CH340 模块** ↔ PC。
 * 见 docs/02-hardware.md §4 与 ADR-013。
 *
 * ⚠ 接线是**交叉**的：模块 TX → PA10、模块 RX → PA9。
 * ⚠ 不要同时给核心板 Type-C 与底板 Type-C 供电（双路供电倒灌）。
 *
 * 波特率 921600 8N1：UART 侧 92 KB/s，是 F103 全链路里最快的一档
 * （`docs/04-performance.md` §4）。**板载桥有没有、能不能稳跑这档，
 * 标着 `【待核实】`** —— 实测不稳就降到 460800。
 */

#include "usart.h"

#include "main.h"

UART_HandleTypeDef huart1;

void MX_USART1_UART_Init(void)
{
    huart1.Instance = USART1;
    /* 链路波特率。主机侧 `scope-cli serial --baud` 必须与它一致。
     *
     * 921600 是默认档；实测稳定后可切 2000000（USART1 挂 APB2=72 MHz，
     * USARTDIV = 2.25（= 2 + 4/16，4 位小数部分可精确表示）→ BRR = 0x24 = 36，
     * 零误差；CH340 系列按手册支持 2 Mbps）。
     * ⚠ 2 Mbps 在杜邦线上是否可靠只能实测 —— 切换后必须先过
     * docs/09 §4.2 的压测判据（连续 50 次 capture 无 CRC 错误）。 */
#define LINK_BAUD 921600u
    huart1.Init.BaudRate = LINK_BAUD;
    huart1.Init.WordLength = UART_WORDLENGTH_8B;
    huart1.Init.StopBits = UART_STOPBITS_1;
    huart1.Init.Parity = UART_PARITY_NONE;
    huart1.Init.Mode = UART_MODE_TX_RX;
    huart1.Init.HwFlowCtl = UART_HWCONTROL_NONE;
    huart1.Init.OverSampling = UART_OVERSAMPLING_16;
    if (HAL_UART_Init(&huart1) != HAL_OK)
    {
        Error_Handler();
    }
}

void HAL_UART_MspInit(UART_HandleTypeDef* uartHandle)
{
    GPIO_InitTypeDef GPIO_InitStruct = {0};

    if (uartHandle->Instance == USART1)
    {
        __HAL_RCC_USART1_CLK_ENABLE();
        __HAL_RCC_GPIOA_CLK_ENABLE();

        /* PA9 ------> USART1_TX */
        GPIO_InitStruct.Pin = GPIO_PIN_9;
        GPIO_InitStruct.Mode = GPIO_MODE_AF_PP;
        GPIO_InitStruct.Speed = GPIO_SPEED_FREQ_HIGH;
        HAL_GPIO_Init(GPIOA, &GPIO_InitStruct);

        /* PA10 ------> USART1_RX */
        GPIO_InitStruct.Pin = GPIO_PIN_10;
        GPIO_InitStruct.Mode = GPIO_MODE_INPUT;
        GPIO_InitStruct.Pull = GPIO_NOPULL;
        HAL_GPIO_Init(GPIOA, &GPIO_InitStruct);

        /* 优先级 5：已占的是 1(TIM3 捕获) / 2(EXTI15_10) / 3(TIM1 µs)
         * / 11~14(DMA/ADC/EXTI4)。串口是逐字节中断，比采集链低一档合适 ——
         * 丢一个字节由协议的 CRC + 重传兜底，而漏掉采集半区没法补。 */
        HAL_NVIC_SetPriority(USART1_IRQn, 5, 0);
        HAL_NVIC_EnableIRQ(USART1_IRQn);
    }
}

void HAL_UART_MspDeInit(UART_HandleTypeDef* uartHandle)
{
    if (uartHandle->Instance == USART1)
    {
        __HAL_RCC_USART1_CLK_DISABLE();

        HAL_GPIO_DeInit(GPIOA, GPIO_PIN_9 | GPIO_PIN_10);

        HAL_NVIC_DisableIRQ(USART1_IRQn);
    }
}
