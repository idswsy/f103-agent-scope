/* Hardware/inc/display.h —— 板载 1.8 寸 TFT（ST7735S，160×128）的渲染。
 *
 * ⚠ **已冻结**：本模块不在 Keil 工程内（docs/09 §4.4），文件保留、无编译开关；
 * 恢复时把 `Hardware/src/display.c` 加回 Keil 工程的 `Application/Hardware` 组。
 *
 * # 刷新模型：被动的
 *
 * 采集完成后刷一帧，空闲时画面静止。设备**不会**自己去抢 ADC ——
 * 采集的所有权始终在协议手里（`App/acq.c` 的状态机）。
 *
 * # 为什么不用上游那套 `TFT_StaticUI` / `TFT_ShowUI` / `drawCurve`
 *
 * 1. 它们的静态版里有**函数发生器面板**（PWM 输出状态/频率/占空比），
 *    这台仪器不做信号源，那一块是上游当"简易信号源"时的产物
 * 2. `drawCurve` 的每列要画一条折线并擦 51 个点，**每列约 138 次 SPI 调用** ——
 *    100 列一帧就是 12 ms 以上，而采集期间主循环的单轮预算只有 2.39 ms
 * 3. `drawCurve` 的 `lastX/lastY/firstPoint` 是文件级 static 且**没有重置接口**，
 *    与"每帧整幅重画"的模型不合
 *
 * 这里改成整列批量写（见 `TFT_Blit`），每列 2 次 SPI 调用、一帧约 0.5 ms。
 * 上游那几个函数因此成了死代码，由链接器按段删除 —— 它们仍在 `tft.c` 里留档。
 */

#ifndef HARDWARE_DISPLAY_H
#define HARDWARE_DISPLAY_H

#include <stdbool.h>
#include <stdint.h>

#include "acq.h"

/* 屏幕是否可用。初始化时 SPI 有故障则为假，此后所有绘制调用都是空操作
 * —— 屏幕坏了不该影响协议链路。 */
bool Display_IsReady(void);

/* 上电调用一次：初始化面板、清屏、画静态框架。
 *
 * 内部有约 420 ms 的阻塞延时（面板复位与寄存器序列），所以**只在启动时调**。 */
void Display_Init(void);

/* 采集完成（`acq_poll` 返回 `ACQ_EV_TRIGGERED`）时调用。
 *
 * 它**当场**把采集窗压成一帧快照，此后绘制只读这份快照 ——
 * 主机随后什么时候再 ARM、环里被写成什么都不再影响这一帧。
 * `a` 未就绪（`capture_ready` 为假）时什么也不做，保留上一帧。 */
void Display_Capture(const acq_t *a);

/* 主循环每次迭代调用一次：按预算推进绘制。
 *
 * `acq_armed` 只用于选预算档位 —— 采集期间半个环每 2.39 ms 发布一次，
 * 主循环单轮必须显著小于它，否则 `acq_poll` 会漏掉已发布的半区。
 *
 * ⚠ 调用点必须在 `proto_task_poll()` **之后** —— 串口的收包环只有 1 KB
 * （约 11 ms），任何时候都不该让屏幕排在它前面。 */
void Display_Poll(bool acq_armed);

#endif /* HARDWARE_DISPLAY_H */
