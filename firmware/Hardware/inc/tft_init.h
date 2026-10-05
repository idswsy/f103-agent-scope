/*
 * Upstream notice -- this file comes from the "STM32-Oscilloscope" project
 * (simple digital oscilloscope firmware), redistributed here under the
 * MulanPSL-2.0 license (Mulan Permissive Software License, version 2).
 *
 * License text:  http://license.coscl.org.cn/MulanPSL2
 * Provenance:    see NOTICE.md, section 2.
 *
 * Modified for this project: only this notice was added.
 */

#ifndef TFT_INIT_H
#define TFT_INIT_H

#include "main.h"

#define USE_HORIZONTAL 2  //设置横屏或者竖屏显示 0或1为竖屏 2或3为横屏

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

#endif
