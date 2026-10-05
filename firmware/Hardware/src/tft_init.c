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
 *   2. the three write primitives now use a **finite** SPI timeout instead of
 *      `HAL_MAX_DELAY`, and count failures in a sticky counter readable via
 *      `TFT_IoErr()` (a stuck SPI bus used to hang the whole main loop, which
 *      is strictly worse than a blank screen: the device stops answering PING);
 *   3. added `TFT_Blit()` -- a windowed bulk write. See its comment for why the
 *      per-pixel path is unusable for the waveform redraw.
 */

#include "tft_init.h"
#include "spi.h"

/* 一次 SPI 传输的上限。
 *
 * 正常时间：2 字节 @ 18 MHz ≈ 0.9 µs。5 ms 是它的五千倍 —— 走到这个超时
 * 一定是硬件出了问题（SCK 被拉死、DC/CS 接错），不是"慢"。
 *
 * 为什么不能用 `HAL_MAX_DELAY`：那会让主循环**永久停住**，连 PING 都不回。
 * 屏幕是可选件，不该有把整台仪器拖死的权力。 */
#define TFT_SPI_TIMEOUT_MS 5u

/* 累计的 SPI 传输失败次数。粘滞 —— 一旦非零就说明这条链路不可信了。 */
static uint16_t s_io_err = 0u;

uint16_t TFT_IoErr(void)
{
    return s_io_err;
}

/* 三个写原语共用的收尾：超时就记账，但不阻断调用方。 */
static void note_transmit(HAL_StatusTypeDef st)
{
    if (st != HAL_OK) {
        if (s_io_err != 0xFFFFu) {
            s_io_err++;
        }
    }
}

/*
*   函数内容：TFT发送单个字节数据
*   函数参数：无
*   返回值：无
*/
void TFT_WR_DATA8(uint8_t data)
{
    HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_RESET);   //拉低片选信号

		note_transmit(HAL_SPI_Transmit(&hspi1,&data,1,TFT_SPI_TIMEOUT_MS));

    HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_SET);     //拉高片选信号
}

/*
*   函数内容：TFT发送2个字节数据
*   函数参数：无
*   返回值：无
*/
void TFT_WR_DATA(uint16_t data)
{
	uint8_t sendData[2] = {0};
	sendData[0] = (data>>8);
	sendData[1] = (data);
    
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_RESET);   //拉低片选信号

	note_transmit(HAL_SPI_Transmit(&hspi1,sendData,2,TFT_SPI_TIMEOUT_MS));

	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_SET);     //拉高片选信号
}

/*
*   函数内容：TFT发送命令数据
*   函数参数：无
*   返回值：无
*/
void TFT_WR_REG(uint8_t reg)
{
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_6,GPIO_PIN_RESET);   //拉低命令信号
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_RESET);   //拉低片选信号

	note_transmit(HAL_SPI_Transmit(&hspi1,&reg,1,TFT_SPI_TIMEOUT_MS));

	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_6,GPIO_PIN_SET);     //拉高命令信号
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_SET);     //拉高片选信号
}

void TFT_Address_Set(uint16_t x1,uint16_t y1,uint16_t x2,uint16_t y2)
{
	TFT_WR_REG(0x2a);//列地址设置
	TFT_WR_DATA(x1);
	TFT_WR_DATA(x2);
	TFT_WR_REG(0x2b);//行地址设置
	TFT_WR_DATA(y1);
	TFT_WR_DATA(y2);
	TFT_WR_REG(0x2c);//储存器写
}

/*
*   函数内容：往一个矩形窗口里连续写像素（RGB565，小端）
*   函数参数：窗口坐标 + 数据缓冲 + 字节数
*   返回值：无
*
*   为什么需要它：`TFT_WR_DATA()` 每传 2 个字节就要拉一次片选、走一遍
*   `HAL_SPI_Transmit`。画一列 51 个像素 = 1 次 `TFT_Address_Set`（7 次调用）
*   + 51 次 `TFT_WR_DATA` = **58 次 SPI 调用**；100 列一帧就是 5800 次，
*   按每次 2 µs 量级算是 12 ms 以上，而主循环在采集期间的单轮预算只有
*   2.39 ms（半个采集环的发布周期，见 `Hardware/src/adc_dma.c`）。
*
*   这里把整段像素攒成一个缓冲、**一次**传出去：一列降到 2 次调用，
*   100 列一帧约 0.5 ms。
*
*   ⚠ `nbytes` 必须等于窗口像素数 × 2。多写会绕回窗口开头（ST7735 的
*   地址计数器到边界后不自动换行，会从窗口起点重来），画出一片错位。
*/
void TFT_Blit(uint16_t x1, uint16_t y1, uint16_t x2, uint16_t y2,
              const uint8_t *buf, uint32_t nbytes)
{
	/* 超时随长度放大：102 B 只要 45 µs，而整屏 20480 B 要 9.1 ms ——
	 * 用固定的 5 ms 会把一次**正常**的整屏填充误判成故障。
	 * 18 MHz 下 1000 B ≈ 0.44 ms，所以每 1000 B 加 1 ms 富余。 */
	uint32_t timeout_ms = TFT_SPI_TIMEOUT_MS + nbytes / 1000u;

	if (buf == NULL || nbytes == 0u || nbytes > 0xFFFFu) {
		return;
	}

	TFT_Address_Set(x1, y1, x2, y2);

	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_6,GPIO_PIN_SET);     //数据模式
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_RESET);   //拉低片选

	note_transmit(HAL_SPI_Transmit(&hspi1, (uint8_t *)(void *)buf,
	                               (uint16_t)nbytes, timeout_ms));

	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_7,GPIO_PIN_SET);     //拉高片选
}

void TFT_Init(void)
{
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_5,GPIO_PIN_RESET);  	//复位
	HAL_Delay(100);
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_5,GPIO_PIN_SET);     //复位完成
	HAL_Delay(100);
	
	HAL_GPIO_WritePin(GPIOB,GPIO_PIN_8,GPIO_PIN_SET);     //打开背光
	HAL_Delay(100);
    
	//************* Start Initial Sequence **********//
	TFT_WR_REG(0x11); //Sleep out 
	HAL_Delay(120);              //Delay 120ms 
	//------------------------------------ST7735S Frame Rate-----------------------------------------// 
	TFT_WR_REG(0xB1); 
	TFT_WR_DATA8(0x05); 
	TFT_WR_DATA8(0x3C); 
	TFT_WR_DATA8(0x3C); 
	TFT_WR_REG(0xB2); 
	TFT_WR_DATA8(0x05);
	TFT_WR_DATA8(0x3C); 
	TFT_WR_DATA8(0x3C); 
	TFT_WR_REG(0xB3); 
	TFT_WR_DATA8(0x05); 
	TFT_WR_DATA8(0x3C); 
	TFT_WR_DATA8(0x3C); 
	TFT_WR_DATA8(0x05); 
	TFT_WR_DATA8(0x3C); 
	TFT_WR_DATA8(0x3C); 
	//------------------------------------End ST7735S Frame Rate---------------------------------// 
	TFT_WR_REG(0xB4); //Dot inversion 
	TFT_WR_DATA8(0x03); 
	//------------------------------------ST7735S Power Sequence---------------------------------// 
	TFT_WR_REG(0xC0); 
	TFT_WR_DATA8(0x28); 
	TFT_WR_DATA8(0x08); 
	TFT_WR_DATA8(0x04); 
	TFT_WR_REG(0xC1); 
	TFT_WR_DATA8(0XC0); 
	TFT_WR_REG(0xC2); 
	TFT_WR_DATA8(0x0D); 
	TFT_WR_DATA8(0x00); 
	TFT_WR_REG(0xC3); 
	TFT_WR_DATA8(0x8D); 
	TFT_WR_DATA8(0x2A); 
	TFT_WR_REG(0xC4); 
	TFT_WR_DATA8(0x8D); 
	TFT_WR_DATA8(0xEE); 
	//---------------------------------End ST7735S Power Sequence-------------------------------------// 
	TFT_WR_REG(0xC5); //VCOM 
	TFT_WR_DATA8(0x1A); 
	TFT_WR_REG(0x36); //MX, MY, RGB mode 
	if(USE_HORIZONTAL==0){
        TFT_WR_DATA8(0x00);
    }
	else if(USE_HORIZONTAL==1){
        TFT_WR_DATA8(0xC0);
    }
	else if(USE_HORIZONTAL==2){
        TFT_WR_DATA8(0x70);
    }
	else {
        TFT_WR_DATA8(0xA0); 
    }
	//------------------------------------ST7735S Gamma Sequence---------------------------------// 
	TFT_WR_REG(0xE0); 
	TFT_WR_DATA8(0x04); 
	TFT_WR_DATA8(0x22); 
	TFT_WR_DATA8(0x07); 
	TFT_WR_DATA8(0x0A); 
	TFT_WR_DATA8(0x2E); 
	TFT_WR_DATA8(0x30); 
	TFT_WR_DATA8(0x25); 
	TFT_WR_DATA8(0x2A); 
	TFT_WR_DATA8(0x28); 
	TFT_WR_DATA8(0x26); 
	TFT_WR_DATA8(0x2E); 
	TFT_WR_DATA8(0x3A); 
	TFT_WR_DATA8(0x00); 
	TFT_WR_DATA8(0x01); 
	TFT_WR_DATA8(0x03); 
	TFT_WR_DATA8(0x13); 
	TFT_WR_REG(0xE1); 
	TFT_WR_DATA8(0x04); 
	TFT_WR_DATA8(0x16); 
	TFT_WR_DATA8(0x06); 
	TFT_WR_DATA8(0x0D); 
	TFT_WR_DATA8(0x2D); 
	TFT_WR_DATA8(0x26); 
	TFT_WR_DATA8(0x23); 
	TFT_WR_DATA8(0x27); 
	TFT_WR_DATA8(0x27); 
	TFT_WR_DATA8(0x25); 
	TFT_WR_DATA8(0x2D); 
	TFT_WR_DATA8(0x3B); 
	TFT_WR_DATA8(0x00); 
	TFT_WR_DATA8(0x01); 
	TFT_WR_DATA8(0x04); 
	TFT_WR_DATA8(0x13); 
	//------------------------------------End ST7735S Gamma Sequence-----------------------------// 
	TFT_WR_REG(0x3A); //65k mode 
	TFT_WR_DATA8(0x05); 
	TFT_WR_REG(0x29); //Display on   
}
