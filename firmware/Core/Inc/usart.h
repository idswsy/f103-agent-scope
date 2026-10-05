/* CubeMX 风格的 USART1 初始化 —— 本工程加的（上游没有串口）。
 *
 * 手写而不是 CubeMX 生成：这个工程不再用 CubeMX 重新生成，
 * 而 CubeMX 在本机也跑不了。写法刻意对齐 `Core/Src` 下其它 `.c`。
 *
 * ⚠ 块注释内不得出现注释起始的那两个字符。写路径时把它们直接连写就会触发，
 *   而 ARMCC5 会对**每一个**包含本文件的 `.c` 各报一次
 *   `#9-D: nested comment is not allowed` —— 2026-10-05 的 Rebuild 报了 4 条。
 *   CI 的「严格 C99」一步用 `-Werror=comment` 挡这一类。
 */

#ifndef __USART_H
#define __USART_H

#ifdef __cplusplus
extern "C" {
#endif

#include "main.h"

/* ⚠ 用 USART1（PA9/PA10）。
 *   底板上的 PA2/PA3 归 PWM 与模拟输入，所以 **USART2 绝对禁用**。 */
extern UART_HandleTypeDef huart1;

void MX_USART1_UART_Init(void);

#ifdef __cplusplus
}
#endif

#endif /* __USART_H */
