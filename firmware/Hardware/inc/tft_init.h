/*
 * Upstream notice -- this file comes from the "STM32-Oscilloscope" project
 * (simple digital oscilloscope firmware), redistributed here under the
 * MulanPSL-2.0 license (Mulan Permissive Software License, version 2).
 *
 * License text:  http://license.coscl.org.cn/MulanPSL2
 * Provenance:    see NOTICE.md, section 2.
 *
 * Modified for this project:
 *   1. this notice;
 *   2. finite SPI timeouts in the write primitives (was `HAL_MAX_DELAY`,
 *      which turns a stuck bus into a hung main loop);
 *   3. added `TFT_IoErr()` and `TFT_Blit()` -- see their comments in the .c.
 */

#ifndef TFT_INIT_H
#define TFT_INIT_H

#include "main.h"

#define USE_HORIZONTAL 2  //���ú�������������ʾ 0��1Ϊ���� 2��3Ϊ����

#if USE_HORIZONTAL==0||USE_HORIZONTAL==1
#define LCD_W 128
#define LCD_H 160

#else
#define LCD_W 160
#define LCD_H 128
#endif

void TFT_WR_DATA8(uint8_t data);
void TFT_WR_DATA(uint16_t data);
void TFT_WR_REG(uint8_t reg);
void TFT_Address_Set(uint16_t x1,uint16_t y1,uint16_t x2,uint16_t y2);
void TFT_Init(void);

/* 往一个矩形窗口里连续写像素（RGB565，小端）。
 * `nbytes` 必须等于窗口像素数 × 2 —— 见 .c 里的说明。 */
void TFT_Blit(uint16_t x1, uint16_t y1, uint16_t x2, uint16_t y2,
              const uint8_t *buf, uint32_t nbytes);

/* 累计的 SPI 传输失败次数（粘滞）。非零即说明屏幕这条链路已不可信，
 * 调用方应停止绘制 —— 但**不要**因此停掉别的功能。 */
uint16_t TFT_IoErr(void);

#endif
