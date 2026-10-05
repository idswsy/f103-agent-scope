/* Hardware/src/display.c —— 见 display.h 的说明。
 *
 * # 布局是照上游那台的样子做的
 *
 * 坐标逐项对着 `Hardware/src/tft.c` 的 `TFT_StaticUI`（:510）与 `TFT_ShowUI`（:577），
 * 真机上显示什么、摆在哪，两边一致。
 *
 * ⚠ **但有两处配色与那份源码不同，以真机照片为准。**
 * 上游源码里标题与两个底部数值写的是「黑字绿底」（`BLACK, GREEN`），
 * 而真机上是**绿字黑底**；黄底那几项（PWM / 打开 / 输出频率 / 占空比）
 * 则与源码一致。也就是说**板上跑的那版源码与我们仓库里这份在这两处不一样**。
 * 这里按看到的效果做 —— 那是用户认的"原来那个界面"。
 *
 * # 为什么不用 `TFT_StaticUI` / `TFT_ShowUI` 本身
 *
 * 见 ADR-014：`drawCurve` 每列约 138 次 SPI 调用（折线逐像素 + 擦 51 点），
 * 100 列一帧 12 ms 以上，而采集期间主循环单轮预算只有 2.39 ms。
 * 这里保留上游的**观感**，把绘制换成整列批量写。
 */

#include "display.h"

#include <stddef.h>

#include "local_freq.h"
#include "local_gen.h"
#include "tft.h"
#include "tft_init.h"
#include "waveform.h"

/* ── 布局（坐标取自上游 `TFT_StaticUI` / `TFT_ShowUI`）──────────── */

#define WAVE_X0   0u
#define WAVE_Y0   30u
#define WAVE_Y1   (WAVE_Y0 + WAVE_ROWS - 1u)      /* 80 */

#define SEP_X     106u
#define PANEL_X   110u

#define TITLE_X   10u
#define LBL_PWM_Y     0u
#define LBL_STATE_Y   20u
#define VAL_STATE_X   118u
#define VAL_STATE_Y   36u
#define LBL_OFREQ_Y   56u
#define VAL_OFREQ_X   110u
#define VAL_OFREQ_Y   72u
#define VAL_DUTY_X    110u
#define VAL_DUTY_Y    106u

#define LBL_VPP_Y     92u
#define VAL_VPP_X     5u
#define VAL_VPP_Y     106u
#define LBL_FREQ_Y    92u
#define VAL_FREQ_X    55u
#define VAL_FREQ_Y    106u

/* 数值字段固定 6 字符宽（16px ASCII 每字 8 px → 48 px，正好放进右侧 50 px）。
 *
 * 定宽不只是排版：`TFT_ShowChar` 的 mode=0 会把字符格的背景一起写掉，
 * 于是"新值比旧值短"时尾巴自动擦干净。**定宽的前提是每一帧长度一致**。 */
#define PANEL_CHARS 6u

/* ── 颜色 ─────────────────────────────────────────────────────── */

#define COL_BLACK  0x0000u
#define COL_WHITE  0xFFFFu
#define COL_GREEN  0x07E0u
#define COL_YELLOW 0xFFE0u
#define COL_PURPLE 0x780Fu

/* ── 每轮的绘制预算 ──────────────────────────────────────────── */

/* 每轮最多画几列。一列 = 1 次 `TFT_Address_Set`（7 次 SPI 调用）
 * + 1 次 102 B 的批量传输（线上 45 µs），合计约 55 µs。
 * 8 列 ≈ 440 µs，远小于采集期间 2.39 ms 的单轮预算。 */
#define COLS_PER_TICK 8u

/* ── 状态 ─────────────────────────────────────────────────────── */

typedef enum {
    PH_IDLE = 0,
    PH_COLUMNS,
    PH_TEXT,
} phase_t;

/* 要动态刷新的数值字段。顺序就是刷新的顺序。 */
typedef enum {
    F_VPP = 0,
    F_IN_FREQ,
    F_OUT_STATE,
    F_OUT_FREQ,
    F_OUT_DUTY,
    F_COUNT
} field_id_t;

static wave_frame_t s_frame;

/* 一列的像素（51 行 × 2 B）。
 * 存**字节**而不是 uint16：ST7735 收的是大端序，而 ARM 是小端。 */
static uint8_t s_col_buf[WAVE_ROWS * 2u];

/* 五个字段的文本。各 6 字符 + 结尾。 */
static char s_text[F_COUNT][PANEL_CHARS + 1u];

static phase_t  s_phase;
static uint32_t s_col;
static uint8_t  s_field;      /* 正在刷哪个字段 */
static uint8_t  s_pos;        /* 字段里刷到第几个字符/字 */
static bool     s_ready;
static bool     s_fault;

/* ── 小工具 ───────────────────────────────────────────────────── */

/* 画一个**闭区间**矩形。
 *
 * ⚠ 必须用这个包装，不要直接调 `TFT_Fill` —— 它的 `xend`/`yend` 是
 * **开区间**：内部先 `TFT_Address_Set(x0, y0, x1-1, y1-1)` 设窗口，
 * 再按 `x0..x1-1` 循环写。两端都减一，所以**闭合的那一次要由调用方补**。
 *
 * 传错方向的后果**完全没有提示**：`TFT_Fill(106, 0, 106, 127, c)` 得到窗口
 * `(106, 0, 105, 126)` —— 起点大于终点，`for` 一次都不进，一个像素不写。
 * 2026-10-05 上板实测：右侧分隔线与两条坐标轴就是这么静默消失的。 */
static void fill_rect(uint16_t x0, uint16_t y0, uint16_t x1, uint16_t y1, uint16_t color)
{
    TFT_Fill(x0, y0, (uint16_t)(x1 + 1u), (uint16_t)(y1 + 1u), color);
}

/* 把一个 6 字符字段填成定长。
 *
 * 上游用的是 `sprintf`，这里手写 —— 引 `sprintf` 会把一大坨 C 库拉进镜像，
 * 而布局只需要三种形状：`%1.2fV `、`%3dKHz`/`%3dHz `、`  %2d%%`。 */
static void fmt_fixed6(char *out, const char *body)
{
    uint32_t i = 0u;
    while (i < PANEL_CHARS) {
        out[i] = (body[i] != '\0') ? body[i] : ' ';
        i++;
    }
    out[i] = '\0';
}

/* 把 `v` 按 `%3d` 的规则写进 `body[0..2]`：三位宽、右对齐、多余补空格。
 *
 * 手写而不是引 `printf` —— 一台仪器为了印三个数字拉进整坨 C 库不划算。 */
static void fmt_three_digits(uint32_t v, char *body)
{
    if (v > 999u) {
        v = 999u;
    }
    body[0] = (v >= 100u) ? (char)('0' + ((v / 100u) % 10u)) : ' ';
    body[1] = (v >= 10u) ? (char)('0' + ((v / 10u) % 10u)) : ' ';
    body[2] = (char)('0' + (v % 10u));
}

/* 电压：`%1.2fV ` 的形状 —— `3300 mV` → `"3.30V "`。整数运算，不引浮点。 */
static void fmt_volts(uint32_t mv, char *out)
{
    char body[PANEL_CHARS + 1u];
    uint32_t whole = mv / 1000u;
    uint32_t frac = (mv % 1000u) / 10u;    /* 两位小数 */

    if (whole > 9u) {
        whole = 9u;                        /* 字段只有一位整数位 */
    }
    body[0] = (char)('0' + whole);
    body[1] = '.';
    body[2] = (char)('0' + (frac / 10u));
    body[3] = (char)('0' + (frac % 10u));
    body[4] = 'V';
    body[5] = ' ';
    body[6] = '\0';

    fmt_fixed6(out, body);
}

/* 频率：`>=1 kHz` 用 `%3dKHz`，否则 `%3dHz `。两支都是 6 字符。 */
static void fmt_hz(uint32_t hz, char *out)
{
    char body[PANEL_CHARS + 1u];

    if (hz >= 1000u) {
        fmt_three_digits(hz / 1000u, body);
        body[3] = 'K';
        body[4] = 'H';
        body[5] = 'z';
    } else {
        fmt_three_digits(hz, body);
        body[3] = 'H';
        body[4] = 'z';
        body[5] = ' ';
    }
    body[PANEL_CHARS] = '\0';

    fmt_fixed6(out, body);
}

/* 占空比：`  %2d%%` —— 两位宽右对齐 + `%`，所以 100% 会显示成 `100%`
 * 撑满五格。上游就是这么写的，这里保持一致。 */
static void fmt_duty(uint16_t permille, char *out)
{
    uint32_t pct = (uint32_t)permille / 10u;   /* 千分比 → 百分比 */
    char body[PANEL_CHARS + 1u];

    if (pct > 99u) {
        pct = 99u;
    }
    body[0] = ' ';
    body[1] = (char)('0' + ((pct / 10u) % 10u));
    body[2] = (char)('0' + (pct % 10u));
    body[3] = '%';
    body[4] = ' ';
    body[5] = ' ';
    body[6] = '\0';

    fmt_fixed6(out, body);
}

/* 往屏上写一串 ASCII（逐字符，mode=0 会连背景一起写）。**只在开机用。** */
static void put_text(uint16_t x, uint16_t y, const char *s, uint16_t fg, uint16_t bg)
{
    uint16_t cx = x;

    while (*s != '\0') {
        TFT_ShowChar(cx, y, (uint8_t)(*s), fg, bg, 16u, 0u);
        cx = (uint16_t)(cx + 8u);
        s++;
    }
}

/* ── 静态框架（只在开机调一次）────────────────────────────────── */

static void draw_static(void)
{
    uint16_t i;

    fill_rect(0u, 0u, (uint16_t)(LCD_W - 1u), (uint16_t)(LCD_H - 1u), COL_BLACK);

    /* 标题：绿字黑底（见文件头 —— 上游源码写的是黑字绿底，真机上是反的） */
    TFT_ShowChinese(TITLE_X, 0u, (uint8_t *)"简易示波器", COL_GREEN, COL_BLACK, 16u, 0u);

    /* 右侧 PWM 面板的标签 */
    put_text(PANEL_X, LBL_PWM_Y, "  PWM ", COL_BLACK, COL_YELLOW);
    TFT_ShowChinese(PANEL_X, LBL_STATE_Y, (uint8_t *)"输出状态", COL_WHITE, COL_PURPLE, 12u, 0u);
    TFT_ShowChinese(PANEL_X, LBL_OFREQ_Y, (uint8_t *)"输出频率", COL_WHITE, COL_PURPLE, 12u, 0u);
    TFT_ShowChinese(VAL_DUTY_X, LBL_VPP_Y, (uint8_t *)"占空比", COL_WHITE, COL_PURPLE, 12u, 0u);

    /* 底部数值行的标签 */
    TFT_ShowChinese(5u, LBL_VPP_Y, (uint8_t *)"输入峰值", COL_WHITE, COL_PURPLE, 12u, 0u);
    TFT_ShowChinese(55u, LBL_FREQ_Y, (uint8_t *)"输入频率", COL_WHITE, COL_PURPLE, 12u, 0u);

    /* 数值栏的底：黄底那块先铺满，绿底那块铺绿。
     * 上游是把占位空格串写一遍（`"      "`）—— 效果一样，但那是 6 个字符
     * × 135 次 SPI 调用；这里直接填矩形。 */
    fill_rect(110u, 36u, 157u, 51u, COL_YELLOW);
    fill_rect(110u, 72u, 157u, 87u, COL_YELLOW);
    fill_rect(110u, 106u, 157u, 121u, COL_YELLOW);
    fill_rect(5u, 106u, 52u, 121u, COL_BLACK);
    fill_rect(55u, 106u, 102u, 121u, COL_BLACK);

    /* 右侧分隔线：**点线**，上游是 `i += 2` */
    for (i = 0u; i <= 128u; i = (uint16_t)(i + 2u)) {
        TFT_DrawPoint(SEP_X, i, COL_YELLOW);
    }

    /* 波形区的坐标轴与刻度 */
    for (i = 0u; i < 100u; i++) {
        TFT_DrawPoint(i, (uint16_t)(WAVE_Y1 + 1u), COL_GREEN);
    }
    for (i = WAVE_Y0; i <= WAVE_Y1; i++) {
        TFT_DrawPoint(0u, i, COL_GREEN);
    }
    for (i = 0u; i < 10u; i++) {
        uint16_t x = (uint16_t)(2u + i * 10u);
        TFT_DrawPoint(x, (uint16_t)(WAVE_Y1 + 2u), COL_GREEN);
        TFT_DrawPoint((uint16_t)(x + 1u), (uint16_t)(WAVE_Y1 + 2u), COL_GREEN);
        TFT_DrawPoint(x, (uint16_t)(WAVE_Y1 + 3u), COL_GREEN);
        TFT_DrawPoint((uint16_t)(x + 1u), (uint16_t)(WAVE_Y1 + 3u), COL_GREEN);
    }
}

/* ── 波形列 ───────────────────────────────────────────────────── */

static void draw_column(uint32_t col)
{
    uint8_t top_row = s_frame.top[col];
    uint8_t bot_row = s_frame.bot[col];
    uint32_t k;

    /* 缓冲第 0 个像素打在窗口的**顶行**（屏幕 y = WAVE_Y0），
     * 而 `top/bot` 的行号是 0 = 底行的。所以要翻过来。 */
    for (k = 0u; k < WAVE_ROWS; k++) {
        uint32_t row = (WAVE_ROWS - 1u) - k;
        uint16_t color = (row >= bot_row && row <= top_row) ? COL_GREEN : COL_BLACK;

        s_col_buf[k * 2u] = (uint8_t)(color >> 8);
        s_col_buf[k * 2u + 1u] = (uint8_t)(color & 0xFFu);
    }

    {
        uint16_t x = (uint16_t)(WAVE_X0 + col);
        TFT_Blit(x, WAVE_Y0, x, WAVE_Y1, s_col_buf, sizeof(s_col_buf));
    }
}

/* ── 数值字段 ─────────────────────────────────────────────────── */

static uint32_t field_len(uint8_t f)
{
    if (f == F_OUT_STATE) {
        return 2u;   /* 打开 / 关闭：两个字 */
    }
    return PANEL_CHARS;
}

static void field_xy(uint8_t f, uint16_t *x, uint16_t *y, uint16_t *fg, uint16_t *bg)
{
    switch (f) {
    case F_VPP:
        *x = VAL_VPP_X;  *y = VAL_VPP_Y;  *fg = COL_GREEN;  *bg = COL_BLACK;  break;
    case F_IN_FREQ:
        *x = VAL_FREQ_X; *y = VAL_FREQ_Y; *fg = COL_GREEN;  *bg = COL_BLACK;  break;
    case F_OUT_STATE:
        *x = VAL_STATE_X; *y = VAL_STATE_Y; *fg = COL_BLACK; *bg = COL_YELLOW; break;
    case F_OUT_FREQ:
        *x = VAL_OFREQ_X; *y = VAL_OFREQ_Y; *fg = COL_BLACK; *bg = COL_YELLOW; break;
    case F_OUT_DUTY:
    default:
        *x = VAL_DUTY_X; *y = VAL_DUTY_Y; *fg = COL_BLACK; *bg = COL_YELLOW; break;
    }
}

/* 画字段里的第 `pos` 个字符。
 *
 * **一次只画一个** —— `TFT_ShowChar` 内部逐像素写，一个 8×16 字符就是
 * 128 次 SPI 调用（约 260 µs），`TFT_ShowChinese16x16` 是 256 次（约 510 µs）。
 * 两个都在 2.39 ms 的单轮预算之内，但不能连画好几个。 */
static void draw_field_char(uint8_t f, uint8_t pos)
{
    uint16_t x;
    uint16_t y;
    uint16_t fg;
    uint16_t bg;

    field_xy(f, &x, &y, &fg, &bg);

    if (f == F_OUT_STATE) {
        /* 中文字模：一个字 16 px 宽。传单字串（3 字节 + NUL），
         * `TFT_ShowChinese` 步进 3 字节，正好一个。 */
        const char *s = LocalGen_IsEnabled() ? "打开" : "关闭";
        uint8_t one[4];
        one[0] = (uint8_t)s[pos * 3];
        one[1] = (uint8_t)s[pos * 3 + 1];
        one[2] = (uint8_t)s[pos * 3 + 2];
        one[3] = '\0';
        TFT_ShowChinese((uint16_t)(x + pos * 16u), y, one, fg, bg, 16u, 0u);
        return;
    }

    {
        char ch = s_text[f][pos];
        if (ch == '\0') {
            ch = ' ';
        }
        TFT_ShowChar((uint16_t)(x + pos * 8u), y, (uint8_t)ch, fg, bg, 16u, 0u);
    }
}

/* 把五个字段的文本按当前状态重新算一遍。 */
static void refresh_text(void)
{
    uint32_t in_hz = 0u;
    bool have_hz;

    /* 输入峰值：来自采集窗的峰峰值（ADC LSB → 毫伏，占位换算）。
     *
     * ⚠ 这是**未标定**的，而且底板的模拟前端是 `Uadc = (5 − Vin) / 2`，
     * 输入摆幅是 ADC 摆幅的两倍，占位换算不含这个因子。真值要标定之后才有
     * （见 docs/02-hardware.md §9）。 */
    fmt_volts(((uint32_t)s_frame.vpp_lsb * 3300u) / 4096u, s_text[F_VPP]);

    /* 输入频率：走 TIM3 输入捕获（比较器那条路），与上游一致。
     * 测不出就摆 `-----`，**不要拿 0 顶替** —— 那是"信号是直流"，另一回事。 */
    have_hz = LocalFreq_GetHz(&in_hz);
    if (have_hz && in_hz > 0u) {
        fmt_hz(in_hz, s_text[F_IN_FREQ]);
    } else {
        fmt_fixed6(s_text[F_IN_FREQ], "  ----");
    }

    /* 函数发生器那三项 */
    fmt_hz(LocalGen_GetHz(), s_text[F_OUT_FREQ]);
    fmt_duty(LocalGen_GetDutyPermille(), s_text[F_OUT_DUTY]);
}

/* ── 对外接口 ─────────────────────────────────────────────────── */

bool Display_IsReady(void)
{
    return s_ready && !s_fault;
}

void Display_Init(void)
{
    s_ready = false;
    s_fault = false;
    s_phase = PH_IDLE;

    /* ⚠ **照搬上游的序列：延时 → 初始化 → 延时 → 再初始化一遍。**
     *
     * 上游在这块板子上实测出来「第一次初始化可能是无效的」，原因是面板自己的
     * 上电复位与 MCU 的不一致（那块 8 pin 软排线的接触也未必可靠）。
     * 我在第一版里自作主张省掉了这次重复与前置延时 —— 那是没有依据的乐观。
     *
     * 真机实测：按一下复位键就有画面，正是这个现象。
     *
     * 代价：开机多约 2.4 s 不响应。确认稳定后可以一层层减
     * （先减到一次，再减延时），**但每减一次都要上板验**。 */
    HAL_Delay(1000);
    TFT_Init();
    HAL_Delay(1000);
    TFT_Init();

    if (TFT_IoErr() != 0u) {
        s_fault = true;
        return;
    }

    draw_static();
    refresh_text();

    /* 数值栏的初值先画一遍 —— 开机就该看到一个完整的界面，
     * 而不是几个空白的黄块。这一次是开机，可以连画。 */
    {
        uint8_t f;
        uint8_t p;
        for (f = 0u; f < (uint8_t)F_COUNT; f++) {
            for (p = 0u; p < (uint8_t)field_len(f); p++) {
                draw_field_char(f, p);
            }
        }
    }

    if (TFT_IoErr() != 0u) {
        s_fault = true;
        return;
    }

    s_ready = true;
}

void Display_Capture(const acq_t *a)
{
    if (!Display_IsReady()) {
        return;
    }

    /* 当场建快照 —— 之后主机再 ARM、环被写成什么，都与这一帧无关。 */
    wave_build(a, &s_frame);
    if (!s_frame.valid) {
        return;
    }

    refresh_text();

    s_col = 0u;
    s_field = 0u;
    s_pos = 0u;
    s_phase = PH_COLUMNS;
}

void Display_Poll(bool acq_armed)
{
    (void)acq_armed;   /* 当前两档预算相同；留这个参数是为了将来调整时有落点 */

    if (!Display_IsReady()) {
        return;
    }

    /* 屏幕上一次传输失败之后就别再画了。**不要**因此停掉别的东西 ——
     * SPI 卡住只说明屏幕这条支路坏了。 */
    if (TFT_IoErr() != 0u) {
        s_fault = true;
        s_phase = PH_IDLE;
        return;
    }

    switch (s_phase) {
    case PH_COLUMNS: {
        uint32_t n = 0u;
        while (n < COLS_PER_TICK && s_col < WAVE_COLUMNS) {
            draw_column(s_col);
            s_col++;
            n++;
        }
        if (s_col >= WAVE_COLUMNS) {
            s_phase = PH_TEXT;
        }
        break;
    }

    case PH_TEXT:
        if (s_field >= (uint8_t)F_COUNT) {
            s_phase = PH_IDLE;
            break;
        }
        if (s_pos >= (uint8_t)field_len(s_field)) {
            s_field++;
            s_pos = 0u;
            break;
        }
        draw_field_char(s_field, s_pos);
        s_pos++;
        break;

    case PH_IDLE:
    default:
        break;
    }
}
