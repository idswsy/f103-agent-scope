/* Hardware/link_uart.c —— 串口链路的收发环实现
 *
 * 两个单生产者/单消费者环（SPSC）：
 *
 *   收：USART1 中断（生产者）  →  g_rx  →  主循环的 LinkUart_Read（消费者）
 *   发：主循环的 LinkUart_Write（生产者）→ g_tx → 发送中断（消费者）
 *
 * SPSC 环在 32 位 MCU 上不需要临界区：16 位下标的读/写是原子的
 * （不会读到半个值），加 `volatile` 就够。两边的下标各只有一方写，
 * 所以不存在竞争。
 */

#include "link_uart.h"

#include "main.h"
#include "usart.h"

/* 环满时最多等多久（毫秒）。
 * 一个满环 2048 B @921600 ≈ 22 ms，给 100 ms 是 4~5 倍余量。
 * 超了就丢 —— **主循环卡死比丢一帧严重得多**，而丢帧有 CRC + 重传兜底。 */
#define LINK_TX_BLOCK_MS 100u

/* ── 收包环 ──────────────────────────────────────────────────── */

static uint8_t  g_rx[LINK_RX_RING];
static volatile uint16_t g_rx_head;   /* 中断写 */
static volatile uint16_t g_rx_tail;   /* 主循环写 */
static uint8_t  g_rx_byte;            /* `HAL_UART_Receive_IT` 的单字节落点 */

/* ── 发包环 ──────────────────────────────────────────────────── */

static uint8_t  g_tx[LINK_TX_RING];
static volatile uint16_t g_tx_head;   /* 主循环写 */
static volatile uint16_t g_tx_tail;   /* 中断写 */
static volatile uint16_t g_tx_chunk;  /* 本次 `Transmit_IT` 发了多少字节 */
static volatile uint8_t  g_tx_busy;

/* 把一个环的下标往前推一格 */
static uint16_t ring_next(uint16_t i, uint16_t size)
{
    uint16_t n = (uint16_t)(i + 1u);
    return (n >= size) ? 0u : n;
}

/* 取走一个字节，没有就返回 -1 */
static int ring_pop(uint8_t *ring, uint16_t size,
                    volatile uint16_t *head, volatile uint16_t *tail)
{
    if (*head == *tail) {
        return -1;
    }
    uint8_t v = ring[*tail];
    *tail = ring_next(*tail, size);
    return (int)v;
}

/* 放一个字节；满了返回 0 */
static int ring_push(uint8_t *ring, uint16_t size,
                     volatile uint16_t *head, volatile uint16_t *tail, uint8_t v)
{
    uint16_t next = ring_next(*head, size);
    if (next == *tail) {
        return 0;
    }
    ring[*head] = v;
    *head = next;
    return 1;
}

/* ── 发包：把环里的连续一段交给中断 ────────────────────────── */

static void tx_kick(void)
{
    if (g_tx_busy != 0u || g_tx_head == g_tx_tail) {
        return;
    }

    /* 环是圆的，一次只能发到「回绕」为止；剩下的等这次发完再发。 */
    uint16_t n = (g_tx_head > g_tx_tail)
                     ? (uint16_t)(g_tx_head - g_tx_tail)
                     : (uint16_t)(LINK_TX_RING - g_tx_tail);

    g_tx_chunk = n;
    g_tx_busy  = 1u;
    if (HAL_UART_Transmit_IT(&huart1, (uint8_t *)&g_tx[g_tx_tail], n) != HAL_OK) {
        /* 启动失败就当没发 —— 不能让 `g_tx_busy` 卡在 1，否则发送永久停摆 */
        g_tx_busy  = 0u;
        g_tx_chunk = 0u;
    }
}

/* ── 对外接口 ────────────────────────────────────────────────── */

void LinkUart_Init(void)
{
    g_rx_head = 0u;
    g_rx_tail = 0u;
    g_tx_head = 0u;
    g_tx_tail = 0u;
    g_tx_chunk = 0u;
    g_tx_busy  = 0u;

    (void)HAL_UART_Receive_IT(&huart1, &g_rx_byte, 1u);
}

void LinkUart_Write(const uint8_t *data, uint32_t len)
{
    uint32_t t0 = HAL_GetTick();

    for (uint32_t i = 0; i < len; i++) {
        while (ring_push(g_tx, LINK_TX_RING, &g_tx_head, &g_tx_tail, data[i]) == 0) {
            tx_kick();
            if ((uint32_t)(HAL_GetTick() - t0) > LINK_TX_BLOCK_MS) {
                /* 超时：**丢掉剩下的这一整段**并返回。
                 * 半截帧会被对端的 CRC 拒掉并触发重传 —— 这是可恢复的；
                 * 而在这里无限等下去，整个采集状态机都停了。 */
                return;
            }
        }
    }
    tx_kick();
}

uint32_t LinkUart_Read(uint8_t *buf, uint32_t cap)
{
    uint32_t n = 0;
    int v;
    while (n < cap && (v = ring_pop(g_rx, LINK_RX_RING, &g_rx_head, &g_rx_tail)) >= 0) {
        buf[n++] = (uint8_t)v;
    }
    return n;
}

uint32_t LinkUart_TxPending(void)
{
    uint16_t h = g_tx_head;
    uint16_t t = g_tx_tail;
    return (uint32_t)((h >= t) ? (h - t) : (LINK_TX_RING - t + h));
}

/* ── HAL 回调 ────────────────────────────────────────────────── */

void HAL_UART_RxCpltCallback(UART_HandleTypeDef *huart)
{
    if (huart->Instance == USART1) {
        /* 环满就**丢这一个字节** —— 不能在这里阻塞（这是中断）。
         * 丢字节会让这一帧的 CRC 失败，协议的解析器会重新同步。 */
        (void)ring_push(g_rx, LINK_RX_RING, &g_rx_head, &g_rx_tail, g_rx_byte);
        (void)HAL_UART_Receive_IT(&huart1, &g_rx_byte, 1u);
    }
}

void HAL_UART_TxCpltCallback(UART_HandleTypeDef *huart)
{
    if (huart->Instance == USART1) {
        /* 推进尾指针，然后接着发下一段（可能是回绕后的那截） */
        for (uint16_t i = 0; i < g_tx_chunk; i++) {
            g_tx_tail = ring_next(g_tx_tail, LINK_TX_RING);
        }
        g_tx_chunk = 0u;
        g_tx_busy  = 0u;
        tx_kick();
    }
}

void HAL_UART_ErrorCallback(UART_HandleTypeDef *huart)
{
    if (huart->Instance == USART1) {
        /* 溢出/帧错误之后接收中断不会自己重挂 —— 必须重新点起来，
         * 否则串口从此哑掉，而且**没有任何迹象**（最坏的一种失败）。 */
        (void)HAL_UART_Receive_IT(&huart1, &g_rx_byte, 1u);
    }
}
