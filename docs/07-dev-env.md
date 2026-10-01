# 07 · 开发环境搭建

> 目标：**新成员克隆仓库后 15 分钟内能跑通 `cargo test` 与 C 端向量测试。**
> 遇到问题先看第 6 节「已知的坑」，那两个坑几乎人人都会踩。

---

## 1. 需要装什么

| 组件 | 版本 | 用途 | 谁要装 |
|---|---|---|---|
| **git** | ≥ 2.30 | 版本管理 | 全员 |
| **Rust** (rustup) | stable | 上位机 | 上位机 + Agent |
| **Python** | ≥ 3.8 | 生成黄金测试向量 | 全员（跑测试要用） |
| **C 编译器** | C11 | 协议层测试 | 固件 |
| **Keil MDK** | ≥ 5.36 + AC6 | 固件编译 | 固件 |
| **DAP-Link** | — | 烧录 | 固件 |

> Python **不是可选项** —— `proto/tests/vectors.json` 由 `gen_vectors.py` 生成，
> C 端测试也依赖它把 JSON 转成头文件。不装 Python 就跑不了协议测试。

### 安装命令

<details>
<summary>Windows</summary>

```powershell
# Rust：用 GNU 工具链，避开 4GB 的 VS Build Tools
winget install Rustlang.Rustup
# 或下载 rustup-init 后：
#   rustup-init.exe -y --default-host x86_64-pc-windows-gnu

# Python
winget install Python.Python.3.12

# C 编译器：MSYS2 或 MinGW-w64
winget install MSYS2.MSYS2      # 然后 pacman -S mingw-w64-x86_64-gcc

# 或者已经装了 Git for Windows，可以用它的 make（如果装了）
```

> **为什么用 GNU 而不是 MSVC 工具链？**
> MSVC 需要 4 GB 的 Visual Studio Build Tools，而本项目只用到
> `serialport` 这类纯 Rust 库，GNU 完全够用。
> **例外**：将来若要做 Tauri GUI，需要换回 MSVC（WebView2 依赖它）。

</details>

<details>
<summary>Linux / macOS</summary>

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sudo apt install build-essential python3        # Debian/Ubuntu
brew install gcc python3                        # macOS
```

</details>

---

## 2. 跑通验证（10 分钟）

```bash
git clone <repo-url>
cd F103

# ── 1. C 端协议测试 ──────────────────────────────────
./proto/run_tests.sh
# 期望：全部通过: 50 项

# ── 2. Rust 上位机测试 ───────────────────────────────
./host/run.sh test
# 期望：所有 test result: ok，共 57 项

# ── 3. 端到端：对着模拟器抓一次波形 ──────────────────
./host/run.sh run -p scope-cli -- sim capture --scenario sine_1k_3v3 -n 1024 -o wave.csv
# 期望：打印采集摘要，并写出 wave.csv

# ── 4. MCP 工具链自检 ────────────────────────────────
./host/run.sh run -p scope-mcp -- --selftest
# 期望：自检通过
```

**四条全绿 = 环境就绪。** 没硬件也能到这一步。

---

## 3. 日常命令

| 目的 | 命令 |
|---|---|
| Rust 全部测试 | `./host/run.sh test` |
| C 协议测试 | `./proto/run_tests.sh` |
| 重新生成黄金向量 | `./proto/run_tests.sh --regen` |
| 格式 + 静态检查 | `./host/run.sh fmt --all && ./host/run.sh clippy --all-targets` |
| 列出可用串口 | `./host/run.sh run -p scope-cli -- ports` |
| 连真机抓波形 | `./host/run.sh run -p scope-cli -- serial --port COM3 capture -n 2048 -o wave.csv` |
| 对模拟器开发 | `./host/run.sh run -p scope-cli -- sim capture --scenario i2c_100k -n 4096 -o i2c.csv` |

> **始终用 `./host/run.sh` 而不是直接 `cargo`** —— 见下面的坑 #1。

---

## 4. 固件环境（Keil MDK）

1. 安装 **Keil MDK-ARM**（社区版即可）
2. 安装 **STM32F1xx DFP**（Device Family Pack）
3. 打开 `firmware/MDK-ARM/<工程名>.uvprojx`
   > ⚠️ **工程文件尚未创建**（`firmware/MDK-ARM/` 现在是空目录，`.uvprojx` 是 **P1 任务**）。
   > 在那之前，固件侧的协议逻辑可以先用 PC 上的 gcc 编译测试：
   > `gcc -std=c11 -Iproto proto/protocol.c proto/tests/test_vectors.c`
4. 调试器选 **DAP-Link**（ST-Link 在 Win11 上可能与 DAPLink 驱动冲突）

### 烧录方式

| 方式 | 适用 | 说明 |
|---|---|---|
| **DAP-Link / SWD** ⭐ | 日常开发 | 用核心板上的 `3V3/DIO/CLK/GND` 四脚 |
| 外接 USB-TTL 串口 ISP | 没有调试器时 | BOOT0=1、BOOT1=0 进 ROM bootloader |
| USB DFU | ⚠️ **不要指望** | F103C8T6 的系统 bootloader 是否支持原生 USB DFU 存在争议 |

### ⚠️ 数据链路的引脚选择

**地阔星核心板没有板载串口桥**，Type-C 直连 PA11/PA12。
详见 [`02-hardware.md`](02-hardware.md) §4。

```
P1 期：外接 USB-TTL → PA9(TX) / PA10(RX)   ← 最快打通
P2 起：USB CDC    → PA11 / PA12            ← 板载 Type-C，带宽高
🚫 禁止：USART2    → PA2 / PA3             ← 硬冲突（PWM 输出 + 模拟输入）
```

---

## 5. 编辑器

### VS Code 推荐配置

`.vscode/settings.json`（已提交）：

```json
{
  "rust-analyzer.cargo.extraEnv": {
    "CARGO_TARGET_DIR": "${env:HOME}/.cargo-target/i2c-scope-f103"
  },
  "rust-analyzer.check.command": "clippy",
  "files.associations": { "*.h": "c" },
  "C_Cpp.default.cStandard": "c11"
}
```

### 协议改动的工作流

**改协议 = 改四处 + 一次提交**（这是硬纪律，CI 会卡）：

1. `docs/03-protocol.md`
2. `proto/protocol.h`
3. `host/crates/proto/src/lib.rs`
4. `proto/tests/vectors.json`（跑 `./proto/run_tests.sh --regen`）

然后两端测试必须全绿：

```bash
./proto/run_tests.sh && ./host/run.sh test
```

---

## 6. ⚠️ 已知的坑

### 坑 #1：中文路径导致链接失败（Windows）

**症状**：`cargo build` 在链接阶段报一大堆

```
ld.exe: cannot find C:\Users\...\I2C示波器\F103\host\target\debug\deps\xxx.rcgu.o
ld.exe: cannot find ...\list.def: No such file or directory
collect2.exe: error: ld returned 1 exit status
```

**原因**：MinGW 的 `ld.exe` 用系统 ANSI 代码页解释路径，
而 rustc 传的是 UTF-8 —— 路径里的 `示波器` 三个字直接打不开。

**解决**：把 target 目录指到纯 ASCII 路径。

```bash
export CARGO_TARGET_DIR="C:/cargo-target/i2c-scope"
```

**`./host/run.sh` 会自动检测并处理这件事** —— 这就是它存在的原因。
要一劳永逸也可以写进用户环境变量。

> 把项目移到纯英文路径（如 `C:\dev\i2c-scope`）是更彻底的办法，
> 但当前路径是用户指定的，所以用环境变量绕过。

### 坑 #2：`link.exe` 被 Git Bash 的 coreutils 抢走

**症状**：MSVC 工具链下链接时报奇怪的参数错误。

**原因**：Git Bash 的 `/usr/bin/link`（coreutils 的建立硬链接工具）
在 PATH 里排在 MSVC 的 `link.exe` 前面。

**解决**：本项目用 GNU 工具链，**不会遇到这个问题**。
若将来切 MSVC，把 `/usr/bin` 从 PATH 里挪后，或用完整路径调 cargo。

### 坑 #3：`make` 在 Git Bash 里不存在

**症状**：`make -C proto test` 报 `command not found`。

**解决**：用 `./proto/run_tests.sh`，功能完全等价。
CI（Ubuntu）上 `make` 是可用的，两条路都支持。

### 坑 #4：串口被占用

**症状**：`Failed to open COM3` / `Access is denied`。

**原因**：串口助手、Keil 的调试器、上次没退干净的进程占着。

**解决**：关掉串口助手；Keil 里点 Stop Debug；实在不行拔插一次 USB。

### 坑 #5：USB CDC 连上后设备突然复位

**症状**：刚 `open` 就收到一堆乱码，或者 GET_INFO 无响应。

**原因**：USB CDC 首次打开时 DTR 拉低可能触发板子复位。
地阔星的 PA12 疑似 1.5 kΩ 硬上拉（`【待测】`），概率不低。

**解决**：连接后等 200 ms 再发第一条命令；
或在固件里加 DTR 忽略逻辑。

---

## 7. 团队协作建议

| 角色 | 只装这些就够 |
|---|---|
| 上位机 / Agent | git + Rust + Python |
| 固件 | git + Python + Keil + DAP-Link + MinGW（跑 C 端测试）|
| 硬件 | git + Python + 立创EDA专业版 + 万用表 |

**所有人都要能跑 `./proto/run_tests.sh`** —— 协议是三方交汇点，
谁改了协议都必须自己先验证两端一致。
