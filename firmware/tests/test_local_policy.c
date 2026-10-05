/* tests/test_local_policy.c —— 本地按键的语义（不需要板子）
 *
 * 这一层测的是「哪个键在哪个状态下做什么」，其中最容易写错的两条是：
 * 空闲时按急停不该"停一次给你看"，以及 DONE 状态下按急停不该把
 * 刚采到的那一帧丢掉。
 */

#include "local_policy.h"
#include "test_util.h"

static void test_stop_only_acts_when_something_is_running(void)
{
    GROUP("急停");

    CHECK(local_policy_decide(LOCAL_EV_KEY1, STATE_ARMED) == LOCAL_ACT_ACQ_STOP,
          "ARMED 下按 KEY1 应当急停");
    CHECK(local_policy_decide(LOCAL_EV_KEY1, STATE_STREAMING) == LOCAL_ACT_ACQ_STOP,
          "STREAMING 下按 KEY1 也应当急停（该状态目前还没被用到，但语义要对）");

    CHECK(local_policy_decide(LOCAL_EV_KEY1, STATE_IDLE) == LOCAL_ACT_NONE,
          "空闲时按 KEY1 不该有动作 —— 报「已停止」会让人以为刚才有东西在跑");
    CHECK(local_policy_decide(LOCAL_EV_KEY1, STATE_DONE) == LOCAL_ACT_NONE,
          "DONE 下按 KEY1 不该把刚采到的那一帧丢掉");
}

static void test_rearm_follows_the_engines_own_gate(void)
{
    GROUP("重采");

    /* 门禁必须与 `acq_arm` 自己的一致（IDLE / DONE），否则按下去只会
     * 得到一句 ACQ_ERR_STATE，而用户看不出为什么。 */
    CHECK(local_policy_decide(LOCAL_EV_KEY2, STATE_IDLE) == LOCAL_ACT_ACQ_ARM,
          "IDLE 下按 KEY2 应当重采");
    CHECK(local_policy_decide(LOCAL_EV_KEY2, STATE_DONE) == LOCAL_ACT_ACQ_ARM,
          "DONE 下按 KEY2 应当重采");

    CHECK(local_policy_decide(LOCAL_EV_KEY2, STATE_ARMED) == LOCAL_ACT_NONE,
          "ARMED 中途按 KEY2 不该重臂（引擎自己也会拒绝）");
}

static void test_function_generator_controls(void)
{
    GROUP("函数发生器");

    /* KEY3 开 / 关输出，编码器转向调频率 —— 与上游 KEY2/KEY1/KEY3 的分工对齐。 */
    CHECK(local_policy_decide(LOCAL_EV_KEY3, STATE_IDLE) == LOCAL_ACT_GEN_TOGGLE,
          "KEY3 应当开关函数发生器");
    CHECK(local_policy_decide(LOCAL_EV_ENC_CW, STATE_IDLE) == LOCAL_ACT_GEN_FASTER,
          "编码器正转应当升频");
    CHECK(local_policy_decide(LOCAL_EV_ENC_CCW, STATE_ARMED) == LOCAL_ACT_GEN_SLOWER,
          "编码器反转应当降频（与采集状态无关）");
}

static void test_generator_frequency_steps_are_multiplicative(void)
{
    GROUP("函数发生器频率档");

    /* 1 kHz 上下一档 —— 倍增而不是加减，否则低频段一步跨过整个量程。 */
    CHECK(local_policy_next_gen_hz(1000u, true) == 2000u, "1 kHz 上一档应是 2 kHz");
    CHECK(local_policy_next_gen_hz(1000u, false) == 500u, "1 kHz 下一档应是 500 Hz");

    /* 两端：上不超 500 kHz、下不低于 16 Hz。 */
    CHECK(local_policy_next_gen_hz(400000u, true) == 500000u,
          "400 kHz 上一档应夹到 500 kHz，实测 %u", (unsigned)local_policy_next_gen_hz(400000u, true));
    CHECK(local_policy_next_gen_hz(32u, false) == 16u,
          "32 Hz 下一档应夹到 16 Hz，实测 %u", (unsigned)local_policy_next_gen_hz(32u, false));

    /* 已经在边界上再按：停在边界，不能回绕。 */
    CHECK(local_policy_next_gen_hz(500000u, true) == 500000u, "已在顶再升应停在顶");
    CHECK(local_policy_next_gen_hz(16u, false) == 16u, "已在底再降应停在底");
}

static void test_unassigned_events_do_nothing(void)
{
    GROUP("未分配的事件");

    CHECK(local_policy_decide(LOCAL_EV_ENC_PUSH, STATE_ARMED) == LOCAL_ACT_NONE,
          "编码器按下尚未分配");
    CHECK(local_policy_decide(LOCAL_EV_NONE, STATE_ARMED) == LOCAL_ACT_NONE,
          "空事件不该有动作");
}

int main(void)
{
    test_stop_only_acts_when_something_is_running();
    test_rearm_follows_the_engines_own_gate();
    test_function_generator_controls();
    test_generator_frequency_steps_are_multiplicative();
    test_unassigned_events_do_nothing();

    return test_summary();
}
