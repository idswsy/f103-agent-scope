/* App/local_input.c —— 见 local_input.h 的说明。 */

#include "local_input.h"

/* ⚠ **`NULL` 必须自己 include。**
 *
 * 2026-10-05 真机编译：ARMCC5 报 `#20: identifier "NULL" is undefined` ×4，
 * 而 gcc（本机与 CI 的 Ubuntu runner）**都放行** —— 因为它们的 `<stdint.h>`
 * 会传递引入 `<stddef.h>`。宿主的头文件比 ARMCC5 宽松，这类差异在主机上
 * 永远看不出来。CI 里那条「严格 C99」检查也抓不到它。
 *
 * `<stddef.h>` 是 C99 标准里 `NULL` 的定义处（§7.17）。 */
#include <stddef.h>

/* 三键 + 编码器按下在引脚位图里的那 4 位，按 bit0..bit3 的次序。 */
static const uint8_t BUTTON_PINS[4] = {
    LOCAL_PIN_KEY1, LOCAL_PIN_KEY2, LOCAL_PIN_KEY3, LOCAL_PIN_ENC_SW,
};

/* 每个按钮的**按下**事件。 */
static const local_event_t BUTTON_EVENTS[4] = {
    LOCAL_EV_KEY1, LOCAL_EV_KEY2, LOCAL_EV_KEY3, LOCAL_EV_ENC_PUSH,
};

/* 正交译码表。索引 = (上一次的 A,B) << 2 | (这一次的 A,B)，值是这一步
 * 代表的方向：+1 / -1 / 0（0 = 非法跳变，例如两相同时变化 —— 那是漏采样
 * 或者抖动，**丢掉它**比猜一个方向安全）。
 *
 * 合法的四个相位按灰码次序 00 → 01 → 11 → 10 → 00 走一圈；
 * 每一对**相邻**状态都要是同一个方向，这是这张表唯一要说清的事。 */
static const int8_t ENC_TABLE[16] = {
     0, -1,  1,  0,
     1,  0,  0, -1,
    -1,  0,  0,  1,
     0,  1, -1,  0,
};

/* ── 事件队列 ─────────────────────────────────────────────────── */

static void push(local_input_t *s, local_event_t ev)
{
    if (s->q_count >= LOCAL_EVENT_QUEUE) {
        /* 满则丢**最旧**的：最新的那次操作是用户此刻看到反馈的那一次，
         * 丢掉它比丢掉一个两秒前的按键更让人困惑。 */
        s->q_head = (uint8_t)((s->q_head + 1u) % LOCAL_EVENT_QUEUE);
        s->q_count--;
        s->dropped++;
    }
    {
        uint8_t tail = (uint8_t)((s->q_head + s->q_count) % LOCAL_EVENT_QUEUE);
        s->q[tail] = ev;
        s->q_count++;
    }
}

bool local_input_pop(local_input_t *s, local_event_t *out)
{
    if (s == NULL || out == NULL || s->q_count == 0u) {
        return false;
    }
    *out = s->q[s->q_head];
    s->q_head = (uint8_t)((s->q_head + 1u) % LOCAL_EVENT_QUEUE);
    s->q_count--;
    return true;
}

uint32_t local_input_dropped(const local_input_t *s)
{
    return (s != NULL) ? s->dropped : 0u;
}

/* ── 引脚位图 → 按钮位 ────────────────────────────────────────── */

/* 把引脚位图里那 4 个按钮位抽出来、翻成"1 = 按下"、压成连续的 4 位。 */
static uint8_t buttons_from_pins(uint8_t pins)
{
    uint8_t out = 0u;
    uint32_t i;

    for (i = 0u; i < 4u; i++) {
        if ((pins & BUTTON_PINS[i]) == 0u) {   /* 低有效：读到 0 = 按下 */
            out |= (uint8_t)(1u << i);
        }
    }
    return out;
}

static uint8_t encoder_state(uint8_t pins)
{
    uint8_t a = ((pins & LOCAL_PIN_ENC_A) != 0u) ? 1u : 0u;
    uint8_t b = ((pins & LOCAL_PIN_ENC_B) != 0u) ? 1u : 0u;
    return (uint8_t)((a << 1) | b);
}

/* ── 对外接口 ─────────────────────────────────────────────────── */

void local_input_init(local_input_t *s, uint32_t now_ms, uint8_t pins)
{
    uint32_t i;

    if (s == NULL) {
        return;
    }

    s->raw = buttons_from_pins(pins);
    /* **上电时按着的键不算一次按下。** 否则每次复位都会凭空冒出一个事件，
     * 而用户什么都没做 —— 这与 `trigger.c` 里「初始化必须用真样点而不是 0」
     * 是同一类错误。 */
    s->stable = s->raw;

    for (i = 0u; i < 4u; i++) {
        s->raw_ms[i] = now_ms;
    }

    s->enc_last = encoder_state(pins);
    s->enc_accum = 0;

    s->q_head = 0u;
    s->q_count = 0u;
    s->dropped = 0u;
}

void local_input_feed(local_input_t *s, uint32_t now_ms, uint8_t pins)
{
    uint8_t now_buttons;
    uint32_t i;

    if (s == NULL) {
        return;
    }

    /* ── 按钮：先看原始电平变没变，再看它稳住了没有 ── */
    now_buttons = buttons_from_pins(pins);

    for (i = 0u; i < 4u; i++) {
        uint8_t bit = (uint8_t)(1u << i);
        uint8_t raw_now = (uint8_t)((now_buttons & bit) != 0u);
        uint8_t raw_was = (uint8_t)((s->raw & bit) != 0u);
        uint8_t stable_was = (uint8_t)((s->stable & bit) != 0u);

        /* ⚠ **先结算上一段，再看这一刻电平变没变。**
         *
         * 回归：原来的写法是「电平一变就重新计时、把上一段丢掉」，于是
         * 只要两次调用之间跨过了一次完整的按下-松开，那次按下就永远得不到
         * 确认（松开也一样），紧随其后的那一次按下会被整个吞掉。
         *
         * 这个场景是真的会发生的：`LinkUart_Write` 在发送环满时可以阻塞
         * **22 ms**（见 `Hardware/src/link_uart.c`），比 20 ms 的消抖窗口还长。
         *
         * `>=` 而不是 `==`：主循环不是等间隔的，20 ms 那一刻未必刚好被调用到。 */
        if ((uint32_t)(now_ms - s->raw_ms[i]) >= LOCAL_DEBOUNCE_MS && raw_was != stable_was) {
            s->stable = (uint8_t)((s->stable & (uint8_t)~bit) | (uint8_t)(raw_was ? bit : 0u));
            if (raw_was != 0u) {
                /* 只在**按下**时出事件。松开也是一个稳定沿，但用户
                 * 感知到的动作是"按"那一下。 */
                push(s, BUTTON_EVENTS[i]);
            }
        }

        if (raw_now != raw_was) {
            s->raw = (uint8_t)((s->raw & (uint8_t)~bit) | (uint8_t)(raw_now ? bit : 0u));
            s->raw_ms[i] = now_ms;
        }
    }

    /* ── 编码器：每变一个相位就走一步表 ── */
    {
        uint8_t st = encoder_state(pins);

        if (st != s->enc_last) {
            int8_t step = ENC_TABLE[((uint32_t)s->enc_last << 2) | st];
            s->enc_last = st;

            s->enc_accum = (int8_t)(s->enc_accum + step);

            while (s->enc_accum >= LOCAL_ENC_EDGES_PER_DETENT) {
                s->enc_accum = (int8_t)(s->enc_accum - LOCAL_ENC_EDGES_PER_DETENT);
                push(s, LOCAL_EV_ENC_CW);
            }
            while (s->enc_accum <= -LOCAL_ENC_EDGES_PER_DETENT) {
                s->enc_accum = (int8_t)(s->enc_accum + LOCAL_ENC_EDGES_PER_DETENT);
                push(s, LOCAL_EV_ENC_CCW);
            }
        }
    }
}
