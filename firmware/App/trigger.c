/* App/trigger.c —— 触发搜索的实现。语义说明见 trigger.h。 */

#include "trigger.h"

/* 把触发电平换算成迟滞带的上下门限。
 *
 * 两端都要夹住：下界防下溢，上界防超出 12-bit。
 * 少了任一端的后果是门限跑出量程 —— 低于 0 的 lo 永远等不到、高于 4095 的 hi
 * 永远跨不过，表现都是「**这个电平永远不触发**」，而调用方只会看到「没触发」，
 * 看不出是门限算错了。
 */
static void thresholds(uint16_t level, uint16_t *lo, uint16_t *hi)
{
    *lo = (level > TRIG_HYSTERESIS_LSB) ? (uint16_t)(level - TRIG_HYSTERESIS_LSB) : 0u;
    uint32_t h = (uint32_t)level + TRIG_HYSTERESIS_LSB;
    *hi = (h > TRIG_ADC_MAX_LSB) ? (uint16_t)TRIG_ADC_MAX_LSB : (uint16_t)h;
}

void trig_reset(trig_state_t *st, bool rising, uint16_t level, uint16_t first_sample)
{
    uint16_t lo, hi;
    thresholds(level, &lo, &hi);

    st->rising = rising;
    /* 初始置位状态：信号**一开始就在**迟滞带的「已越过」那一侧才算置位。
     * 这一条与 host/crates/sim 的 find_trigger 逐字对应。 */
    st->primed = rising ? (first_sample <= lo) : (first_sample >= hi);
}

bool trig_scan(trig_state_t *st, const uint16_t *samples, uint32_t n, uint16_t level,
               uint32_t *index)
{
    if (n == 0) {
        return false;
    }

    uint16_t lo, hi;
    thresholds(level, &lo, &hi);

    for (uint32_t i = 0; i < n; i++) {
        uint16_t v = samples[i];

        if (!st->rising) {
            /* 下降沿：先确认升到 hi 以上，再等跌到 lo 以下 */
            if (v >= hi) {
                st->primed = true;
            } else if (v <= lo && st->primed) {
                *index = i;
                return true;
            }
            continue;
        }

        /* 上升沿：先确认跌到 lo 以下，再等升到 hi 以上 */
        if (v <= lo) {
            st->primed = true;
        } else if (v >= hi && st->primed) {
            *index = i;
            return true;
        }
        /* 带内的值：不改状态、不触发 —— 迟滞抑制抖动就靠这一条 */
    }
    return false;
}
