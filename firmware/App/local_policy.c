/* App/local_policy.c —— 见 local_policy.h 的说明。 */

#include "local_policy.h"

local_action_t local_policy_decide(local_event_t ev, scope_state_t acq_state)
{
    switch (ev) {
    case LOCAL_EV_KEY1:
        /* **急停。** 只在真的有事可停时才动作 —— 空闲时按下去什么都不做，
         * 而不是"停一次给你看"（那会让人以为刚才有东西在跑）。 */
        if (acq_state == STATE_ARMED || acq_state == STATE_STREAMING) {
            return LOCAL_ACT_ACQ_STOP;
        }
        return LOCAL_ACT_NONE;

    case LOCAL_EV_KEY2:
        /* **用当前配置重采。** 门禁与 `acq_arm` 自己的一致（IDLE / DONE），
         * 否则按下去只会得到一句 `ACQ_ERR_STATE`，而用户看不出为什么。
         *
         * ⚠ 从 DONE 重采会**覆盖掉上一次的采集窗**。对本地按键这是
         * 「再来一次」的自然含义；但如果主机刚读过一帧、还没读第二帧，
         * 它拿到的会是新的那份 —— 所以每一次本地 ARM 都要上报。 */
        if (acq_state == STATE_IDLE || acq_state == STATE_DONE) {
            return LOCAL_ACT_ACQ_ARM;
        }
        return LOCAL_ACT_NONE;

    case LOCAL_EV_KEY3:
        /* 函数发生器开 / 关。上游的 KEY2 就是这一条。 */
        return LOCAL_ACT_GEN_TOGGLE;

    case LOCAL_EV_ENC_CW:
        return LOCAL_ACT_GEN_FASTER;

    case LOCAL_EV_ENC_CCW:
        return LOCAL_ACT_GEN_SLOWER;

    case LOCAL_EV_ENC_PUSH:
        /* 暂未分配（上游的语义是冻结画面）。等 `display.c` 支持冻结再定。 */
        return LOCAL_ACT_NONE;

    case LOCAL_EV_NONE:
    default:
        return LOCAL_ACT_NONE;
    }
}

uint32_t local_policy_next_gen_hz(uint32_t cur_hz, bool up)
{
    uint32_t next;

    /* 与 `Hardware/inc/local_gen.h` 的量程一致。两份常量必须一起改 ——
     * 这里是纯逻辑层，不能 include Hardware 的头。 */
    const uint32_t lo = 16u;
    const uint32_t hi = 500000u;

    if (up) {
        next = cur_hz * 2u;
        if (next > hi || next < cur_hz) {   /* 后者是溢出 */
            next = hi;
        }
    } else {
        next = cur_hz / 2u;
        if (next < lo) {
            next = lo;
        }
    }
    return next;
}
