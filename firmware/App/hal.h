/* App/hal.h —— App 层看到的「硬件」
 *
 * # 这个文件是 ADR-008 那条纪律的落点
 *
 * `firmware/README.md` 写着：**`App/` 不许 `#include` 任何 HAL**。
 * 理由很现实 —— 三个人里只有一个人拿得到板子，App/ 一旦依赖 HAL，
 * 另外两个人就只能干等，而这是项目最常见的死法。
 *
 * 这条纪律靠什么落地？就是这个文件：App 层要用的每一样硬件能力，
 * 在这里都是一个**纯函数指针**。没有寄存器、没有 `volatile`、
 * 没有任何厂商头文件 —— 于是 PC 上拿一个假实现就能把整个 App 层跑起来。
 *
 * # 所有权规则（最容易出人命的一条）
 *
 * 采集环是 DMA 循环写的。**绝不能读正在被写的半区。**
 * 只有 HT/TC 中断发布过的半区才可以读 —— 这条约束的接口化身就是
 * [`hal_t::take_published_halves`]：它不是「查询」而是**领取**，
 * 领到之后那一半的所有权才转到 App 手上。
 */

#ifndef APP_HAL_H
#define APP_HAL_H

#include <stdbool.h>
#include <stdint.h>

/* 采集环的几何。与 firmware/README.md 的采样架构一致。 */
#define ACQ_RING_SAMPLES 4096u
#define ACQ_HALF_SAMPLES (ACQ_RING_SAMPLES / 2u)

/* 「已发布半区」位图的两位。 */
#define ACQ_HALF_FIRST  0x1u
#define ACQ_HALF_SECOND 0x2u

/* 设备实际生效的采样率上限（F103 单 ADC，见 docs/04-performance.md）。 */
#define ACQ_RATE_MAX_HZ 857142u
#define ACQ_RATE_MIN_HZ 1u

/* App 层能看到的全部硬件。
 *
 * 由 `Hardware/` 在启动时填好并注册（`App` 只管用，不关心是谁实现的）。
 */
typedef struct {
    /* ── 链路 ──────────────────────────────────────────────── */

    /* 把一段字节交给链路发送。**可以阻塞** —— 直接写就行。 */
    void (*link_write)(const uint8_t *data, uint32_t len);

    /* 收字节。返回实际收到的数量，0 表示暂时没有。
     * **不许阻塞** —— 主循环还要搜触发，卡在串口上会把块中断的处理拖垮。 */
    uint32_t (*link_read)(uint8_t *buf, uint32_t cap);

    /* ── 时间 ──────────────────────────────────────────────── */

    /* 自开机以来的毫秒数。用于 auto 模式的超时与触发 holdoff。 */
    uint32_t (*now_ms)(void);

    /* ── 采集 ──────────────────────────────────────────────── */

    /* 把请求的采样率**量化**到定时器能达到的档位（`72 MHz / (ARR+1)`）。
     *
     * **不启动采集、没有副作用** —— 必须是这样：协议规定 `SET_SAMPLE_RATE`
     * 要当场回显 `actual_hz`，而那时还没 ARM。量化是定时器的固有属性，
     * 问它一下不该把 ADC 打开。
     *
     * 返回 0 表示这个请求根本达不到（调用方应按参数错误处理）。 */
    uint32_t (*quantize_rate_hz)(uint32_t requested_hz);

    /* 请求以 `rate_hz` 开始采样（**必须传已经量化过的值**）。 */
    void (*acq_start)(uint32_t rate_hz);

    /* 再采 `extra_samples` 个样点之后停 DMA。
     *
     * 为什么要「再采一点」而不是立刻停：停 DMA 这个动作本身要花时间，
     * 期间 ADC 还在写。不给余量的话，最后几个样点可能正被写就被读了。 */
    void (*acq_stop)(uint32_t extra_samples);

    /* 环缓冲基址，`ACQ_RING_SAMPLES` 个 u16。 */
    const uint16_t *(*ring_base)(void);

    /* **领取**自上次调用以来被中断发布过的半区，返回位图并清零。
     *
     * 这是所有权交接点：领到的那一半，从现在起归 App 读，DMA 不会再碰它。
     *
     * 实现要点：读位图与清零必须在**关中断**的临界区里做，否则
     * 「读到 bit0 → 中断又置 bit0 → 清零」会丢掉新到的那一次发布。
     */
    uint8_t (*take_published_halves)(void);

    /* 微秒级 tick，用于 EVENT_TRIGGER 的时间戳。 */
    uint32_t (*tick_us)(void);
} hal_t;

#endif /* APP_HAL_H */
