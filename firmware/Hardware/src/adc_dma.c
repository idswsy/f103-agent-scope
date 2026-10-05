/* Hardware/adc_dma.c —— 采集链的实现
 *
 *   TIM4 (PSC=0, ARR=83, CCR4=83) ──CC4──► ADC1 单次转换
 *                                              │
 *                                              ▼
 *                                  DMA1 Channel 1（循环模式）
 *                                              │
 *                                              ▼
 *                        uint16_t g_ring[4096]  (8 KB)
 *                                              │
 *                        半传输 / 全传输中断 → 置「已发布」位图的对应位
 *
 * 设计说明见 adc_dma.h 与 firmware/README.md 的「采样架构」。
 *
 * ⚠ 半区**发布顺序是硬要求**：`ACQ_HALF_FIRST` 必须对应 `ring[0..2048)`、
 *   `ACQ_HALF_SECOND` 对应 `ring[2048..4096)` —— `App/acq.c` 依赖
 *   「先前半再后半」的顺序推进绝对样点号，反过来算出来的号会跳。
 */

#include "adc_dma.h"

#include "adc.h"
#include "hal.h"     /* ACQ_RING_SAMPLES / ACQ_HALF_* / ACQ_RATE_MAX_HZ —— 单一真相源 */
#include "main.h"
#include "tim.h"
#include "protocol.h"   /* proto_quantize_rate_hz —— 采样率量化三端共用 */

/* 采集环。DMA1_Channel1 循环写它，地址必须稳定。 */
static uint16_t g_ring[ACQ_RING_SAMPLES];

/* 已发布的半区位图。中断里置位、主循环里关中断读+清。 */
static volatile uint8_t g_published;

/* ── 临界区 ──────────────────────────────────────────────────────
 * 存/恢复 PRIMASK 而不是无脑 __enable_irq()：调用方本来关着中断时，
 * 我们不能替它打开。
 * ─────────────────────────────────────────────────────────────── */
static uint32_t critical_enter(void)
{
    uint32_t primask = __get_PRIMASK();
    __disable_irq();
    return primask;
}

static void critical_exit(uint32_t primask)
{
    __set_PRIMASK(primask);
}

/* ── 对外接口 ────────────────────────────────────────────────── */

void AdcDma_Init(void)
{
    g_published = 0u;
    /* 环不清零：ARM 之后 DMA 会从头写满，清它只是白费时间。 */
}

uint32_t AdcDma_Quantize(uint32_t requested_hz)
{
    return proto_quantize_rate_hz(requested_hz);
}

/* ── 起停顺序：触发源必须先于 ADC ────────────────────────────────
 *
 * ⚠ **停 ADC 之前必须先停 TIM4。** 反过来的话：`EXTTRIG` 还开着、TIM4
 * 还在以采样率往外发 CC4，ADC 被持续触发，`ADON` 就清不干净 ——
 * `HAL_ADC_Stop_DMA` 内部的 `ADC_ConversionStop_Disable` 等满 2 ms 超时
 * 后返回 HAL_ERROR，**并且跳过 DMA abort**。于是 `hdma_adc1.State` 永久
 * 停在 BUSY，而 `HAL_ADC_Start_DMA` 又把 `HAL_DMA_Start_IT` 的返回值丢掉了
 * （`stm32f1xx_hal_adc.c` 里那行没有判返回值），**之后每一次 ARM 都静默地
 * 不起 DMA**：
 *
 *   - ARM 照常回 Ack，状态是 ARMED，看不出任何异常
 *   - `CNDTR` 停在 0（`DMA_SetConfig` 从没跑到）
 *   - 一个半区都不发布 → `total_samples` 恒为 0 → **永远不触发**
 *   - 报出去的事件里 `n_samples = 0`，`read_buffer` 永远回 NO_DATA
 *
 * 2026-10-05 真机实测：**第一次 ARM 正常**（环里进了一个样点、CNDTR 4095），
 * 之后全部哑掉 —— 正是「第一次的 Stop 把通道锁死，此后一次性失效」。
 * ─────────────────────────────────────────────────────────────── */

/* 起 ADC + DMA，并**核对 DMA 真的起来了**。
 * 补这一道是因为 `HAL_ADC_Start_DMA` 丢弃了 `HAL_DMA_Start_IT` 的返回值，
 * 而后者在 `State != READY` 时返回 HAL_BUSY、**一个寄存器都不写**。 */
static void adc_dma_start_checked(void)
{
    if (HAL_ADC_Start_DMA(&hadc1, (uint32_t *)g_ring, ACQ_RING_SAMPLES) != HAL_OK) {
        Error_Handler();
    }
    if (hadc1.DMA_Handle->State == HAL_DMA_STATE_BUSY) {
        return;   /* 起来了 */
    }

    /* 没起来：多半是上一次 `HAL_ADC_Stop_DMA` 半路失败，把通道留在了 BUSY。
     * 强制放回 READY 再启一遍 —— 这是唯一能解开那个状态的办法。 */
    (void)HAL_DMA_Abort(hadc1.DMA_Handle);
    hadc1.DMA_Handle->State = HAL_DMA_STATE_READY;
    if (HAL_ADC_Start_DMA(&hadc1, (uint32_t *)g_ring, ACQ_RING_SAMPLES) != HAL_OK) {
        Error_Handler();
    }
    if (hadc1.DMA_Handle->State != HAL_DMA_STATE_BUSY) {
        /* 第二次还是起不来。**明确死给你看，不要静默** ——
         * 「ARM 回了 Ack 但什么都不会发生」是最难查的一种失败。 */
        Error_Handler();
    }
}

void AdcDma_Start(uint32_t rate_hz)
{
    /* rate_hz 已量化过（AdcDma_Quantize 的输出），所以这里整除。 */
    uint32_t period = 72000000u / rate_hz;
    if (period < 1u)      { period = 1u; }
    if (period > 65536u)  { period = 65536u; }

    /* 先停干净：上一轮的标志位与在途 DMA 会污染这一轮的起点。
     * ⚠ **顺序不能反** —— 见本函数上方那段。 */
    __HAL_TIM_DISABLE(&htim4);
    (void)HAL_ADC_Stop_DMA(&hadc1);

    /* 采样周期 = ARR + 1 个 72 MHz tick */
    __HAL_TIM_SET_AUTORELOAD(&htim4, period - 1u);
    /* CCR4 = ARR → 每周期产生一次比较事件。
     * **输出没使能**（CC4E=0），所以不会驱动 PB9（底板编码器按下键）。 */
    __HAL_TIM_SET_COMPARE(&htim4, TIM_CHANNEL_4, period - 1u);
    __HAL_TIM_SET_COUNTER(&htim4, 0u);
    __HAL_TIM_CLEAR_FLAG(&htim4, TIM_FLAG_UPDATE | TIM_FLAG_CC4);

    /* 位图清零与 TakePublished 的临界区配对 —— 否则清的那一瞬间
     * 若有发布进来会被吞掉。 */
    {
        uint32_t p = critical_enter();
        g_published = 0u;
        critical_exit(p);
    }

    /* 先开定时器再启 ADC：反过来的话，第一个 CC4 事件可能落在 ADC 还没
     * 就绪的时候，第一帧就少一个样点。 */
    __HAL_TIM_ENABLE(&htim4);

    /* Length 是**传输次数**（半字），不是字节 —— 4096 个 u16。
     * 起完还要核对 DMA 真起来了，见 `adc_dma_start_checked`。 */
    adc_dma_start_checked();
}

void AdcDma_Stop(void)
{
    /* ⚠ **触发源先于 ADC** —— 顺序反了会把 DMA 通道锁死在 BUSY，
     * 而此后每一次 ARM 都会静默失效。见本文件上方那段。 */
    __HAL_TIM_DISABLE(&htim4);
    (void)HAL_ADC_Stop_DMA(&hadc1);
}

const uint16_t *AdcDma_Ring(void)
{
    return g_ring;
}

uint8_t AdcDma_TakePublished(void)
{
    uint32_t p = critical_enter();
    uint8_t v = g_published;
    g_published = 0u;
    critical_exit(p);
    return v;
}

/* ── HAL 回调 ──────────────────────────────────────────────────
 * `HAL_ADC_Start_DMA` 把 DMA 的半传输/全传输回调接到这两个函数上
 * （见 stm32f1xx_hal_adc.c 的 `ADC_DMAHalfConvCplt` / `ADC_DMAConvCplt`）。
 * ─────────────────────────────────────────────────────────────── */

void HAL_ADC_ConvHalfCpltCallback(ADC_HandleTypeDef *hadc)
{
    if (hadc->Instance == ADC1) {
        g_published |= ACQ_HALF_FIRST;      /* ring[0 .. 2048) 写完 */
    }
}

void HAL_ADC_ConvCpltCallback(ADC_HandleTypeDef *hadc)
{
    if (hadc->Instance == ADC1) {
        g_published |= ACQ_HALF_SECOND;     /* ring[2048 .. 4096) 写完 */
    }
}

/* ── 微秒时基（TIM1）───────────────────────────────────────────
 * 协议要求 `tick_us` 是 32 位微秒计数。TIM4 拿去做 ADC 触发了，
 * TIM3 归 PA6 的输入捕获，TIM2 是函数发生器 —— 只剩 TIM1。
 * 只借它的计数器，**不启用任何输出脚**，所以与 USART1 的 PA9/PA10 不冲突。
 * ─────────────────────────────────────────────────────────────── */

static volatile uint16_t g_us_ovf;

void AdcDma_TickInit(void)
{
    __HAL_RCC_TIM1_CLK_ENABLE();

    TIM1->PSC = 71u;                 /* 72 MHz / (71+1) = 1 MHz */
    TIM1->ARR = 0xFFFFu;
    TIM1->EGR = TIM_EGR_UG;          /* 立刻把 PSC 装进影子寄存器 */
    TIM1->SR  = 0u;                  /* UG 会顺带置更新标志，清掉 */
    TIM1->DIER = TIM_DIER_UIE;

    HAL_NVIC_SetPriority(TIM1_UP_IRQn, 3u, 0u);
    HAL_NVIC_EnableIRQ(TIM1_UP_IRQn);

    g_us_ovf = 0u;
    TIM1->CR1 |= TIM_CR1_CEN;
}

uint32_t AdcDma_TickUs(void)
{
    /* 读两遍：若高位的递增发生在两次读之间，重来。
     * 溢出中断才 15.26 Hz，重试几乎不会发生。 */
    uint16_t ovf;
    uint16_t cnt;
    do {
        ovf = g_us_ovf;
        cnt = (uint16_t)TIM1->CNT;
    } while (ovf != g_us_ovf);

    return ((uint32_t)ovf << 16) | (uint32_t)cnt;
}

void AdcDma_TickIsr(void)
{
    if ((TIM1->SR & TIM_SR_UIF) != 0u) {
        TIM1->SR = (uint16_t)(TIM1->SR & ~TIM_SR_UIF);   /* 写 0 清标志 */
        g_us_ovf++;
    }
}
