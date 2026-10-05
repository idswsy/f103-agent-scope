/* App/local_policy.h —— 本地按键「按下去应该发生什么」。
 *
 * # 为什么单独一个文件
 *
 * 这台仪器的设备端是**被上位机 / AI 操控**的，本地按键是一条**旁路**。
 * 两条路都想动采集状态机，所以「哪个事件在哪个状态下做什么」必须是一处
 * 能读、能测的定义，而不是散在 `main.c` 的 `switch` 里。
 *
 * 这里的函数是**纯的**：给「事件 + 当前状态」，回「动作」。它不调用
 * `acq_arm` / `acq_stop`，也不发任何上报 —— 那是装配点的事。
 *
 * ⚠ **它只决定"做什么"，不决定"要不要告诉主机"。** 每一次本地动作都会
 * 被上报（见协议侧的 `EVENT_KEY`），因为一次人工急停会让正在跑的 Agent
 * 超时，而**它必须能知道那是人按的，不是设备坏了**。
 */

#ifndef APP_LOCAL_POLICY_H
#define APP_LOCAL_POLICY_H

#include "local_input.h"
#include "protocol.h"

typedef enum {
    LOCAL_ACT_NONE = 0,
    /* 停止采集（急停）。 */
    LOCAL_ACT_ACQ_STOP,
    /* 用当前配置重新布防。 */
    LOCAL_ACT_ACQ_ARM,
    /* 函数发生器输出开 / 关。 */
    LOCAL_ACT_GEN_TOGGLE,
    /* 函数发生器频率 ×2 / ÷2。 */
    LOCAL_ACT_GEN_FASTER,
    LOCAL_ACT_GEN_SLOWER,
} local_action_t;

/* 给一个本地事件与采集状态机的当前状态，回一个动作。 */
local_action_t local_policy_decide(local_event_t ev, scope_state_t acq_state);

/* 函数发生器频率的下一档（×2 或 ÷2）。
 *
 * 倍增而不是加减 —— 信号源的频率是有量级的（16 Hz 到 500 kHz 跨了五十年），
 * 加常数要么在低频段慢得没法用、要么在高频段一步跨过整个量程。
 * 超出范围就停在边界，返回夹过之后的值。 */
uint32_t local_policy_next_gen_hz(uint32_t cur_hz, bool up);

#endif /* APP_LOCAL_POLICY_H */
