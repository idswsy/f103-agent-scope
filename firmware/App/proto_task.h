/* App/proto_task.h —— 命令分发：收字节 → 解析 → 执行 → 应答
 *
 * # 它是什么
 *
 * 链路层与应用层之间那一层胶水。真正的工作分给了两边：
 *   · 帧的解析与编码  → `proto/protocol.c`（与主机端跑同一份黄金向量）
 *   · 「哪个命令在哪个状态合法」→ `proto_cmd_allowed()`
 *   · 采集本身        → `App/acq.c`
 * 这里只负责把它们串起来，外加**把协议的数据结构翻译成采集状态机的调用**。
 *
 * # 一条容易踩的规矩
 *
 * 解析器返回的 `proto_frame_t.payload` 指向**解析器内部缓冲**，
 * 下一次 `proto_parser_feed()` 之后就会失效。所以处理一帧必须当场做完 ——
 * 「先攒一批帧再一起处理」会把所有 payload 指向同一块被覆盖的内存。
 *
 * # 免回复标志
 *
 * `PROTO_FLAG_NO_REPLY` 置位时不回应答。这条在**错误路径上也要遵守** ——
 * 回一个错误应答同样是应答，会让主机的序号匹配错位。
 */

#ifndef APP_PROTO_TASK_H
#define APP_PROTO_TASK_H

#include "acq.h"
#include "hal.h"
#include "protocol.h"

/* 一次从链路读多少字节。够装一个半满的最大接收帧即可。 */
#define PT_RX_CHUNK 64u

typedef struct {
    const hal_t *hal;
    /* 采集状态机。**不拥有** —— 由 `main` 持有，这里只驱动它。 */
    acq_t *acq;

    proto_parser_t parser;
    uint8_t rx[PT_RX_CHUNK];

    /* 设备主动上报用的序号。与主机的请求序号是**两个空间** ——
     * 事件没有对应的请求，随便编一个不冲突的就行。 */
    uint16_t event_seq;

    /* ── 统计 ── */
    uint32_t frames_rx;
    uint32_t frames_tx;
    uint16_t last_error_code;

    /* 发送缓冲。一次一帧、发完即复用，所以只留一份。 */
    uint8_t tx[PROTO_MAX_FRAME_TX];
} proto_task_t;

/* 初始化。`hal` 与 `acq` 的生命周期必须长于本对象。 */
void proto_task_init(proto_task_t *pt, const hal_t *hal, acq_t *acq);

/* 主循环反复调用：能收多少收多少，能处理多少处理多少，**绝不阻塞**。 */
void proto_task_poll(proto_task_t *pt);

/* 把采集状态机产生的事件主动推给主机。 */
void proto_task_emit(proto_task_t *pt, const acq_out_t *ev);

/* 最近一次错误码（GET_STATUS 上报）。 */
uint16_t proto_task_last_error(const proto_task_t *pt);

/* 收到的 CRC 错误 / 丢弃字节数（GET_STATUS 上报）。 */
uint32_t proto_task_crc_err(const proto_task_t *pt);
uint32_t proto_task_dropped(const proto_task_t *pt);

#endif /* APP_PROTO_TASK_H */
