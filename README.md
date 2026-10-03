# F103 Agent Scope

> 一台**可被 AI Agent 直接操控**的数字示波器 / I2C 总线分析仪。
> 硬件 = 立创开源《简易数字示波器设计（入门版）》插件底板 + 立创·**地阔星** `LCKFB-DKX-STM32F103C8T6` 核心板。

```
┌──────────────────────────────────────────────────────────────┐
│  AI Agent (Claude / 任意 LLM)        AI 分析面板（GUI 内嵌）   │
│      │ MCP (JSON-RPC)                     │ HTTPS + 密钥      │
│  ┌───▼─────────┐                     ┌────▼──────────────┐   │
│  │ MCP Server  │  ← 工具 schema      │ scope-gui         │   │
│  │             │    由 Rust 类型生成  │ （人操作的界面）   │   │
│  └───┬─────────┘                     └────┬──────────────┘   │
│      └──────────────┬───────────────────────┘                │
│  ┌──────────────────▼──┐                                     │
│  │ 命令层 core         │  CommandBus：编码 / seq / 超时重试   │
│  └──────────────────┬──┘                     / 状态缓存      │
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

> 两条通向模型的路，**服务的场景不同**：左边由 AI 主导（它决定采什么），
> 右边由人主导（AI 解释屏幕上这一窗）。真机串口独占，二者不能同时连同一台设备。

---

## 这个项目在解决什么

要的是一台**能被 AI Agent 直接操控**的调试仪器：AI 一句话就能抓波形、解协议、判断信号质量，不用人坐在示波器前拧旋钮。

平台是**立创开源《简易数字示波器设计（入门版）》插件底板 + STM32F103C8T6 核心板**。它给了模拟前端和一路比较器，同时留下三条硬件硬约束（**无以太网、无 FSMC、只有 2 个 ADC**）和一片空白的接口层 —— 整个软件栈就是围着这几件事展开的，详见 [`docs/00-origin.md`](docs/00-origin.md)。

**核心设计选择是双通路**：I2C 解码的瓶颈不是采样率，而是**边沿时间精度**。857 kSPS 的 ADC 要解 400 kHz 需要 3.33 MSPS，还伴随采样相位拍频（两颗晶振 40 ppm 频差会让解码周期性时好时坏），这是采样体制问题，软件补不了。而板上那颗 **LM393 滞回比较器**接到定时器输入捕获后，直接给出 **72 MHz / 13.9 ns 的边沿时间戳**（等效 ~72 MSPS），只占一个定时器通道，不吃 ADC、不吃内存带宽。

两条通路的分工：

| 通路 | 硬件 | 用途 | 能力 |
|---|---|---|---|
| **协议通路**（权威） | LM393 → TIM 输入捕获 | 解码 I2C 帧、时序、时钟拉伸 | 100k / 400k / 1MHz **全部可靠**，13.9 ns 分辨率 |
| **模拟通路**（差异化） | TL072 → ADC | 电平裕量、上拉强度、振铃、边沿形状 | 100 kHz 保证 / 400 kHz 有条件 / 1 MHz 不支持 |

这是 10 元逻辑分析仪永远给不了、而万元示波器才有的组合：**协议对不对** + **信号好不好**，一次说清。

---

## 性能承诺与边界

见 [`docs/04-performance.md`](docs/04-performance.md)。摘要：

| 指标 | F103 版本能做的 | 明确做不到的 |
|---|---|---|
| 采样率 | 单通道 **857 kSPS**；单通道交织 1.71 MSPS（仅显示）<br>⚠ 双通道同步 857 kSPS/路 **要等 P4 改板** —— 底板只有一路模拟输入（[ADR-010](docs/06-decisions.md)） | 1 MSPS 与 USB 并存；三 ADC 交织 7.2 MSPS（主板升级后，见路线图 P5） |
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
├─ docs/          设计文档 00~08（起源 / 架构 / 硬件 / 协议 / 性能边界 /
│                 路线图 / 决策记录 / 开发环境 / Agent 工作流实录）
├─ proto/         协议唯一真相源：C 头文件 + 纯 C 编解码 + 双端共用的黄金测试向量
├─ firmware/      STM32F103 固件（C，分层 App/Hardware）—— ⚠ 尚未实现，见该目录 README
├─ host/          Rust workspace，8 个 crate：
│                   proto · core · transport-serial · sim · device · cli · gui · mcp
├─ hardware/      立创底板资料与改板设计 —— ⚠ 尚未整理，见该目录 README
├─ tools/         独立工具：agent_demo（已实现）；i2c_decode / csv_export（仅规划）
└─ .github/       CI：5 个 job（C 向量 / Rust / GUI / 固件分层检查 / 端到端冒烟）
```

> **三个目录是空的**：`firmware/`（0 行代码）、`hardware/`（只有说明）、
> `tools/i2c_decode` 与 `tools/csv_export`（只有规划）。它们保留在树里是为了
> 说明**打算往哪放**，各自的 README 写了完整设计 —— 别把规划当成已有实现。

**协议真相源策略**：固件用 C、上位机用 Rust，跨语言无法靠编译器保证一致。
所以把 [`proto/tests/vectors.json`](proto/tests/vectors.json) 当作唯一契约 —— 同一份黄金向量由 Rust `#[test]` 和「不依赖 HAL 的纯 C 测试」两端同时执行，漂移在测试期暴露。

---

## 快速开始

> 全部命令都在**仓库根目录**（`F103/`）执行。
> ⚠ **`-o` 的相对路径是相对 `host/` 的**，不是相对你敲命令的地方 ——
> `run.sh` 会先 `cd` 到 `host/` 再调 cargo。想让它落在仓库根目录就写
> `-o ../wave.csv` 或给绝对路径。
>
> 用 `./host/run.sh` 而不是裸 `cargo`。它做两件事：把 `CARGO_TARGET_DIR` 指到
> 一个纯 ASCII 路径（历史包袱 —— 项目路径曾经含中文，MinGW 的 `ld.exe` 会链接失败；
> 现在 checkout 到中文目录仍然会踩，所以保留），以及统一构建参数。
> 详见 [`docs/07-dev-env.md`](docs/07-dev-env.md) 坑 #1。

### 路线 A：没有硬件也能开发（推荐先走这条）

```bash
# 1. 协议测试：C 与 Rust 两端跑同一份黄金向量
./proto/run_tests.sh            # C 端，50 项
./host/run.sh test              # Rust 端，**全部 ok 即通过**（不写死条数 —— 会腐烂）

# 2. 对着模拟器抓一次波形
./host/run.sh run -p scope-cli -- sim capture --scenario sine_1k_3v3 -n 1024 -o wave.csv

# 3. 看一眼 I2C 双通道是什么样
./host/run.sh run -p scope-cli -- sim capture --scenario i2c_100k -n 2048 -o i2c.csv

# 4. 打开桌面 GUI（自动连模拟器并采集一帧）
./host/run.sh run -p scope-gui -- --demo --scenario i2c_100k

# 5. MCP 工具链自检
./host/run.sh run -p scope-mcp -- --selftest
```

> GUI 是一个 **egui 桌面应用**（`host/crates/gui`）：设备 / 配置 / 波形
> （泳道显示、缩放平移、触发点、判决带）/ I2C 解码面板（双泳道 + 色标 +
> 交易表 + 信号质量）/ 历史采集 / 测量 / **AI 分析面板** / 导出 /
> 模拟器故障注入 / 帮助页。
> `--demo` 免点击直接出图；去掉它就是正常的连接流程。
> `--scenario <名字>` 指定初始场景，`--font <路径>` 换中文字体。

> **AI 分析面板**（工具条上的「AI 分析」）：把当前采集**已经算好的**证据
> （测量值、I2C 帧表、信号质量、minmax 包络）发给语言模型，结论直接显示在
> 界面上。不发送原始样点。密钥存于用户配置目录，**明文**，详见帮助页「注意事项」。
>
> 它与 MCP 那条路**不是二选一**：真机串口是独占的，GUI 占着串口时
> `scope-mcp` 打不开同一个口 —— 所以「分析你屏幕上这一窗」只能在 GUI 进程内做。

模拟器实现同一套 `DevicePort`，内含 F103 真实档位表、ARM/触发语义、状态机约束、
seq 去重缓存，以及六种故障注入（丢帧 / CRC 错 / 延迟尖峰 / 永不触发 / 强制溢出）。
MCP / CLI / GUI 切到 `sim` 后 **Agent 逻辑零改动**。

> 它**不按波特率节流** —— 那样只会让测试变慢。链路的快慢只影响命令层估算超时。

### 路线 B：有硬件

```bash
# 先看看板子插在哪个口
./host/run.sh run -p scope-cli -- ports

# 连真机抓波形（注意 --port/--baud 在 serial 之后、子命令之前）
./host/run.sh run -p scope-cli -- serial --port COM3 --baud 921600 capture -n 2048 -o wave.csv
```

固件工程**尚未创建**（P1 任务）—— `firmware/README.md` 写了完整的分层结构与采样架构，
拿到板子后照着建。烧录用 DAP-Link，接线见那份文档。

### 路线 C：把它接到你自己的 Agent 上

`scope-mcp` 是标准 MCP server，跑在 stdio 上。任何 MCP 客户端都能接：

```bash
# Claude Code
claude mcp add scope -- <仓库绝对路径>/host/target/debug/scope-mcp.exe

# 或者直接跟任意客户端/脚本用 JSON-RPC 对话（一行一条消息）
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | ./host/target/debug/scope-mcp.exe
```

`tools/agent_demo/` 里有一个**零依赖的最小客户端**（一百多行 Python），
做的是同样的事：`tools/list` 拿 schema → 转成 LLM 的工具表 → 跑 tool-use 循环。
它同时是 [`docs/08-agent-walkthrough.md`](docs/08-agent-walkthrough.md)
那份实录的生成程序 —— 想改造自己的用法，从它改起最快。

> ⚠ **`capture_id` 是进程内的。** 一次管道输入就是一个会话，进程退出后
> `capture_id` 作废，下次从 1 重新开始。所以「抓完再读」必须把
> `scope_capture` 与后续的 `scope_read_waveform` / `scope_measure` /
> `scope_i2c_decode` **一次喂进同一个进程**。

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

| 阶段 | 目标 | 状态 | 验收标准 |
|---|---|---|---|
| **P0** | 协议闭环（无硬件） | ✅ 完成 | 黄金向量在 C 与 Rust 两端全绿；`sim` 能跑通 capture |
| **P1** | 最简硬件闭环 | ⚠️ 固件的 `App/` 层**硬件无关部分已做完**（hal / trigger / acq / proto_task，129 项断言在 PC 上跑）；`Hardware/` 与 Keil 工程未做 | F103 采 2048 点 → 串口 → CLI 落 CSV → 能画图 |
| **P2** | 数字通路 I2C 解码 | ⚠️ 主机侧完成，硬件侧 0 | 100k/400k/1MHz 真实总线解码结果与逻辑分析仪一致 |
| **P3** | MCP + Agent | ✅ 完成，见 [`docs/08`](docs/08-agent-walkthrough.md) | Agent 一句话完成"抓一次 I2C 写时序并告诉我为什么 NACK" |
| **P4** | 增强 | ⚠️ GUI / 历史 / 导出 / **AI 分析面板**已做；改板与自动量程未做 | 双通道改板、自动量程、等效时间采样 |
| **P5** | 主板升级 STM32F407 | ❌ 未开始（后期计划） | 以太网 / FSMC / 三 ADC 三重解锁：7.2 MSPS、深存储、直接传原始波形 |

> **一句话现状**：软件栈（协议 / 上位机 / 模拟器 / GUI / MCP / Agent / GUI 内嵌 AI）
> 已完整可跑，固件的 `App/` 层（硬件无关的那一半）也已做完并可在 PC 上测试。
> **但没有硬件** —— `firmware/Hardware/` 与 Keil 工程是空的，串口路径
> **从未与真机对过话**。所有演示都跑在内置模拟器上。

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

### 3. 耦合与量程是机械开关

SW2（AC/DC 耦合）与 SW3（X1/X50 衰减）都是手拨开关，**AI 无法程控**。
自动量程要改板（换继电器/模拟开关）。

> **触发电平不在此列** —— 它是软件判定的（固件拿样点和 `trigger_level_lsb`
> 比较），可以程控。此处小标题曾经写成「触发电平与量程是机械开关」，
> 与它自己的正文（只提 SW2/SW3）矛盾，会让人以为触发电平也设不了。
>
> 另外，数字通路那颗 LM393 的阈值是**电阻分压固定的**
> （`Uth=2.214 V / Utl=2.172 V`，见 [`docs/02-hardware.md`](docs/02-hardware.md) §5），
> **不受 `SET_TRIGGER.level_lsb` 影响** —— 那个电平只管模拟通路的触发搜索。

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

致谢：立创开源硬件平台、立创EDA-莫工、EDA课程案例团队、chenlong。
