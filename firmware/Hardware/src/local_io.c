/* Hardware/src/local_io.c —— 见 local_io.h 的说明。 */

#include "local_io.h"

#include "gpio.h"
#include "main.h"

/* 引脚归属（依据底板原理图 `SCH-简易数字示波器V1.2` 逐条核对）：
 *
 *   KEY1 = PB13、KEY2 = PB14、KEY3 = PB15     对地按钮，上拉
 *   编码器 A = PB4、B = PB3、按下 = PB9       带上拉的机械编码器（EC11）
 *   LED1 = PC14、LED2 = PC15                  **低电平点亮**
 *
 * ⚠ **编码器的 A/B 与方向的关系只由这两个宏决定。** 如果上板后发现转向
 * 与预期相反，把这两个宏对调即可 —— 不要去动 `App/local_input.c` 的译码表，
 * 那张表的方向语义本身是对的。 */
#define PIN_ENC_A   GPIO_PIN_4
#define PIN_ENC_B   GPIO_PIN_3

static local_input_t s_in;

/* ── 采样 ─────────────────────────────────────────────────────── */

/* 读一次 GPIOB 的输入数据寄存器，拼成 `App/local_input.h` 的位图。
 *
 * 一次读 IDR 再掩码，而不是六次 `HAL_GPIO_ReadPin` —— 后者每次都走一遍
 * 函数调用与断言。六根线必须**同一时刻**读，否则编码器两相之间可能夹进
 * 一次变化，译出一个不存在的方向。 */
static uint8_t sample_pins(void)
{
    uint32_t idr = GPIOB->IDR;
    uint8_t out = 0u;

    if ((idr & GPIO_PIN_13) != 0u) {
        out |= LOCAL_PIN_KEY1;
    }
    if ((idr & GPIO_PIN_14) != 0u) {
        out |= LOCAL_PIN_KEY2;
    }
    if ((idr & GPIO_PIN_15) != 0u) {
        out |= LOCAL_PIN_KEY3;
    }
    if ((idr & PIN_ENC_A) != 0u) {
        out |= LOCAL_PIN_ENC_A;
    }
    if ((idr & PIN_ENC_B) != 0u) {
        out |= LOCAL_PIN_ENC_B;
    }
    if ((idr & GPIO_PIN_9) != 0u) {
        out |= LOCAL_PIN_ENC_SW;
    }
    return out;
}

/* ── LED ──────────────────────────────────────────────────────── */

static uint16_t led_pin(local_led_t led)
{
    return (led == LOCAL_LED1) ? GPIO_PIN_14 : GPIO_PIN_15;
}

void LocalIo_LedSet(local_led_t led, bool on)
{
    /* 低电平点亮：`on` 对应 RESET。 */
    HAL_GPIO_WritePin(GPIOC, led_pin(led), on ? GPIO_PIN_RESET : GPIO_PIN_SET);
}

void LocalIo_LedToggle(local_led_t led)
{
    HAL_GPIO_TogglePin(GPIOC, led_pin(led));
}

/* ── 对外接口 ─────────────────────────────────────────────────── */

void LocalIo_Init(void)
{
    uint32_t now = HAL_GetTick();
    uint8_t pins;

    /* EXTI 是 CubeMX 配的，但我们轮询，用不到 —— 而且
     * `EXTI15_10_IRQn` 的优先级是 2，高于 µs 时基（3）与串口（5）。
     * 一个抖动按键没资格抢它们。这里**运行时**关掉，不去改 `gpio.c`
     * 的生成区（改了会被 CubeMX 下次重新生成覆盖）。 */
    HAL_NVIC_DisableIRQ(EXTI15_10_IRQn);
    HAL_NVIC_DisableIRQ(EXTI4_IRQn);

    pins = sample_pins();

    /* 自检：两颗 LED 各亮一下。看不见就说明这两条线有问题，
     * 而它们同时是「LED1 常亮 = 采集进行中」那套语义的载体。 */
    LocalIo_LedSet(LOCAL_LED1, true);
    LocalIo_LedSet(LOCAL_LED2, true);
    HAL_Delay(120);
    LocalIo_LedSet(LOCAL_LED1, false);
    LocalIo_LedSet(LOCAL_LED2, false);

    local_input_init(&s_in, now, pins);
}

void LocalIo_Poll(void)
{
    local_input_feed(&s_in, HAL_GetTick(), sample_pins());
}

bool LocalIo_PopEvent(local_event_t *out)
{
    return local_input_pop(&s_in, out);
}

uint32_t LocalIo_Dropped(void)
{
    return local_input_dropped(&s_in);
}
