# NOTICE — 上游来源与许可归属

本项目（F103 Agent Scope）是**派生作品**，包含并衍生自以下上游作品。
分发、修改、再发布时必须保留本文件。

---

## 1. 硬件设计

**《简易数字示波器设计（入门版）》**

- 来源：立创开源硬件平台（OSHWHub）
- 项目页：<https://oshwhub.com/course-examples/yi-qi-yi-biao-jian-yi-shu-zi-shi-bo-qi-she-ji-cha-jian-ban>
- 工程 UUID：`5e03b6545745463bb8b64813209ffb8c`
- 作者：嘉立创EDA-莫工（`lceda_01`）
- 版权归属：EDA课程案例团队（`course-examples`）
- 团队成员：嘉立创EDA-莫工、牧尘（`chenlong11`）、`eda_kngxlawxc`、OSHWHub、立创开发板（`jlckfb`）
- 参与者（页面原文感谢名单）：
  - 福州大学旗山校区 李健东 — CW32 核心板移植与适配
  - 长春电子科技学院 郭富城 — STM32 核心板移植与适配
- **许可证：GPL-3.0**

**页面原文的知识产权声明：**

> 本项目为开源硬件项目，其相关的知识产权归创作者所有……仅供平台用户用于学习交流及研究，**不包括任何商业性使用**。

### 对本项目的约束

- 本项目对硬件的任何修改（改板、加装第二路前端等）**必须同样以 GPL-3.0 开源**。
- **禁止商业性使用**。
- 必须保留原作者署名与本声明。

---

## 2. 参考固件源码

**GD32E230-Oscilloscope（简易数字示波器）**

- 来源：Gitee
- 仓库：<https://gitee.com/chen11232/GD32E230-Oscilloscope>
- 作者：chenlong（`chen11232`）
- 分支：`master`（GD32E230）、`CW32版本`、`STM32版本`、`MSPG3507版本`
- **许可证：MulanPSL-2.0（木兰宽松许可证 v2）**

### 对本项目的约束

- 若 `firmware/` 中引入或改写该仓库的代码，相关文件必须保留 MulanPSL-2.0 许可声明。
- MulanPSL-2.0 与 GPL-3.0 兼容，本项目整体以 GPL-3.0 发布不构成冲突。

---

## 3. 开发文档

- 立创课程 wiki：<https://wiki.lceda.cn/zh-hans/course-projects/microcontroller/32-simple-oscilloscope/introduce.html>
- B 站视频教程 16 集（软件 11 集 + 硬件 5 集），见 `hardware/README.md`

文档内容版权归立创所有，本项目仅在 `docs/` 中**引用并标注来源**，不复制原文。

---

## 4. 方案与行为规范的来源

以下内容由**马忠学学长**提供，本项目借鉴其设计思路与行为规范：

- 《可被 AI Agent 控制的 STM32 示波器 —— 方案设计文档》
- `I2C示波器_改良版.exe`（Rust + egui 0.29 桌面应用，CSV 离线分析 + I2C 解码）

**学长方案中被本项目采用的部分**（思路借鉴，非代码复制）：

- 「命令只定义一次，三端复用」的单一真相源原则
- `AA 55` 帧头 + `seq` + `crc16` 的帧格式
- `GetCapabilities` 能力协商
- `DevicePort` 传输层解耦 + 模拟器一等公民
- I2C 解码器的行为规范：默认 SCL/SDA 通道、阈值 + 去抖、泳道显示、帧数统计、导出 `i2c_decode.txt`

上述两份材料**未收录进本仓库**：`.exe` 是第三方二进制（5.3 MB，不适合入库），方案文档是私人交流材料。需要时由团队成员自行留存于 `reference/`（该目录已被 `.gitignore` 排除）。

---

## 5. 第三方依赖

各语言生态的第三方依赖清单见对应目录：

- Rust：`host/Cargo.toml`
- C 固件：`firmware/README.md`
- 工具：`tools/README.md`
