/* tests/test_trigger.c —— 触发搜索状态机的测试
 *
 * `firmware/README.md` 的「触发搜索：必须是状态机」那一节点名了四个边界用例
 * （缓变斜坡 / 带内抖动 / 对称下降沿 / 从不跨越）—— 前四组就是它们。
 *
 * 第五组是本模块**独有**的性质：固件要在**已发布的半区**上分块搜触发，
 * 所以「分块扫描」与「整块扫描」必须给出同一个下标。
 * 这条性质在 PC 上完全可测，而它正是「状态跨块保留」这个设计的全部意义。
 */

#include "trigger.h"
#include "test_util.h"

#include <string.h>

/* 一个够大的样本缓冲（比任何一次采集都大）。 */
#define CAP 4096u

static uint16_t buf[CAP];

/* 把 `n` 个样点填成一条从 `from` 到 `to` 的线性斜坡。 */
static void ramp(uint32_t n, uint16_t from, uint16_t to)
{
    for (uint32_t i = 0; i < n; i++) {
        buf[i] = (uint16_t)(from + (int32_t)(to - from) * (int32_t)i / (int32_t)(n ? n : 1));
    }
}

/* ── 用例一：缓变斜坡必须能触发 ─────────────────────────────────────
 *
 * 这是「为什么必须是状态机」的直接证据。迟滞带宽 32 LSB，
 * 而 1 kHz 正弦在 857 kSPS 下每样点只走约 14 LSB ——
 * 如果实现写成「相邻两点跨越整条迟滞带」，这条斜坡永远触发不了。
 */
static void case_slow_ramp(void)
{
    GROUP("缓变斜坡");

    const uint16_t level = 2048;
    /* 每步 14 LSB：跨过 32 LSB 的迟滞带要三步 —— 单步判据必然失败 */
    ramp(600, 1400, 2800);

    trig_state_t st;
    trig_reset(&st, true, level, buf[0]);
    uint32_t idx = 0;
    bool hit = trig_scan(&st, buf, 600, level, &idx);

    CHECK(hit, "每步 14 LSB 的斜坡没能触发 —— 迟滞大概写成了单步跨越");
    if (hit) {
        /* 触发点应当落在电平附近，而不是落在斜坡起点 */
        CHECK(buf[idx] >= level, "触发点 %u 还没到电平 %u", buf[idx], level);
        CHECK(idx > 0, "不该在第 0 个样点触发");
    }
}

/* ── 用例二：带内抖动不得触发 ───────────────────────────────────────
 *
 * 信号一直在判决带里来回抖（±8 LSB，带是 ±16）—— 一次都不该触发。
 * 这是迟滞存在的理由：不这样做的话，噪声会把触发点定在随机位置。
 */
static void case_in_band_jitter(void)
{
    GROUP("带内抖动");

    const uint16_t level = 2048;
    /* 在电平附近 ±8 LSB 交替 —— 全部落在 (±16) 的迟滞带内 */
    for (uint32_t i = 0; i < 400; i++) {
        buf[i] = (uint16_t)(level + ((i % 2) ? 8 : (int32_t)-8));
    }

    trig_state_t st;
    trig_reset(&st, true, level, buf[0]);
    uint32_t idx = 0;
    bool hit = trig_scan(&st, buf, 400, level, &idx);
    CHECK(!hit, "带内抖动触发了（在 %u）—— 迟滞没起作用", idx);

    /* 下降沿也试一遍：不该只是一侧漏了 */
    trig_reset(&st, false, level, buf[0]);
    hit = trig_scan(&st, buf, 400, level, &idx);
    CHECK(!hit, "带内抖动在下降沿触发了（在 %u）", idx);
}

/* ── 用例三：下降沿与上升沿的**门限**镜像 ───────────────────────────
 *
 * 「对称」在这里的意思是：上升沿在 `v >= level + 16` 触发、
 * 下降沿在 `v <= level - 16` 触发 —— 两侧用**镜像的门限**。
 * 这与 host/crates/sim 的 `falling_edge_works_symmetrically` 一致。
 *
 * ⚠ **不是**「两个触发点关于峰值位置对称」。迟滞**故意**让它们不对称：
 * 上升跨的是 `level+16`、下降跨的是 `level-16`，两者相差整整一个迟滞带宽，
 * 所以在同一条三角波上，两个触发点会相距「32 LSB ÷ 斜率」那么多步。
 *
 * 这条测试的第一版就是按「位置对称」写的，断言 ±3 步，实测差 6 步 ——
 * 而那个 6 步**恰恰是迟滞应该有的行为**。差点把正确的实现改成错的。
 */
static void case_falling_edge_is_mirrored(void)
{
    GROUP("下降沿与上升沿门限镜像");

    const uint16_t level = 2048;
    uint32_t idx = 0;
    trig_state_t st;

    /* 上升斜坡：必须在 hi 以上才触发 */
    for (uint32_t i = 0; i < 400; i++) {
        buf[i] = (uint16_t)(1000 + (3000 - 1000) * (int32_t)i / 399);
    }
    trig_reset(&st, true, level, buf[0]);
    bool up = trig_scan(&st, buf, 400, level, &idx);
    CHECK(up, "上升斜坡没触发");
    if (up) {
        CHECK(buf[idx] >= level + TRIG_HYSTERESIS_LSB,
              "上升沿应当在 hi(%u) 以上触发，实测 %u",
              level + TRIG_HYSTERESIS_LSB, buf[idx]);
    }

    /* 下降斜坡：必须在 lo 以下才触发 */
    for (uint32_t i = 0; i < 400; i++) {
        buf[i] = (uint16_t)(3000 - (3000 - 1000) * (int32_t)i / 399);
    }
    trig_reset(&st, false, level, buf[0]);
    bool dn = trig_scan(&st, buf, 400, level, &idx);
    CHECK(dn, "下降斜坡没触发");
    if (dn) {
        CHECK(buf[idx] <= level - TRIG_HYSTERESIS_LSB,
              "下降沿应当在 lo(%u) 以下触发，实测 %u",
              level - TRIG_HYSTERESIS_LSB, buf[idx]);
    }
}

/* ── 用例四：从不跨越 → 必须不触发，且不得误判 ─────────────────────
 *
 * 信号一直待在一侧。这里最容易出的错是：初始置位状态算错，
 * 于是「一直都在高电平」被当成一次上升沿。
 */
static void case_never_crosses(void)
{
    GROUP("从不跨越");

    const uint16_t level = 2048;
    uint32_t idx = 0;
    trig_state_t st;

    /* 一直在高侧：上升沿不该触发（它本来就在上面，不是「升上来」的） */
    for (uint32_t i = 0; i < 200; i++) buf[i] = 3000;
    trig_reset(&st, true, level, buf[0]);
    CHECK(!trig_scan(&st, buf, 200, level, &idx),
          "一直高电平不该算上升沿（在 %u）", idx);

    /* 一直在低侧：下降沿不该触发 */
    for (uint32_t i = 0; i < 200; i++) buf[i] = 1000;
    trig_reset(&st, false, level, buf[0]);
    CHECK(!trig_scan(&st, buf, 200, level, &idx),
          "一直低电平不该算下降沿（在 %u）", idx);

    /* 但「一直在高侧」的下降沿应当立刻点到（它已经在 hi 之上，等跌下来即可）*/
    for (uint32_t i = 0; i < 200; i++) buf[i] = 3000;
    for (uint32_t i = 200; i < 400; i++) buf[i] = 1000;
    trig_reset(&st, false, level, buf[0]);
    CHECK(trig_scan(&st, buf, 400, level, &idx),
          "从高跌到低应当产生下降沿");
}

/* ── 用例五：分块扫描必须与整块扫描同结果 ───────────────────────────
 *
 * **这是固件独有的、也是最重要的性质。**
 *
 * 主循环每 2048 个样点处理一个半区，一次触发可能跨块 ——
 * 前半块置位、后半块才跨过。所以状态必须跨调用保留。
 *
 * 判据直接冲着「跨块」去：构造一个**刚好跨在分块边界上**的触发。
 * 只在第一块里搜不到的、且第二块开头就置位的那种布局，会把
 * 「每块重置状态」的实现当场抓出来。
 */
static void case_chunked_equals_whole(void)
{
    GROUP("分块扫描 == 整块扫描");

    const uint16_t level = 2048;
    const uint32_t n = 1000;
    /* 前 700 点在高侧，之后跌到低侧再升回来 —— 触发点在 700 之后 */
    for (uint32_t i = 0; i < n; i++) buf[i] = 3000;
    for (uint32_t i = 300; i < 700; i++) buf[i] = 900;
    for (uint32_t i = 900; i < n; i++) buf[i] = 3000;

    /* 整块 */
    trig_state_t st;
    uint32_t whole = 0;
    trig_reset(&st, true, level, buf[0]);
    bool hit = trig_scan(&st, buf, n, level, &whole);
    CHECK(hit, "整块扫描应当找到触发");
    CHECK(whole >= 700, "触发点应当在低电平段之后，实测 %u", whole);

    /* 各种分块大小都得给出同一个下标 —— 特别是**跨过触发点**的那种切法 */
    const uint32_t chunks[] = {1, 7, 256, 512, 700, 701, 999, 1000};
    for (size_t c = 0; c < sizeof(chunks) / sizeof(chunks[0]); c++) {
        uint32_t step = chunks[c];
        uint32_t got = 0;
        bool found = false;
        trig_reset(&st, true, level, buf[0]);
        for (uint32_t off = 0; off < n && !found; off += step) {
            uint32_t len = (off + step <= n) ? step : (n - off);
            uint32_t local = 0;
            if (trig_scan(&st, buf + off, len, level, &local)) {
                got = off + local;
                found = true;
            }
        }
        CHECK(found, "分块 %u 时没找到触发（整块扫描在 %u）", step, whole);
        CHECK(got == whole, "分块 %u 给出 %u，整块给出 %u —— 状态没跨块保留",
              step, got, whole);
    }
}

/* ── 用例六：门限在量程两端不得跑出范围 ─────────────────────────────
 *
 * level 取 0 或 4095 时，±16 的迟滞会算出负的 lo 或超过 4095 的 hi。
 * 不夹住的话 lo/hi 回绕成很大的值，表现为「这个电平永远不触发」。
 */
static void case_level_at_the_rails(void)
{
    GROUP("电平在量程两端");

    uint32_t idx = 0;
    trig_state_t st;

    /* level = 0：lo 会算成 -16 → 必须夹到 0 */
    for (uint32_t i = 0; i < 64; i++) buf[i] = 0;
    for (uint32_t i = 64; i < 128; i++) buf[i] = 100;
    trig_reset(&st, true, 0, buf[0]);
    CHECK(trig_scan(&st, buf, 128, 0, &idx), "level=0 时斜坡没能触发");

    /* level = 4095：hi 会算成 4111 → 必须夹到 4095 */
    for (uint32_t i = 0; i < 64; i++) buf[i] = 4000;
    for (uint32_t i = 64; i < 128; i++) buf[i] = 4095;
    trig_reset(&st, true, 4095, buf[0]);
    CHECK(trig_scan(&st, buf, 128, 4095, &idx), "level=4095 时斜坡没能触发");
}

/* ── 用例七：空输入与首样点 ─────────────────────────────────────────
 *
 * 空块是常态（半区还没发布），不能崩、不能改状态。
 */
static void case_empty_and_first_sample(void)
{
    GROUP("空输入与首样点");

    const uint16_t level = 2048;
    trig_state_t st;
    uint32_t idx = 12345;

    trig_reset(&st, true, level, 3000);
    CHECK(!trig_scan(&st, buf, 0, level, &idx), "空块不该触发");
    CHECK(idx == 12345, "空块不该改写下标");

    /* 首样点决定初始置位：首样点在高侧 + 找上升沿 → 未置位，
     * 于是「一直在高侧」不触发；而首样点在低侧则会先置位。 */
    trig_state_t a, b;
    for (uint32_t i = 0; i < 32; i++) buf[i] = 3000;
    trig_reset(&a, true, level, 3000); /* 首样点在高侧 */
    CHECK(!trig_scan(&a, buf, 32, level, &idx), "首样点在高侧不该立刻置位");

    for (uint32_t i = 0; i < 32; i++) buf[i] = 1000;
    trig_reset(&b, true, level, 1000); /* 首样点在低侧 */
    for (uint32_t i = 32; i < 64; i++) buf[i] = 3000;
    CHECK(trig_scan(&b, buf, 64, level, &idx), "先低后高应当触发");
}

int main(void)
{
    case_slow_ramp();
    case_in_band_jitter();
    case_falling_edge_is_mirrored();
    case_never_crosses();
    case_chunked_equals_whole();
    case_level_at_the_rails();
    case_empty_and_first_sample();
    return test_summary();
}
