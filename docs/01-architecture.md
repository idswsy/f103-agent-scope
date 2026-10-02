# 01 · 系统架构

## 分层总览

```
┌─────────────────────────────────────────────────────────────────────┐
│  L5  Agent 层           Claude / 任意 LLM                           │
│                          ↕ MCP (JSON-RPC over stdio)                │
│  L4  MCP Server        14 个粗粒度工具，schema 手写（待改 schemars）│
│                          ↕ 直接调用 scope-core 的 async API         │
│  L3  命令层 scope-core  CommandBus：编码 / seq / 超时重试 / 状态缓存│
│                          ↕ DevicePort (trait)                       │
│  L2  传输层             UART(tokio-serial) │ sim │ tcp(可选)        │
│                          ↕ AA55 帧 / CRC16 / 分片                   │
│  L1  设备层             STM32F103C8T6 固件                          │
└─────────────────────────────────────────────────────────────────────┘
```

**解耦点只有一个**：`DevicePort`。L3 以上完全不关心背后是真硬件、模拟器还是 TCP。

---

## F103 固件内部分层

```
firmware/
├─ App/            应用层（与硬件无关的逻辑，可 PC 上单元测试）
│   ├─ proto_task       帧解析状态机 → 命令分发 → 响应编码
│   ├─ acq              采集状态机（IDLE/ARMED/DONE/STREAMING/FAULT）
│   ├─ trigger          触发搜索（迟滞电平比较）
│   ├─ measure          定点测量（Vpp/频率/占空比/RMS）
│   ├─ i2c_decode       数字通路 I2C 解码器
│   └─ ui               屏幕 / 编码器 / 按键
├─ Hardware/       硬件抽象（唯一碰寄存器的地方）
│   ├─ adc_dma          TIM3_TRGO → 双 ADC → DMA1Ch1 → 8KB 环
│   ├─ tim_capture      LM393 → TIM 输入捕获（13.9 ns 时间戳）
│   ├─ uart / usb_cdc   链路
│   ├─ tft_st7735       1.8 寸屏
│   └─ encoder          条 EC11
└─ main.c
```

**规则**：`App/` 不许 `#include` 任何 HAL / 寄存器头文件。这样 `measure.c`、`i2c_decode.c`、`proto_task.c` 可以在 PC 上用 gcc 加 mock 编译测试 —— 这是本项目在没有硬件时也能推进的关键。

---

## 数据通路：为什么是双通路

这是本项目最重要的架构决策，来自一次定量分析（见 [`04-performance.md`](04-performance.md)）。

### 硬约束

I2C 解码要求：**SCL 与 SDA 必须同时刻采样**，且每比特至少 2 个采样点才能判定 START/STOP（判据 `S ≥ 2/tHIGH`）。

| I2C 速率 | tHIGH | 需要采样率 | F103 单 ADC 857 kSPS 够吗 |
|---|---|---|---|
| 100 kHz | 4.0 µs | 0.5 MSPS | ✅ 够（8.6 点/bit，2× 裕量） |
| 400 kHz | 0.6 µs | 3.33 MSPS | ❌ 差 3.9 倍 |
| 1 MHz | 0.26 µs | 7.69 MSPS | ❌ 差 9 倍 |

而且 400 kHz 时 `857k/400k ≈ 15/7`，采样相位只有 **15 个离散值**（间隔 1/15 个比特周期）；两颗晶振 40 ppm 差会让解码**以 ~62 ms 为周期时好时坏地闪** —— 这不是 bug，是采样相位拍频，软件补不了。

### 解法：把「解码」和「看波形」拆开

```
                    ┌──────────────────────────────────────┐
   BNC ──► TL072 ──►│ ADC1/ADC2 (同步模式, 857 kSPS/路)     │──► 模拟通路
                    │   → 电平裕量 / 上拉强度 / 振铃 / 边沿形状 │
                    └──────────────────────────────────────┘
                    ┌──────────────────────────────────────┐
   输入 ──► LM393 ──►│ TIM 输入捕获 (72 MHz, 13.9 ns)        │──► 协议通路
                    │   → START/地址/ACK/数据/STOP 解码      │
                    └──────────────────────────────────────┘
```

**板上现成的 LM393 滞回比较器**（原设计只用来测频率）接输入捕获后：

- 边沿时间戳精度 **13.9 ns**（等效 72 MSPS）
- 边沿驱动解码 → **免疫混叠、免疫采样相位问题、免疫时钟拉伸**
- 100 kHz / 400 kHz / 1 MHz **全部 100% 可靠**

两条通路共用一个触发，时间对齐后同时呈现「协议对不对」+「信号好不好」。

> **代价**：数字通路只有 0/1 二值，丢失电平裕量信息 —— 正好由 ADC 通路补上。这就是为什么两条都要。

---

## 一次典型采集的时序

```
主机                                    设备
 │  SET_SAMPLE_RATE(857143)               │  量化到 TIM3 ARR=83 → 857142 Hz
 │ ◄──── ACTUAL(857142)  ← 必须回显实际值  │
 │  SET_TRIGGER(mode=normal, edge=↑, lvl) │
 │  ARM ─────────────────────────────────►│  状态 IDLE → ARMED
 │                                        │  TIM3_TRGO → ADC → DMA 循环写入 8KB 环
 │                                        │  HT/TC 中断每 2048 点发布一个半区
 │                                        │  主循环在已发布半区上搜索触发点（迟滞）
 │                                        │  找到 T → 继续采 T+post+512 余量 → 停 DMA
 │  ◄──── EVENT_TRIGGER{trigger_index}    │  状态 → DONE
 │  READ_BUFFER(cap_id, 0, 1024, RAW16) ─►│
 │  ◄──── 1024 样点 + 12B 分片头           │
 │  READ_BUFFER(cap_id, 1024, 1024, ...)─►│
 │  ◄──── ...                             │
 │  上位机侧：minmax 降采样 → 存 capture store → Agent 只拿到统计量+预览
```

---

## 命令语义三端一致

同一条命令在三端各是什么：

| 命令 | Agent 侧（MCP 工具） | 上位机侧（Rust） | 固件侧（C） |
|---|---|---|---|
| `SET_SAMPLE_RATE` | `scope_configure(sample_rate_hz=…)` | 编码 → `0x0201`，**以回显值建时间轴** | 量化到 TIM 实际分频 → 回显 `actual_hz` |
| `READ_BUFFER` | `scope_read_waveform(capture_id, …)` | 分片循环 + 超时重拉 | 按绝对样点号切片回发 |
| `MEASURE` | `scope_measure(capture_id, metrics=[…])` | 默认主机侧算（精度最高） | 可选，流模式快算 |

**关键纪律**：
- 主机的时间轴**永远**由 `(capture_id, start_sample, actual_rate_hz)` 推导，**绝不用分片到达时刻**。
- 一律 `SET_*` 回显实际生效值，主机以回显为准。
- 控制链路**永不出现 `f32`**（F103 无 FPU，软浮点慢且线上格式易被两端误解）—— 电平用 ADC LSB 整数，测量结果用带缩放字段名的定点（`mean_x256`、`duty_x10000`）。

---

## 协议真相源的跨语言问题

固件 C、上位机 Rust，**编译器帮不上忙**。所以：

```
proto/
├─ protocol.h          ← C 端定义（#pragma pack + static_assert 锁偏移）
├─ protocol.c          ← 纯 C 编解码，不依赖 HAL，PC 可 gcc 编译
└─ tests/vectors.json  ← 唯一契约：黄金测试向量
```

同一份 `vectors.json` 被两端执行：

| 执行方 | 命令 | CI |
|---|---|---|
| C 端 | `make -C proto test`（gcc + 纯 C） | ✅ |
| Rust 端 | `cargo test -p scope-proto` | ✅ |

任何一端改了编码但没改另一处 → **测试立刻红**。

`docs/03-protocol.md` + `protocol.h` + Rust 类型 + `vectors.json` 四处必须同一次提交更新 —— 这是硬纪律，写进 CI 门禁。

---

## 无硬件开发的支撑

| 组件 | 作用 |
|---|---|
| `host/crates/sim` | 实现同一 `DevicePort`，内含 F103 真实档位表、ARM/触发/环缓冲语义、按波特率模拟的分片节奏 |
| 故障注入 | `drop_every_n_frames` / `crc_err_every_n` / `latency_spike_ms` / `no_trigger` |
| `MockDevice` | 进程内，供 `cargo test` 与 MCP 开发 |
| C 端纯逻辑测试 | `App/` 层不依赖 HAL → PC 上 gcc 编译跑同一批黄金向量 |

三种传输（UART / sim / tcp）即插即用，切到 `transport='sim'` 后 **Agent 逻辑零改动**。
