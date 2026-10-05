/* Hardware/src/local_freq.c —— 见 local_freq.h 的说明。 */

#include "local_freq.h"

#include "freq_meter.h"
#include "main.h"
#include "tim.h"

/* 频率与「是否有效」被打包进**一个** 32 位字。
 *
 * 分开存两个变量的话，主循环可能读到「新的频率 + 旧的 valid」这样的错配 ——
 * 而 32 位对齐读在 Cortex-M3 上是单条指令、天然原子。频率上限远小于 2^31，
 * 借用最高位当有效标志是安全的。 */
#define FREQ_WORD_VALID 0x80000000u

static freq_meter_t      s_meter;    /* 只在中断里改 */
static volatile uint32_t s_word;     /* 中断写、主循环读 */
static volatile uint32_t s_last_ms;  /* 上一次出结果的时刻 */
static volatile uint32_t s_too_fast;

void LocalFreq_Init(void)
{
    /* 见 local_freq.h 的文件头：`tim.c` 把 TIM3 配成优先级 1，
     * 一个高频输入能把采集与串口一起压住。运行时降下来。 */
    HAL_NVIC_SetPriority(TIM3_IRQn, 6u, 0u);

    freq_meter_reset(&s_meter);
    s_word = 0u;
    s_last_ms = 0u;
    s_too_fast = 0u;

    (void)HAL_TIM_IC_Start_IT(&htim3, TIM_CHANNEL_1);
}

/* ⚠ **本文件是全固件里 `HAL_TIM_IC_CaptureCallback` 的唯一 owner。**
 * 以后再有模块要响应输入捕获，只能由这里转发，不许自己再定义一个 ——
 * 两个定义会在链接期报重名（CI 有一条检查盯着，见 ci.yml 的
 * 「HAL 回调归属」）。 */
void HAL_TIM_IC_CaptureCallback(TIM_HandleTypeDef *htim)
{
    uint32_t tick;
    uint32_t hz = 0u;

    if (htim->Instance != TIM3) {
        return;
    }

    tick = HAL_TIM_ReadCapturedValue(htim, TIM_CHANNEL_1);

    switch (freq_meter_feed(&s_meter, tick, &hz)) {
    case FREQ_OK:
        s_word = hz | FREQ_WORD_VALID;
        s_last_ms = HAL_GetTick();
        break;

    case FREQ_TOO_FAST:
        /* 分不出来就说分不出来 —— 留着上一个读数会让人以为信号还在。 */
        s_word = 0u;
        s_too_fast++;
        break;

    case FREQ_PENDING:
    default:
        break;
    }
}

bool LocalFreq_GetHz(uint32_t *out_hz)
{
    uint32_t w = s_word;

    if ((w & FREQ_WORD_VALID) == 0u) {
        return false;
    }

    /* 无符号减法天然跨过 `HAL_GetTick()` 的 49 天回绕。 */
    if ((uint32_t)(HAL_GetTick() - s_last_ms) > LOCAL_FREQ_STALE_MS) {
        return false;
    }

    if (out_hz != 0) {
        *out_hz = w & ~FREQ_WORD_VALID;
    }
    return true;
}

uint32_t LocalFreq_TooFastCount(void)
{
    return s_too_fast;
}
