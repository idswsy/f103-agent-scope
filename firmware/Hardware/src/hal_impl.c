/* Hardware/hal_impl.c —— 把 App 要的 9 个能力接到真实硬件上
 *
 * 契约在 `App/hal.h`。这里只做**装配**，实现全在 `adc_dma.c` 与 `link_uart.c`。
 */

#include "hal_impl.h"

#include "adc_dma.h"
#include "link_uart.h"
#include "main.h"
#include "protocol.h"

/* ── 唯一需要适配的一个 ────────────────────────────────────────
 * `App` 的契约是「再采 extra_samples 个样点之后才停 DMA」，余量是为了
 * 避免停的那一刻最后几个样点正被写。但 `App/acq.c` **恒传 0** ——
 * 它自己在停止前多等 `ACQ_STOP_MARGIN_SAMPLES`（512）个点。
 *
 * 所以这里直接停，并把「没实现 extra」写明白：默默忽略是最坏的做法，
 * 将来真有人用非零值时会以为余量生效了。
 * ─────────────────────────────────────────────────────────────── */
static void hal_acq_stop(uint32_t extra_samples)
{
    (void)extra_samples;   /* 见上：App 恒传 0，余量由它自己留 */
    AdcDma_Stop();
}

/* ── 装配 ────────────────────────────────────────────────────── */

static const hal_t HAL_IMPL = {
    .link_write             = LinkUart_Write,
    .link_read              = LinkUart_Read,
    .now_ms                 = HAL_GetTick,
    .quantize_rate_hz       = AdcDma_Quantize,
    .acq_start              = AdcDma_Start,
    .acq_stop               = hal_acq_stop,
    .ring_base              = AdcDma_Ring,
    .take_published_halves  = AdcDma_TakePublished,
    .tick_us                = AdcDma_TickUs,
};

const hal_t *HalImpl_Get(void)
{
    return &HAL_IMPL;
}

/* ── 编译期检查 ──────────────────────────────────────────────────
 * 采样率上限在两个地方各写了一遍：`App/hal.h`（App 的契约）与
 * `proto/protocol.h`（三端共用的量化函数用的）。**它们必须是同一个数** ——
 * 不一致的话，量化说「能达到」而 App 说「超上限」，行为取决于谁先判。
 * ─────────────────────────────────────────────────────────────── */
PROTO_STATIC_ASSERT(ACQ_RATE_MAX_HZ == PROTO_RATE_MAX_HZ,
                    "ACQ_RATE_MAX_HZ must equal PROTO_RATE_MAX_HZ");

void HalImpl_Init(void)
{
    AdcDma_Init();
    AdcDma_TickInit();   /* 1 MHz 的微秒时基，协议要 */
    LinkUart_Init();
}
