# firmware/ —— STM32F103C8T6 固件

> **当前状态**：目录骨架 + 分层契约已就位，**业务代码待 P1 实现**。
> 这里不伪造"已经写好的固件" —— 下面写清楚了每一层该放什么、以及为什么这么分。

语言：**C11**。库：HAL 或标准外设库（见下方「库的选择」）。

---

## 分层结构

```
firmware/
├─ App/             应用层 —— 与硬件无关的逻辑，可在 PC 上单测
│   ├─ proto_task.c       帧解析状态机 → 命令分发 → 响应编码
│   ├─ acq.c              采集状态机 IDLE/ARMED/DONE/STREAMING/FAULT
│   ├─ trigger.c          触发搜索（迟滞状态机）
│   ├─ measure.c          定点测量（Vpp/频率/占空比/RMS）
│   ├─ i2c_decode.c       数字通路 I2C 解码器
│   └─ ui.c               屏幕 / 编码器 / 按键
│
├─ Hardware/        硬件抽象层 —— 唯一允许碰寄存器的地方
│   ├─ adc_dma.c          TIM3_TRGO → ADC1/ADC2 → DMA1Ch1 → 8KB 环
│   ├─ tim_capture.c      LM393 → PA6 输入捕获（13.9 ns 时间戳）
│   ├─ link_uart.c        USART1（PA9/PA10）
│   ├─ link_usbcdc.c      USB CDC（PA11/PA12）
│   ├─ tft_st7735.c       1.8" 屏
│   └─ encoder.c          EC11 + 3 按键
│
├─ MDK-ARM/         Keil 工程（*.uvprojx）—— **尚未创建，P1 任务**
└─ Core/            CubeMX 生成的时钟树与初始化（若用 HAL）
```

---

## ⭐ 硬纪律：`App/` 不许 `#include` HAL

这是**整个项目最重要的一条工程约束**，理由是现实的：

> 三个人里只有一个人能同时拿到板子。
> 如果 `App/` 依赖 HAL，另外两个人就只能干等 —— 这是项目最常见的死法。

`App/` 层必须能这样测试：

```bash
# 在 PC 上编译 App 层的纯逻辑，不需要板子、不需要 Keil、不需要交叉编译
gcc -std=c11 -I../proto -I App App/proto_task.c App/measure.c App/trigger.c \
    ../proto/protocol.c -o build/app_test
```

**CI 会强制检查这条规矩**：

```bash
grep -rn '#include.*\(stm32\|hal_\|gd32\|HAL\)' firmware/App/ && exit 1 || exit 0
```

违反它 = CI 红。

---

## 库的选择：HAL vs 标准外设库

| | ST HAL | 标准外设库 (SPL) |
|---|---|---|
| CubeMX 支持 | ✅ 可视化配置 | ❌ |
| 代码体积 | 大（20–35 KB） | 小（~10 KB） |
| USB CDC | ✅ 官方例程 | 需移植 |
| **本项目 64 KB Flash 下的余量** | ⚠️ **紧张** | ✅ 宽松 |

> **C8 只有 64 KB Flash。HAL + USB CDC 就要吃掉 20–35 KB。**
> 建议：**优先 LL 库（HAL 的轻量版）或 SPL**；若坚持用 HAL，
> 要么换 **F103CB（128 KB Flash，引脚完全兼容）**，要么严格控制功能规模。
>
> 换 CB 是更省事的路 —— 它是同封装同引脚的，底板不用动。
> 见 [`docs/04-performance.md`](../docs/04-performance.md) §3。

---

## 数据链路选型

核心板**没有板载串口桥**（不是 CH340），Type-C 直连 PA11/PA12。

| 优先级 | 链路 | 引脚 | 需要的额外硬件 |
|---|---|---|---|
| **P1 期** | USART1 | PA9 / PA10 | 一个 USB-TTL 小板 |
| **P2 起** | USB CDC | PA11 / PA12 | 无（板载 Type-C） |
| 🚫 **禁用** | ~~USART2~~ | ~~PA2 / PA3~~ | 硬冲突：PA2 = PWM 输出，PA3 = 模拟输入 |

**无论是哪条，上层都用同一个 `App/proto_task.c`** —— 链路差异只在 `Hardware/link_*.c` 里。

---

## 采样架构（定时器触发 + DMA + 环缓冲）

```
TIM3 (PSC=0, ARR=83, 72MHz) ──TRGO──► ADC1/ADC2 同步转换
                                          │
                                          ▼
                              DMA1 Channel 1（循环模式）
                                          │
                                          ▼
                            uint16_t ring[4096]  (8 KB)
                                          │
                            半传输/全传输中断（每 2048 点发布一个半区）
                                          │
                                          ▼
                    主循环在**已发布**的半区上搜索触发点（迟滞状态机）
                                          │
                          找到 T → 继续采 T+post+512 余量 → 停 DMA
                                          │
                                          ▼
                              EVENT_TRIGGER 上报 → 状态 DONE
```

### 关键数字

| 项 | 值 | 来源 |
|---|---|---|
| 采样率 | **857.143 kSPS** | 12 MHz ADCCLK ÷ 14 周期 |
| 缓冲深度 | 4096 点 / 8 KB | 20 KB SRAM 的现实预算 |
| 块中断频率 | 419 Hz（每 2048 点） | 857143 / 2048 |
| 每样点可用周期 | **84** | 72 MHz / 857 kHz |
| 停 DMA 余量 | +512 样点 | 吸收停驻延迟 |

完整推导见 [`docs/04-performance.md`](../docs/04-performance.md)。

### ⚠️ 三条最容易踩的坑

1. **ADC2 在中容量型号上没有 DMA 请求通道**
   → 双 ADC 模式下必须借 `ADC1_DR` 组合寄存器经 ADC1 的 DMA 搬出，
   且 DMA 必须配 **32-bit 宽度**（低半字 = ADC1，高半字 = ADC2）。
   配成 16-bit 会**静默丢掉一半**，速率直接腰斩。

2. **禁止逐样本中断**
   857 kSPS 下每样本只有 84 个周期，逐样本中断会吃掉全部 CPU。
   必须 DMA + 块中断。

3. **绝不读正在被 DMA 写的半区**
   所有权规则：只有 HT/TC 中断发布过的半区才可以读、算 CRC、打包发送。
   宁慢勿乱。

---

## 触发搜索：必须是状态机

**不能**写成"相邻两点跨越整条迟滞带"——那样缓变信号永远触发不了：

```
1 kHz 正弦 @ 857 kSPS → 每样点变化约 14 LSB
迟滞带宽 ±16 LSB → 32 LSB
一次跳变跨不过去 → 永远不触发
```

正确语义（施密特触发）：

```
1. 信号必须先跌到 lo 以下置位（armed）
2. 之后升到 hi 以上才算一次上升沿
3. 落在带内的值既不置位也不触发 —— 这就是迟滞抑制抖动的原理
```

参考实现见 `host/crates/sim/src/device.rs` 的 `find_trigger()`，
那里有完整的边界测试用例（缓变斜坡 / 带内抖动 / 对称下降沿 / 从不跨越）。

---

## 编译与烧录

> **工程文件还没有。** `firmware/MDK-ARM/` 目前是空目录 ——
> Keil 工程（`.uvprojx`）要在 **P1 阶段**创建。
> 在那之前，`App/` 与 `Hardware/` 的代码可以先用 PC 上的 gcc 编译测试（见上面的分层纪律）。

```bash
# P1 建好工程之后：
# Keil MDK：打开 firmware/MDK-ARM/<工程名>.uvprojx，F7 编译，DAP-Link 下载

# 命令行（若另建 Makefile + arm-none-eabi-gcc）
make -C firmware && make -C firmware flash
```

**DAP-Link 接线**（核心板上的 SWD 四脚）：

```
DAP-Link 3V3  ──  核心板 3V3
DAP-Link DIO  ──  核心板 DIO
DAP-Link CLK  ──  核心板 CLK
DAP-Link GND  ──  核心板 GND
```

> Win11 下装了 ST-Link 驱动可能导致 DAPLink 识别不到（官方文档提到过）。`【原文】`

---

## 上游参考

立创上游工程是 GD32E23x 标准外设库的 C 代码，另有社区适配的 **STM32 分支**：

- <https://gitee.com/chen11232/GD32E230-Oscilloscope>
  分支：`master`（GD32E230）/ `CW32版本` / **`STM32版本`** / `MSPG3507版本`
- 上游代码许可：**MulanPSL-2.0**；硬件许可：GPL-3.0
  引入其代码时必须保留声明，见 [`NOTICE.md`](../NOTICE.md)

**直接复用是允许且推荐的**（这能省掉几周）：屏幕驱动、编码器、按键、
ADC 初始化都可以从上位工程搬，**但采样架构、触发、协议、数字通路解码必须重写** ——
上游只做本机采集显示，没有任何对外命令接口。
