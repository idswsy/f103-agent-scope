/* Hardware/link_uart.h —— 串口链路的收发环
 *
 * 本工程自己的代码。契约见 `App/hal.h` 的 `link_write` / `link_read`：
 *
 *   - `link_write` **可以阻塞**，但只在**环满**时才真的等
 *   - `link_read`  **绝不阻塞** —— 主循环还要搜触发，卡在串口上会把
 *                  块中断的处理拖垮
 */

#ifndef LINK_UART_H
#define LINK_UART_H

#include <stdint.h>

/* 收包环。设备侧一帧最大 `PROTO_MAX_FRAME_RX` = 524 B，取 1 KB 有富余。 */
#define LINK_RX_RING 1024u

/* 发包环。一帧最大 `PROTO_MAX_FRAME_TX` = 2072 B，取 2 KB 正好装一帧。 */
#define LINK_TX_RING 2048u

/** @brief 启动链路：清环、点起第一次接收中断。在 `MX_USART1_UART_Init()` 之后调用。 */
void LinkUart_Init(void);

/**
 * @brief 把一段字节交给链路发送
 *
 * 拷进发包环后立刻返回；**只有在环满时才等**（等发送中断腾地方）。
 * 等待有上限（见 `.c` 里的 `LINK_TX_BLOCK_MS`）——超了**丢掉这一段并返回**，
 * 而不是无限等：主循环卡死比丢一帧严重得多，而丢帧有协议的 CRC + 重传兜底。
 *
 * ⚠ 这里是**拷贝**语义：调用方返回后可以立刻复用那块缓冲。
 */
void LinkUart_Write(const uint8_t *data, uint32_t len);

/**
 * @brief 取走已收到的字节
 * @return 实际取到的数量；0 表示暂时没有。**绝不阻塞。**
 */
uint32_t LinkUart_Read(uint8_t *buf, uint32_t cap);

/** @brief 发包环里还有多少字节没发完（诊断用）。 */
uint32_t LinkUart_TxPending(void);

#endif /* LINK_UART_H */
