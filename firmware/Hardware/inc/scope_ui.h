/*
 * Upstream notice -- this file carries the local display / keypad application
 * logic of the "STM32-Oscilloscope" project, redistributed here under the
 * MulanPSL-2.0 license. See NOTICE.md, section 2.
 *
 * License text:  http://license.coscl.org.cn/MulanPSL2
 *
 * Modified for this project:
 *   - moved out of Core/Src/{main,gpio,tim,adc}.c and Core/Inc/main.h
 *   - wrapped in `#if SCOPE_LOCAL_UI` (default 0 -- not built)
 *   - the upstream Chinese comments in these blocks were ALREADY corrupted
 *     (GBK bytes read as UTF-8; the original text is unrecoverable), so the
 *     struct field comments below were reconstructed from how the code uses
 *     each field. Field names and behaviour are unchanged.
 */

#ifndef SCOPE_UI_H
#define SCOPE_UI_H

/* 本机 UI 开关 —— **默认 0（不编译）**。
 *
 * 上游那套「本机采集 + TFT 显示 + 按键」的完整应用。路线图规定 P1 阶段先
 * 不碰屏幕（先用串口把数据链路打通），所以默认关掉；代码留着是为了 P4 接
 * 屏幕时能直接用，而 CI 会带 `-DSCOPE_LOCAL_UI=1` 做语法检查，
 * **保证它不会腐烂**。 */
#ifndef SCOPE_LOCAL_UI
#define SCOPE_LOCAL_UI 0
#endif

#include "main.h"
#include <stdint.h>

/* 上游一帧的样点数（写死 300）。与我们自己的 4096 点采集**不是一回事**。 */
#define SCOPE_UI_FRAME_SAMPLES 300u

/* 上游本机模式的状态。
 *
 * ⚠ **这个结构体在 `SCOPE_LOCAL_UI` 之外也无条件定义** —— 因为上游的
 *   `tft.h` 里 `TFT_ShowUI(volatile const struct Oscilloscope *)` 要用它。
 *   把定义关掉的话，那个原型就成了不完整类型。结构体本身不占空间。
 *
 * 字段注释是**重建**的，不是原文 —— 见文件头说明。 */
struct Oscilloscope {
    uint8_t  showbit;        /* 一帧采满的标志：DMA 完成回调置位，主循环消费 */
    uint8_t  keyValue;       /* 当前按键事件（KEY1..3 / KEYA / KEYB / KEYD / NoPRESS） */
    uint8_t  ouptputbit;     /* 函数发生器输出开关（KEY2 切换）—— 原拼写如此 */
    uint16_t outputFreq;     /* 函数发生器频率（Hz） */
    uint16_t pwmOut;         /* PWM 比较值（占空比） */
    uint32_t sampletime;     /* ADC 采样时间档（单位为 ADC 周期） */
    uint32_t timerPeriod;    /* 函数发生器的定时器周期（tick） */
    __IO uint32_t gatherFreq;/* 测到的信号频率（TIM3 输入捕获，整定到 1 kHz 倍数） */
    float    vpp;            /* 峰峰值（V） */
    float    voltageValue[SCOPE_UI_FRAME_SAMPLES]; /* 一帧的电压值（V） */
};

#if SCOPE_LOCAL_UI

extern volatile struct Oscilloscope oscilloscope;
extern uint16_t adc_value[SCOPE_UI_FRAME_SAMPLES];

void Init_Oscilloscope(volatile struct Oscilloscope *value);

/* 在 MX_*_Init() 之后调用一次：启动 TIM3 输入捕获、初始化 TFT、启动 ADC DMA。 */
void ScopeUi_Setup(void);

/* 主循环体（原来直接写在 main() 的 while(1) 里）。 */
void ScopeUi_Loop(void);

/* 按键与指示（原 gpio.c） */
void Key_Sacnf(volatile struct Oscilloscope *value);
void Key_Handle(volatile struct Oscilloscope *value);
void KEYD_SCAN(volatile struct Oscilloscope *value);
void Open_Led(uint8_t value);
void CLose_LED(uint8_t value);
void Toggle_Led(uint8_t value);

/* 测频（原 tim.c） */
void Freq_calibration(__IO uint32_t *freq);

#endif /* SCOPE_LOCAL_UI */
#endif /* SCOPE_UI_H */
