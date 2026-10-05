/* tests/test_local_input.c —— 按键消抖与编码器正交译码（不需要板子）
 *
 * 这些边界条件在真机上都要靠"恰好抖动成那样"才能碰到，而它们恰恰是
 * 「按了没反应」「转一格跳两格」「反着转」这类现象的来源。放在纯逻辑层
 * 就能穷举干净 —— 这是 ADR-008 那条纪律换来的开发能力。
 */

#include "local_input.h"
#include "test_util.h"

#include <string.h>

/* 全部按键松开、编码器两相都为高的空闲位图。
 * 低有效：高 = 没按。 */
#define IDLE_PINS (LOCAL_PIN_KEY1 | LOCAL_PIN_KEY2 | LOCAL_PIN_KEY3 | \
                   LOCAL_PIN_ENC_A | LOCAL_PIN_ENC_B | LOCAL_PIN_ENC_SW)

/* 按下某个引脚（低有效）。 */
static uint8_t press(uint8_t pins, uint8_t pin)
{
    return (uint8_t)(pins & (uint8_t)~pin);
}

/* 编码器两相的电平组合。`a`/`b` 为真表示该相为高。 */
static uint8_t enc(uint8_t pins, int a, int b)
{
    pins = (uint8_t)(a ? (pins | LOCAL_PIN_ENC_A) : (pins & (uint8_t)~LOCAL_PIN_ENC_A));
    pins = (uint8_t)(b ? (pins | LOCAL_PIN_ENC_B) : (pins & (uint8_t)~LOCAL_PIN_ENC_B));
    return pins;
}

/* 数出队列里剩下几个事件，并把它们读空。 */
static uint32_t drain(local_input_t *s, local_event_t *first)
{
    local_event_t ev;
    uint32_t n = 0u;

    if (first != NULL) {
        *first = LOCAL_EV_NONE;
    }
    while (local_input_pop(s, &ev)) {
        if (n == 0u && first != NULL) {
            *first = ev;
        }
        n++;
        if (n > 64u) {
            break;   /* 防跑飞：这两层循环不该产出这么多 */
        }
    }
    return n;
}

/* ══════════════════════════════════════════════════════════════════
 * 按键消抖
 * ══════════════════════════════════════════════════════════════════ */

static void test_a_clean_press_emits_exactly_one_event(void)
{
    local_input_t s;
    local_event_t ev;
    uint32_t n;

    GROUP("干净的一次按下");

    local_input_init(&s, 0u, IDLE_PINS);

    /* 按下，并在抖动窗口之后被看到 */
    local_input_feed(&s, 5u, press(IDLE_PINS, LOCAL_PIN_KEY1));
    local_input_feed(&s, 30u, press(IDLE_PINS, LOCAL_PIN_KEY1));

    n = drain(&s, &ev);
    CHECK(n == 1u, "一次按下应恰好一个事件，实测 %u 个", (unsigned)n);
    CHECK(ev == LOCAL_EV_KEY1, "事件应是 KEY1，实测 %d", (int)ev);
}

static void test_contact_bounce_does_not_duplicate(void)
{
    local_input_t s;
    uint32_t n;

    GROUP("触点抖动");

    local_input_init(&s, 0u, IDLE_PINS);

    /* 触点在 20 ms 窗口内来回跳 —— 这些跳变都**不算数** */
    local_input_feed(&s, 2u, press(IDLE_PINS, LOCAL_PIN_KEY2));
    local_input_feed(&s, 4u, IDLE_PINS);
    local_input_feed(&s, 6u, press(IDLE_PINS, LOCAL_PIN_KEY2));
    local_input_feed(&s, 8u, IDLE_PINS);
    local_input_feed(&s, 10u, press(IDLE_PINS, LOCAL_PIN_KEY2));
    /* 从这里开始稳住 */
    local_input_feed(&s, 40u, press(IDLE_PINS, LOCAL_PIN_KEY2));

    n = drain(&s, NULL);
    CHECK(n == 1u, "抖动应只产生一个事件，实测 %u 个", (unsigned)n);
}

static void test_holding_a_key_does_not_repeat(void)
{
    local_input_t s;
    uint32_t n;
    uint32_t t;

    GROUP("长按不重复");

    local_input_init(&s, 0u, IDLE_PINS);
    for (t = 10u; t <= 500u; t += 10u) {
        local_input_feed(&s, t, press(IDLE_PINS, LOCAL_PIN_KEY3));
    }

    n = drain(&s, NULL);
    CHECK(n == 1u, "按住 500 ms 应只产生一个事件，实测 %u 个", (unsigned)n);
}

static void test_release_then_press_again_emits_again(void)
{
    local_input_t s;
    uint32_t n;

    GROUP("松开再按");

    local_input_init(&s, 0u, IDLE_PINS);

    local_input_feed(&s, 10u, press(IDLE_PINS, LOCAL_PIN_KEY1));
    local_input_feed(&s, 40u, press(IDLE_PINS, LOCAL_PIN_KEY1));
    local_input_feed(&s, 60u, IDLE_PINS);          /* 松开 */
    local_input_feed(&s, 100u, press(IDLE_PINS, LOCAL_PIN_KEY1));  /* 再按 */
    local_input_feed(&s, 130u, press(IDLE_PINS, LOCAL_PIN_KEY1));

    n = drain(&s, NULL);
    CHECK(n == 2u, "两次按下应产生两个事件，实测 %u 个", (unsigned)n);
}

static void test_a_key_held_at_boot_is_not_an_event(void)
{
    local_input_t s;
    uint32_t n;

    GROUP("上电时已按下");

    /* 复位的那一刻手正按着 KEY1 —— 那不是一次「按下」。
     * 与 trigger.c 里「初始化必须用真样点而不是 0」是同一类错误。 */
    local_input_init(&s, 0u, press(IDLE_PINS, LOCAL_PIN_KEY1));
    local_input_feed(&s, 100u, press(IDLE_PINS, LOCAL_PIN_KEY1));

    n = drain(&s, NULL);
    CHECK(n == 0u, "上电时已按着的键不该产生事件，实测 %u 个", (unsigned)n);
}

static void test_timestamp_wraparound(void)
{
    local_input_t s;
    uint32_t n;

    GROUP("毫秒时间戳回绕");

    /* now_ms 在 0xFFFFFFFF 附近回绕。无符号减法本来就能跨过它，
     * 但这条要有测试钉住 —— 换个写法（比如转成有符号比较）就会坏。 */
    local_input_init(&s, 0xFFFFFFF0u, IDLE_PINS);
    local_input_feed(&s, 0xFFFFFFF8u, press(IDLE_PINS, LOCAL_PIN_KEY1)); /* 8 —— 不够 */
    local_input_feed(&s, 0x00000010u, press(IDLE_PINS, LOCAL_PIN_KEY1)); /* 过绕后够 20 ms 了 */

    n = drain(&s, NULL);
    CHECK(n == 1u, "跨回绕的按下应被识别，实测 %u 个事件", (unsigned)n);
}

/* ══════════════════════════════════════════════════════════════════
 * 编码器
 * ══════════════════════════════════════════════════════════════════ */

static void test_encoder_one_detent_forward(void)
{
    local_input_t s;
    local_event_t ev;
    uint32_t n;
    uint32_t t = 100u;

    GROUP("编码器正转一格");

    local_input_init(&s, 0u, enc(IDLE_PINS, 1, 1));   /* 起始相位 11 */

    /* 11 → 01 → 00 → 10 → 11，四个边沿 = 一格 */
    local_input_feed(&s, t++, enc(IDLE_PINS, 0, 1));
    local_input_feed(&s, t++, enc(IDLE_PINS, 0, 0));
    local_input_feed(&s, t++, enc(IDLE_PINS, 1, 0));
    local_input_feed(&s, t++, enc(IDLE_PINS, 1, 1));

    n = drain(&s, &ev);
    CHECK(n == 1u, "四个边沿应恰好一格，实测 %u 个事件", (unsigned)n);
    CHECK(ev == LOCAL_EV_ENC_CW, "方向应为正转，实测 %d", (int)ev);
}

static void test_encoder_one_detent_backward(void)
{
    local_input_t s;
    local_event_t ev;
    uint32_t n;
    uint32_t t = 100u;

    GROUP("编码器反转一格");

    local_input_init(&s, 0u, enc(IDLE_PINS, 1, 1));

    /* 反向走：11 → 10 → 00 → 01 → 11 */
    local_input_feed(&s, t++, enc(IDLE_PINS, 1, 0));
    local_input_feed(&s, t++, enc(IDLE_PINS, 0, 0));
    local_input_feed(&s, t++, enc(IDLE_PINS, 0, 1));
    local_input_feed(&s, t++, enc(IDLE_PINS, 1, 1));

    n = drain(&s, &ev);
    CHECK(n == 1u, "反向四个边沿也应恰好一格，实测 %u 个事件", (unsigned)n);
    CHECK(ev == LOCAL_EV_ENC_CCW, "方向应为反转，实测 %d", (int)ev);
}

static void test_encoder_partial_turn_does_not_emit(void)
{
    local_input_t s;
    uint32_t n;

    GROUP("没转满一格");

    local_input_init(&s, 0u, enc(IDLE_PINS, 1, 1));
    local_input_feed(&s, 10u, enc(IDLE_PINS, 0, 1));
    local_input_feed(&s, 20u, enc(IDLE_PINS, 0, 0));
    /* 停在这里 —— 还差两个边沿 */

    n = drain(&s, NULL);
    CHECK(n == 0u, "不足一格不该出事件，实测 %u 个", (unsigned)n);
}

static void test_encoder_turn_and_back_is_net_zero(void)
{
    local_input_t s;
    uint32_t n;
    uint32_t t = 100u;

    GROUP("转过去又转回来");

    local_input_init(&s, 0u, enc(IDLE_PINS, 1, 1));

    local_input_feed(&s, t++, enc(IDLE_PINS, 0, 1));
    local_input_feed(&s, t++, enc(IDLE_PINS, 0, 0));
    /* 原路返回 */
    local_input_feed(&s, t++, enc(IDLE_PINS, 0, 1));
    local_input_feed(&s, t++, enc(IDLE_PINS, 1, 1));

    n = drain(&s, NULL);
    CHECK(n == 0u, "来回各两步应该净零，实测 %u 个事件", (unsigned)n);
}

static void test_encoder_invalid_transition_is_ignored(void)
{
    local_input_t s;
    uint32_t n;

    GROUP("非法跳变");

    local_input_init(&s, 0u, enc(IDLE_PINS, 1, 1));

    /* 11 → 00：两相同时变化。那是漏采样，不是转动 ——
     * 猜一个方向比丢掉更坏（会凭空多出一格）。 */
    local_input_feed(&s, 10u, enc(IDLE_PINS, 0, 0));
    local_input_feed(&s, 20u, enc(IDLE_PINS, 1, 1));
    local_input_feed(&s, 30u, enc(IDLE_PINS, 0, 0));
    local_input_feed(&s, 40u, enc(IDLE_PINS, 1, 1));

    n = drain(&s, NULL);
    CHECK(n == 0u, "两相同时变化应被丢弃，实测 %u 个事件", (unsigned)n);
}

static void test_encoder_many_detents(void)
{
    local_input_t s;
    uint32_t n;
    uint32_t t = 100u;
    uint32_t i;
    uint32_t got = 0u;
    local_event_t ev;

    GROUP("连续转多格");

    local_input_init(&s, 0u, enc(IDLE_PINS, 1, 1));

    for (i = 0u; i < 10u; i++) {
        local_input_feed(&s, t++, enc(IDLE_PINS, 0, 1));
        local_input_feed(&s, t++, enc(IDLE_PINS, 0, 0));
        local_input_feed(&s, t++, enc(IDLE_PINS, 1, 0));
        local_input_feed(&s, t++, enc(IDLE_PINS, 1, 1));
        /* 队列只有 4 深，边转边取 —— 这也顺带证明环没有卡死 */
        while (local_input_pop(&s, &ev)) {
            if (ev == LOCAL_EV_ENC_CW) {
                got++;
            }
        }
    }

    n = drain(&s, NULL);
    CHECK(n == 0u, "边转边取之后队列应为空，实测剩 %u 个", (unsigned)n);
    CHECK(got == 10u, "转 10 格应产生 10 个正转事件，实测 %u 个", (unsigned)got);
}

/* ══════════════════════════════════════════════════════════════════
 * 事件队列
 * ══════════════════════════════════════════════════════════════════ */

static void test_queue_overflow_drops_oldest_and_counts(void)
{
    local_input_t s;
    local_event_t ev;
    uint32_t n;
    uint32_t t = 0u;
    uint32_t i;

    GROUP("队列溢出");

    local_input_init(&s, 0u, IDLE_PINS);

    /* 连按 6 次（多于队列深度 4），每次之间给够消抖时间 */
    for (i = 0u; i < 6u; i++) {
        t += 100u;
        local_input_feed(&s, t, press(IDLE_PINS, LOCAL_PIN_KEY1));
        t += 100u;
        local_input_feed(&s, t, IDLE_PINS);
    }

    n = drain(&s, &ev);
    CHECK(n == LOCAL_EVENT_QUEUE, "队列应满到 %u 个，实测 %u 个",
          (unsigned)LOCAL_EVENT_QUEUE, (unsigned)n);
    CHECK(local_input_dropped(&s) == 2u,
          "应丢掉 2 个并计数，实测 %u", (unsigned)local_input_dropped(&s));
    CHECK(ev == LOCAL_EV_KEY1, "留下的应是最新那几个，头一个是 KEY1，实测 %d", (int)ev);
}

static void test_empty_queue_pop_returns_false(void)
{
    local_input_t s;
    local_event_t ev;

    GROUP("空队列");

    local_input_init(&s, 0u, IDLE_PINS);
    CHECK(!local_input_pop(&s, &ev), "空队列 pop 应返回 false");
    CHECK(!local_input_pop(&s, &ev), "再 pop 一次仍应是 false（不能越界读）");
}

/* ══════════════════════════════════════════════════════════════════ */

int main(void)
{
    test_a_clean_press_emits_exactly_one_event();
    test_contact_bounce_does_not_duplicate();
    test_holding_a_key_does_not_repeat();
    test_release_then_press_again_emits_again();
    test_a_key_held_at_boot_is_not_an_event();
    test_timestamp_wraparound();

    test_encoder_one_detent_forward();
    test_encoder_one_detent_backward();
    test_encoder_partial_turn_does_not_emit();
    test_encoder_turn_and_back_is_net_zero();
    test_encoder_invalid_transition_is_ignored();
    test_encoder_many_detents();

    test_queue_overflow_drops_oldest_and_counts();
    test_empty_queue_pop_returns_false();

    return test_summary();
}
