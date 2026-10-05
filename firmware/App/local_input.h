/* App/local_input.h —— 按键与编码器的**纯逻辑**：消抖 + 正交译码 + 事件队列。
 *
 * # 为什么这一层要单独拎出来
 *
 * 上游那套（`Hardware/src/scope_ui.c` 的 `Key_Sacnf` / `KEYD_SCAN`）是在
 * 按键处理里直接 `HAL_Delay(20)` 等待抖动过去，编码器按下更是 `do{}while(1)`
 * 忙等松开 —— 最坏一次按键会**阻塞主循环 60 ms**。
 *
 * 那在这台仪器上是不能接受的：采集期间半个 8 KB 环每 **2.39 ms** 发布一次
 * （见 `Hardware/src/adc_dma.c`），主循环单轮必须远小于它，否则 `acq_poll`
 * 会漏掉已发布的半区、样点被 DMA 覆盖而**没有任何标志位能看出来**。
 *
 * 所以这里把它翻过来：**输入是「某个时刻的引脚位图」，输出是「事件」**，
 * 中间没有任何等待。时间是调用方给的参数，不是它去读的时钟 ——
 * 这样整个模块在 PC 上就能把「抖动 / 长按 / 正交序列 / 漏拍 / 回退 /
 * 时间戳回绕」这些边界条件测干净。
 *
 * # 它不做的事
 *
 * 不读 GPIO、不点亮 LED、不决定「按了键该干什么」——那分别是
 * `Hardware/src/local_io.c` 与策略层的事。
 */

#ifndef APP_LOCAL_INPUT_H
#define APP_LOCAL_INPUT_H

#include <stdbool.h>
#include <stdint.h>

/* 引脚位图。**低有效** —— 按键对地、编码器上拉输入，按下读到 0。 */
#define LOCAL_PIN_KEY1  0x01u
#define LOCAL_PIN_KEY2  0x02u
#define LOCAL_PIN_KEY3  0x04u
#define LOCAL_PIN_ENC_A 0x08u
#define LOCAL_PIN_ENC_B 0x10u
#define LOCAL_PIN_ENC_SW 0x20u

/* 消抖窗口。电平抖动持续短于它就不算数。
 *
 * 20 ms 取自上游的既有值；真机上如果发现漏按或连发，先调这个数
 * （`docs/02-hardware.md` §9 把它列进了待实测项）。 */
#define LOCAL_DEBOUNCE_MS 20u

/* 编码器转出一"格"需要几个正交边沿。
 *
 * EC11 一类带定位的编码器通常一个定位点对应 4 个正交边沿（A/B 各一个完整
 * 周期）。**这个数要随编码器型号实测**：数小了会一格出两次事件，
 * 数大了会丢格。 */
#define LOCAL_ENC_EDGES_PER_DETENT 4

/* 事件队列深度。按键事件是稀疏的，4 深足够吸收一次连按；
 * 溢出时**丢最旧的并计数**，不阻塞、不覆盖最新的。 */
#define LOCAL_EVENT_QUEUE 4u

typedef enum {
    LOCAL_EV_NONE = 0,
    LOCAL_EV_KEY1,
    LOCAL_EV_KEY2,
    LOCAL_EV_KEY3,
    LOCAL_EV_ENC_CW,
    LOCAL_EV_ENC_CCW,
    LOCAL_EV_ENC_PUSH,
} local_event_t;

typedef struct {
    /* 三键 + 编码器按下，各占一位，位序为 bit0=KEY1 / bit1=KEY2 /
     * bit2=KEY3 / bit3=ENC_SW。值 **1 表示按下**（引脚是低有效的，
     * 这里已经翻过来了 —— 免得每一处用的人都得再想一遍）。 */
    uint8_t  raw;           /* 这一刻看到的原始电平 */
    uint8_t  stable;        /* 消抖之后确认的电平 */
    uint32_t raw_ms[4];     /* 原始电平最后一次变化的时刻 */

    /* 编码器正交译码 */
    uint8_t  enc_last;      /* 上一次的 (A<<1)|B，0..3 */
    int8_t   enc_accum;     /* 未满一格的边沿数，正负表示方向 */

    /* 事件环 */
    uint8_t  q_head;
    uint8_t  q_count;
    local_event_t q[LOCAL_EVENT_QUEUE];

    /* 因为队列满而丢掉的事件数。**要能看见** —— 静默丢弃会让
     * 「按了没反应」变成一个查不出来的现象。 */
    uint32_t dropped;
} local_input_t;

/* 初始化。`now_ms` 用于给首次消抖定时；`pins` 是**当前**引脚位图，
 * 上电时按下的键不应被当成一次新按下。 */
void local_input_init(local_input_t *s, uint32_t now_ms, uint8_t pins);

/* 主循环每轮调用一次。`pins` 是这一刻的引脚位图（低有效）。
 *
 * **不阻塞、不读时钟**：时间由 `now_ms` 传入。 */
void local_input_feed(local_input_t *s, uint32_t now_ms, uint8_t pins);

/* 取一个事件。返回 `false` 表示队列空。 */
bool local_input_pop(local_input_t *s, local_event_t *out);

/* 因队列满而丢弃的事件数。 */
uint32_t local_input_dropped(const local_input_t *s);

#endif /* APP_LOCAL_INPUT_H */
