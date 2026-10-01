# F103 Agent Scope

> 一台**可被 AI Agent 直接操控**的数字示波器 / I2C 总线分析仪。
> 硬件 = 立创开源《简易数字示波器设计（入门版）》插件底板 + 立创·**地阔星** `LCKFB-DKX-STM32F103C8T6` 核心板。

```
┌──────────────────────────────────────────────────────────────┐
│  AI Agent (Claude / 任意 LLM)                                 │
│      │ MCP (JSON-RPC)                                        │
│  ┌───▼─────────┐                                             │
│  │ MCP Server  │  ← 工具 schema 由 Rust 类型自动生成           │
│  └───┬─────────┘                                             │
│  ┌───▼─────────┐                                             │
│  │ 命令层 core │  CommandBus：编码 / seq / 超时重试 / 状态缓存 │
│  └───┬─────────┘                                             │
│      │ DevicePort (trait)                                    │
│  ┌───┴────┬──────────┬──────────┐                            │
│ 传输:UART  USB CDC   模拟器 sim                             │
└──────┬───────────────────────────────────────────────────────┘
       │  AA55 帧 / CRC16 / 分片
┌──────▼───────────────────────────────────────────────────────┐
│  STM32F103C8T6 固件                                           │
│   ├─ 模拟通路：TIM3_TRGO → ADC1/ADC2 同步 → DMA1Ch1 → 8KB 环   │
│   └─ 协议通路：LM393 滞回比较器 → TIM 输入捕获 (13.9ns)        │
└──────────────────────────────────────────────────────────────┘
```

---

## 这个项目在解决什么

学长的原始方案（[`docs/00-origin.md`](docs/00-origin.md)）指向一台 **F407** 示波器：三 ADC 7.2MSPS、以太网传波形、FSMC 深存储。
本项目的硬件换成了**立创开源《简易数字示波器设计（入门版）》插件板 + STM32F103C8T6 小蓝板**，物理条件整体降级，四个维度必须重做：传输层、采样率、存储深度、I2C 解码能力。

**但我们找到了一个比"堆采样率"更管用的办法**：板上那颗 **LM393 滞回比较器**原本只用来测频率，把它接到定时器输入捕获后，得到 **72 MHz / 13.9 ns 的边沿时间戳** —— I2C 的 100 kHz / 400 kHz / 1 MHz 三档协议解码全部 100% 可靠，彻底绕开了 857 kSPS ADC 的采样率硬墙。

所以本项目采用**双通路**：

| 通路 | 硬件 | 用途 | 能力 |
|---|---|---|---|
| **协议通路**（权威） | LM393 → TIM 输入捕获 | 解码 I2C 帧、时序、时钟拉伸 | 100k / 400k / 1MHz **全部可靠**，13.9 ns 分辨率 |
| **模拟通路**（差异化） | TL072 → ADC | 电平裕量、上拉强度、振铃、边沿形状 | 100 kHz 保证 / 400 kHz 有条件 / 1 MHz 不支持 |

这是 10 元逻辑分析仪永远给不了、而万元示波器才有的组合：**协议对不对** + **信号好不好**，一次说清。

---

## 性能承诺（诚实版）

见 [`docs/04-performance.md`](docs/04-performance.md)。摘要：

| 指标 | F103 版本能做的 | 明确做不到的 |
|---|---|---|
| 采样率 | 双通道同步 **857 kSPS/通道**；单通道交织 1.71 MSPS（仅显示） | 1 MSPS 与 USB 并存；7.2 MSPS（那是 F407） |
| I2C 解码 | **100k / 400k / 1MHz 全部可靠**（走数字通路） | 不测 I2C 时序合规性（tSU;DAT / tr 的 ns 级验证） |
| 模拟带宽 | ≤ 100 kHz 保证 | > 0.5 MHz |
| 存储深度 | 单次 **4096 点**（8 KB 环） | 深存储、外扩 SRAM（C8 无 FSMC） |
| 上传 | 单次采集 8 KB ≈ 12 ms（USB）/ 91 ms（921600） | 1 MSPS 原始连续流 |
| 测量 | Vpp / 频率 / 占空比 / RMS（定点） | THD / SFDR；12-bit 绝对精度 |

> **一句话定位**：低频 / 电源纹波 / 音频级观察 + I2C·UART·SPI 总线协议调试。
> 不是计量仪器，是**给 AI 用的调试眼睛**。

---

## 仓库结构

```
F103/
├─ docs/          设计文档（架构、协议、硬件、性能边界、路线图、决策记录）
├─ proto/         协议唯一真相源：C 头文件 + 纯 C 编解码 + 双端共用的黄金测试向量
├─ firmware/      STM32F103 固件（C，分层 App/Hardware）
├─ host/          Rust workspace：core / transport / sim / cli / mcp
├─ hardware/      立创底板资料、BOM、改板设计
├─ tools/         独立工具（I2C 解码器、CSV 导出、波形查看）
└─ .github/       CI：C 向量测试 + cargo test
```

**协议真相源策略**：固件用 C、上位机用 Rust，跨语言无法靠编译器保证一致。
所以把 [`proto/tests/vectors.json`](proto/tests/vectors.json) 当作唯一契约 —— 同一份黄金向量由 Rust `#[test]` 和「不依赖 HAL 的纯 C 测试」两端同时执行，漂移在测试期暴露。

---

## 快速开始

> 全部命令都在**仓库根目录**（`F103/`）执行。
> 用 `./host/run.sh` 而不是裸 `cargo` —— 它会自动绕开中文路径导致的链接失败（见 [`docs/07-dev-env.md`](docs/07-dev-env.md) 坑 #1）。

### 路线 A：没有硬件也能开发（推荐先走这条）

```bash
# 1. 协议测试：C 与 Rust 两端跑同一份黄金向量
./proto/run_tests.sh            # C 端，50 项
./host/run.sh test              # Rust 端，61 项

# 2. 对着模拟器抓一次波形
./host/run.sh run -p scope-cli -- sim capture --scenario sine_1k_3v3 -n 1024 -o wave.csv

# 3. 看一眼 I2C 双通道是什么样
./host/run.sh run -p scope-cli -- sim capture --scenario i2c_100k -n 2048 -o i2c.csv

# 4. MCP 工具链自检
./host/run.sh run -p scope-mcp -- --selftest
```

模拟器实现同一套 `DevicePort`，内含 F103 真实档位表、ARM/触发语义、按波特率模拟的分片节奏、以及丢帧/CRC 错/延迟尖峰注入。MCP / CLI 切到 `sim` 后 Agent 逻辑零改动。

### 路线 B：有硬件

```bash
# 先看看板子插在哪个口
./host/run.sh run -p scope-cli -- ports

# 连真机抓波形（注意 --port/--baud 在 serial 之后、子命令之前）
./host/run.sh run -p scope-cli -- serial --port COM3 --baud 921600 capture -n 2048 -o wave.csv
```

固件工程**尚未创建**（P1 任务）—— `firmware/README.md` 写了完整的分层结构与采样架构，
拿到板子后照着建。烧录用 DAP-Link，接线见那份文档。

---

## 团队分工（3 人）

| 角色 | 负责 | 主战场 |
|---|---|---|
| **固件** | ADC/DMA、触发、协议解析、I2C 数字解码、屏幕 UI | `firmware/`、`proto/protocol.c` |
| **上位机** | Rust workspace、传输层、模拟器、CLI/GUI、测量算法 | `host/` |
| **Agent / 硬件** | MCP 工具、Agent 工作流、改板设计、BOM、焊接与实测 | `host/crates/mcp/`、`hardware/` |

三人都在 `docs/03-protocol.md` 上对齐 —— 改协议必须先改这里。

---

## 路线图

| 阶段 | 目标 | 验收标准 |
|---|---|---|
| **P0** | 协议闭环（无硬件） | 黄金向量在 C 与 Rust 两端全绿；`sim` 能跑通 capture |
| **P1** | 最简硬件闭环 | F103 采 2048 点 → 串口 → CLI 落 CSV → 能画图 |
| **P2** | 数字通路 I2C 解码 | 100k/400k/1MHz 真实总线解码结果与逻辑分析仪一致 |
| **P3** | MCP + Agent | Agent 一句话完成"抓一次 I2C 写时序并告诉我为什么 NACK" |
| **P4** | 增强 | 双通道改板、自动量程、GUI、等效时间采样 |

详见 [`docs/05-roadmap.md`](docs/05-roadmap.md)。

---

## ⚠️ 硬件上必须先知道的三件事

详见 [`docs/02-hardware.md`](docs/02-hardware.md)。

### 1. 核心板没有板载串口桥

地阔星**没有** CH340——Type-C 直连 MCU 的 PA11/PA12。上游 GD32 版"插上 Type-C 就有 COM 口"的能力**随核心板更换而消失**。

| 优先级 | 链路 | 引脚 |
|---|---|---|
| P1 期主力 | USART1 + 外接 USB-TTL | PA9 / PA10 |
| P2 起主力 | USB CDC | PA11 / PA12 |
| 🚫 **禁用** | ~~USART2~~ | ~~PA2 / PA3~~（PA2 是底板 PWM，PA3 是模拟输入，**硬冲突**） |

协议层不关心底层链路，换链路**协议一个字都不用改**（见 [ADR-009](docs/06-decisions.md)）。

### 2. 底板只有一路模拟输入

BNC → TL072 → **PA3/ADC_IN3**。而 I2C 解码需要 SCL/SDA **两路同时刻采样**。
三条路可选（见 [ADR-010](docs/06-decisions.md)）：

1. **改板加第二路前端** → 真正的双通道 I2C 示波器（推荐，P4 做）
2. **单通道 ADC + 一路数字**【P0–P3 就用这条】→ 一路走 ADC 看模拟质量，一路走 LM393 → PA6 捕获解协议
3. **纯数字双通路** → 加一颗比较器，两路都走定时器捕获，放弃模拟视图

### 3. 触发电平与量程是机械开关

SW2（AC/DC）与 SW3（X1/X50）都是手拨开关，**AI 无法程控**。自动量程要改板。

---

## 许可与致谢

本项目是**派生作品**，必须遵守上游许可：

| 上游 | 内容 | 许可 |
|---|---|---|
| [立创开源《简易数字示波器设计（入门版）》](https://oshwhub.com/course-examples/yi-qi-yi-biao-jian-yi-shu-zi-shi-bo-qi-she-ji-cha-jian-ban) | 硬件设计（原理图/PCB） | **GPL-3.0** |
| [chen11232/GD32E230-Oscilloscope](https://gitee.com/chen11232/GD32E230-Oscilloscope) | 参考固件源码 | **MulanPSL-2.0** |

- 硬件派生作品**必须**同样以 GPL-3.0 开源。
- 参考代码派生的固件部分遵从 MulanPSL-2.0。
- 上游页面明文**禁止商业性使用**。
- 本项目仓库整体以 **GPL-3.0** 发布以保证合规（见 [`LICENSE`](LICENSE) 与 [`NOTICE.md`](NOTICE.md)）。

致谢：立创开源硬件平台、立创EDA-莫工、EDA课程案例团队、chenlong、以及提供方案文档与 I2C 解码器行为规范的**马忠学学长**。
