# 08 · Agent 工作流实录

> 目标：兑现 P3 的验收标准 ——「**Agent 一句话完成一次真实的总线调试**」——
> 并且**可复现**：这份实录不是整理出来的，是一个脚本跑出来的原始输出。
>
> 复现方法见第 6 节。生成它的程序在 [`tools/agent_demo/`](../tools/agent_demo/)。

---

## 1. 给 Agent 的一句话

只给它这一句，**不给任何工具提示** —— 工具表、参数、schema 全靠它自己从
`tools/list` 里读：

> 「我这条 I2C 总线上挂了个传感器，读它一直返回全 0，怀疑是总线本身有问题。
> 你帮我看看总线上到底在发生什么，给我个结论。」

---

## 2. Agent 给出的结论（原文，未经润色）

### 结论：总线上根本没有读操作——问题不在总线，在代码

总线本身是**健康的**，你这条 I2C 上唯一的异常是：**从来没有人去读那个传感器**。

#### 总线上实际在发生什么

我连着抓了几段，解码出来是完全同一个模式，反复循环：

```
S · 0x44+W · ACK · 0x00 · ACK · 0x1A · ACK · P     ← 每 ~310 µs 重复一次
```

即：**往地址 0x44 的寄存器 0x00 写 0x1A**，写完 STOP，隔 20~30 µs 再来一次，一刻不停。

四段窗口累计 74 个完整帧，统计如下：

| 项 | 结果 |
|---|---|
| 写事务（R/W = 0） | **74 / 74，全部** |
| 读事务（R/W = 1） | **0 次** |
| 每帧从机 ACK | 全部 ACK，**0 个 NACK** |
| 重复起始 / 10 位地址 / 帧不完整 | 无 / 无 / 无（只有窗口边界截断的那一帧） |

#### 信号质量：没问题

- SCL 实测 **100.0 kHz**（99 995~100 020 Hz），占空比 **50.0%**
- 电平干净，`unknown_level_samples = 0`——每个采样点都明确落在高(3481)或低(614)，没有悬空/中间态
- 幅度 2.31 Vpp，上升沿很陡（<1 个比特时间）
- 3 秒连续采集 15 次，**0 次溢出、0 次缺口**，从机从不停摆

换句话说：线没断、没短路、没被拉死、没有时钟拉伸卡死、地址也没错——**从机每次都乖乖 ACK**。这是一条工作正常的总线。

#### 那"读出来全是 0"是怎么回事

因为**总线上从来没有产生过读事务**。你的程序在死循环里反复写 `寄存器 0x00 = 0x1A`（这看着像初始化/配置写），写完就 STOP，压根没发出 `0x44+R`。应用层那个"读取结果"的缓冲区因此从来没被填过，自然一直是全 0——**这不是读失败，是压根没读**。

一个旁证：写循环的节奏是每 310 µs 一次、总线上几乎无空闲（占用率 >90%）。如果代码在读写交替，这种密度的流量里不可能一次读都夹不进来。

另外要留意：同一个寄存器被写了每秒三千多次，说明代码很可能**卡在重试初始化的循环里**，而不是走到了读取那一步。

#### 建议你去查的地方

1. **驱动的读路径**：正常的"写寄存器指针 + 读数据"应该是 `S · 0x44+W · 0x00 · ACK · Sr · 0x44+R · ACK · data... · NACK · P`，中间是**重复起始 Sr**。你现在的波形是写完直接 **STOP** 再从头来——检查是不是把 repeated-start 写成了 stop-then-start，或者读函数根本没被调用到。
2. 加日志/打断点确认那个 read 调用到底有没有执行；也可以把示波器触发条件改成"地址 = 0x44 且 R/W = 1"来抓，看它是否真的出现过。

#### 需要你知道的两点局限

- **观测是断续的**：采集环只有 8 KB，单窗口最长 4.8 ms（半采样率下 9.6 ms）。我在约 9 秒内累计只覆盖了约 0.15 秒的总线时间。**我无法排除窗口之外存在极低频的读操作**——但从窗口内"无空闲、纯写"的密度看，读写交替的可能性很小。要彻底排除，建议用真机 + 连续流模式长时间抓。
- **地址 0x44 是我从波形解出来的 7 位地址**（线上字节 0x88）。我用比特级手工核对过第一帧：8 个数据位 = `1 0 0 0 1 0 0 0` = 0x88，第 9 个时钟 SDA 被从机拉低 = ACK，解码器没骗我。**请对照你传感器的数据手册确认它确实是 0x44**——如果你预期的是别的地址，那"有东西在 ACK"就变成另一个故事了。

设备目前还连着（模拟器，采样率我调到了 428571 Hz 做窗口加宽验证）。要不要我把它调回 857142 Hz，或者换个场景/加故障注入再验证一遍？

---

## 3. 它是怎么做到的

一共 **13 轮**、**15 次工具调用**（其中 1 次被 schema 挡下），
**13 次模型调用**，输入 18641 / 输出 15180 tokens。

下面每一轮的「调用 / 结果」都是脚本落盘的原始记录，没有改写。

### 第 1 轮（1.6s）

> *思考：The user wants me to investigate an I2C bus. Let me start by connecting and capturing.  Let me plan: connect to simulator (default i2c_100k scenario — that's the default and only scenario that validates I2C decoding), list devices maybe, status, configure, capture, decode, measure.  Let me start.*

我来帮你查这条 I2C 总线。先接上设备看看。

**调用** `scope_list_devices`
```json
{}
```
**结果** → `isError: false`，2 个字段：`note`, `ports`

<details><summary>展开原始响应</summary>

```json
{
 "note": "不指定 port 调用 scope_connect 时使用内置模拟器，无需硬件",
 "ports": []
}
```
</details>

**调用** `scope_connect`
```json
{
 "sim_scenario": "i2c_100k"
}
```
**结果** → `isError: false`，5 个字段：`config`, `device`, `link`, `simulated`, `state`

<details><summary>展开原始响应</summary>

```json
{
 "config": {
  "acq_mode": 0,
  "capture_samples": 4096,
  "decimation": 1,
  "format": 0,
  "rate_hz": 857142,
  "trigger_edge": 0,
  "trigger_level_lsb": 2048,
  "trigger_mode": 1,
  "trigger_source": 0
 },
 "device": {
  "adc_bits": 12,
  "capture_max_samples": 4096,
  "channels": 2,
  "fw_ver": "1.0.0",
  "max_rx_payload": 512,
  "max_tx_payload": 2060,
  "model": "0x0103",
  "preferred_chunk_samples": 1024,
  "proto_ver": 16,
  "rate_max_hz": 857142
 },
 "link": "sim(i2c_100k)",
 "simulated": true,
 "state": "空闲"
}
```
</details>

### 第 2 轮（2.4s）

> *思考：设备已连接，模拟器，i2c_100k 场景，2 通道。  现在配置采样然后采集。默认采样率 857142 Hz，对于 100 kHz I2C 来说够用。注意 capture_samples 上限 4096。  让我先做一次采集看看。触发源默认 CH1，触发模式 normal（1）。I2C 场景，抓取即可。  先直接 capture。不过要注意触发——如果总线空闲可能不触发。用 auto 模式更保险。  让我配置一下：mode single, trigger mode auto, capture_samples 4096。然后采集。  实际上我应该核对状态并做采集。让我配置并采集。  我可以并行…*

设备接上了（内置模拟器，i2c_100k 场景，2 通道）。先把采集参数配好——I2C 这种总线要在空闲时也能出结果，触发得设成 auto，免得等不到触发白挂住。

**调用** `scope_configure`
```json
{
 "capture_samples": 4096,
 "mode": "single",
 "sample_rate_hz": 857142,
 "trigger": {
  "mode": "auto",
  "source": 0,
  "level_lsb": 2048,
  "edge": "rising"
 }
}
```
**结果** → `isError: false`，2 个字段：`applied`, `warnings`

<details><summary>展开原始响应</summary>

```json
{
 "applied": {
  "capture_samples": 4096,
  "decimation": 1,
  "mode": "single",
  "sample_rate_hz": 857142,
  "trigger": {
   "edge": "rising",
   "level_lsb": 2048,
   "mode": "auto",
   "source": 0
  }
 },
 "warnings": []
}
```
</details>

### 第 3 轮（1.2s）

**调用** `scope_capture`
```json
{
 "capture_id": 1,
 "mode": "single",
 "max_preview_points": 256
}
```
**结果** → `isError: true`
```json
{"error":"参数不合法：unknown field `capture_id`, expected one of `mode`, `timeout_ms`, `max_preview_points`","hint":"本工具接受的参数：max_preview_points:整数? / mode:single|stream? / timeout_ms:整数?。用 tools/list 可以看每一项的完整说明"}
```

### 第 4 轮（1.2s）

> *思考：Right, capture doesn't take capture_id. Let me retry.*

我多传了个参数，去掉重来。

**调用** `scope_capture`
```json
{
 "max_preview_points": 256,
 "mode": "single",
 "timeout_ms": 2000
}
```
**结果** → `isError: false`，9 个字段：`capture_id`, `channels`, `duration_ms`, `note`, `overrun`, `preview`, `rate_hz`, `sample_count`, `trigger_index`

<details><summary>展开原始响应</summary>

```json
{
 "capture_id": 1,
 "channels": [
  {
   "ac_rms_lsb": 1421.8113857142132,
   "channel": 0,
   "duty_pct": 49.98545242944428,
   "freq_hz": 100004.05644457375,
   "max_lsb": 3481,
   "mean_lsb": 2230.187255859375,
   "min_lsb": 614,
   "pp_lsb": 2867,
   "rise_ns": 933.3342666676008,
   "rising_edges": 417,
   "rms_lsb": 2644.8596962304337,
   "sample_count": 4096,
   "vpp_v": 2.3098388671875
  },
  {
   "ac_rms_lsb": 1251.7780849001467,
   "channel": 1,
   "duty_pct": 49.45474372955289,
   "freq_hz": 28509.08505997819,
   "max_lsb": 3481,
   "mean_lsb": 1348.94873046875,
   "min_lsb": 614,
   "pp_lsb": 2867,
   "rise_ns": 933.3342666676008,
   "rising_edges": 77,
   "rms_lsb": 1840.2747216841108,
   "sample_count": 4096,
   "vpp_v": 2.3098388671875
  }
 ],
 "duration_ms": 4.778,
 "note": "只给统计量与 minmax 预览。全量样点走 scope_read_waveform（分页）或 scope_save_capture（落盘）—— 4096 点 ≈ 上万 token，不进上下文。",
 "overrun": false,
 "preview": {
  "bucket": 16,
  "channel": 0,
  "dt_us": 18.666685333352,
  "points": 256,
  "t0_us": 0.0,
  "y_max": [
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   
…（略）
```
</details>

### 第 5 轮（1.7s）

> *思考：Both channels look like clock-ish signals: CH0 freq ~100 kHz, CH1 freq ~28.5 kHz. That's interesting — I2C should have SCL and SDA on the same transaction. Let me decode.  Actually CH1 freq 28509 Hz is odd. Let's just run the I2C decode and see.  Let me also read the waveform for both channels to un…*

两路都有信号：CH0 ≈ 100 kHz，CH1 ≈ 28.5 kHz。直接上 I2C 解码看帧内容，通道让工具自己按边沿密度判定。

**调用** `scope_i2c_decode`
```json
{
 "capture_id": 1
}
```
**结果** → `isError: false`，10 个字段：`all_bytes`, `capture_id`, `frame_count`, `frames`, `levels`, `scl_channel`, `sda_channel`, `signal_quality`, `trustworthy`, `warnings`

<details><summary>展开原始响应</summary>

```json
{
 "all_bytes": [
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26
 ],
 "capture_id": 1,
 "frame_count": 16,
 "frames": [
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 1,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.005
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 2,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.315
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 3,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.625
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 4,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.935
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 5,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.246
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 6,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.555
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   
…（略）
```
</details>

### 第 6 轮（10.6s）

> *思考：Decoded: 16 frames, each: address 0x68 write, bytes [0x00, 0x1A]. So the master writes register 0x00 = 0x1A. Hmm, wait — address 68 decimal = 0x44. Actually "value":68 decimal. Let me note: address value 68 decimal = 0x44. 7-bit address 0x44.  Wait — read:false means it's a write. So the traffic is:…*

解码出来了，信息量不小。我手工核对一下头几个比特，确认解码器没骗我——按 100 kHz 时钟算，这里每个 SCL 周期才 8 点多采样点，属于比较勉强的情况，值得验一眼。

**调用** `scope_read_waveform`
```json
{
 "capture_id": 1,
 "channel": 0,
 "start_sample": 0,
 "count": 120
}
```
**结果** → `isError: false`，8 个字段：`capture_id`, `channel`, `count`, `format`, `has_more`, `rate_hz`, `samples`, `start_sample`

<details><summary>展开原始响应</summary>

```json
{
 "capture_id": 1,
 "channel": 0,
 "count": 120,
 "format": "raw",
 "has_more": true,
 "rate_hz": 857142,
 "samples": [
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481
 ],
 "start_sample": 0
}
```
</details>

**调用** `scope_read_waveform`
```json
{
 "capture_id": 1,
 "channel": 1,
 "start_sample": 0,
 "count": 120
}
```
**结果** → `isError: false`，8 个字段：`capture_id`, `channel`, `count`, `format`, `has_more`, `rate_hz`, `samples`, `start_sample`

<details><summary>展开原始响应</summary>

```json
{
 "capture_id": 1,
 "channel": 1,
 "count": 120,
 "format": "raw",
 "has_more": true,
 "rate_hz": 857142,
 "samples": [
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  3481,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614,
  614
 ],
 "start_sample": 0
}
```
</details>

### 第 7 轮（17.0s）

> *思考：CH0 = SCL (clock), ~4-5 samples per half period. CH1 = SDA.  Let me manually decode. SCL samples (channel 0), sample indices 0..119: index: value 0-8: 3481 (high) 9-12: 614 (low) 13-17: 3481 18-22: 614 23-26: 3481 27-30: 614 31-35: 3481  (wait let me re-read)  Let me carefully write out channel 0 wi…*

手工核对对上了：头一个字节的 8 个数据位在 SCL 高电平窗口里是 `1 0 0 0 1 0 0 0` = **0x88**，即 7 位地址 0x44 + R/W=0，**写**；第 9 个时钟 SDA 被从机拉低 = ACK。解码器可信。

现在关键点是：这次 4.8 ms 窗口里只有写、一次读都没有。但 4.8 ms 太短，可能读操作是在更慢的节奏上。我拉长观察一段时间再下结论。

**调用** `scope_watch`
```json
{
 "duration_ms": 3000,
 "max_points": 256
}
```
**结果** → `isError: false`，6 个字段：`captures`, `gaps`, `note`, `overruns`, `requested_ms`, `sample`

<details><summary>展开原始响应</summary>

```json
{
 "captures": 15,
 "gaps": 0,
 "note": "用重复单次采集实现（设备侧流模式尚未实现），两次采集之间必有间隙；                     gaps 是其中没等到触发的次数。只有第一次采集进了历史，\"sample\" 的                      capture_id 可直接用于 scope_measure / scope_read_waveform。",
 "overruns": 0,
 "requested_ms": 3000,
 "sample": {
  "capture_id": 2,
  "channels": [
   {
    "ac_rms_lsb": 1422.1683286133489,
    "channel": 0,
    "duty_pct": 50.0145137880987,
    "freq_hz": 100020.63396226415,
    "max_lsb": 3481,
    "mean_lsb": 2227.387451171875,
    "min_lsb": 614,
    "pp_lsb": 2867,
    "rise_ns": 933.3342666676008,
    "rising_edges": 418,
    "rms_lsb": 2642.6913577920573,
    "sample_count": 4096,
    "vpp_v": 2.3098388671875
   },
   {
    "ac_rms_lsb": 1252.9475947260646,
    "channel": 1,
    "duty_pct": 50.32858707557503,
    "freq_hz": 28633.987951807227,
    "max_lsb": 3481,
    "mean_lsb": 1351.048583984375,
    "min_lsb": 614,
    "pp_lsb": 2867,
    "rise_ns": 933.3342666676132,
    "rising_edges": 77,
    "rms_lsb": 1842.609549366337,
    "sample_count": 4096,
    "vpp_v": 2.3098388671875
   }
  ],
  "duration_ms": 4.778,
  "note": "只给统计量与 minmax 预览。全量样点走 scope_read_waveform（分页）或 scope_save_capture（落盘）—— 4096 点 ≈ 上万 token，不进上下文。",
  "overrun": false,
  "preview": {
   "bucket": 16,
   "channel": 0,
   "dt_us": 18.666685333352,
   "points": 256,
   "t0_us": 0.0,
   "y_max": [
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
  
…（略）
```
</details>

### 第 8 轮（6.0s）

> *思考：15 captures over 3000 ms, 0 gaps (no timeouts), 0 overruns. So the master keeps hammering the bus continuously for 3 seconds — meaning it's a very dense continuous pattern with no idle periods. Wait, 15 captures × 4.78 ms = 71.7 ms of actual capture in 3000 ms. So the bus is active essentially alway…*

3 秒内 15 次采集，**0 次缺口、0 次溢出**——总线一直是活的，没有卡死、没有从机停摆。我确认下这次采集的流量跟上次是否一致。

**调用** `scope_i2c_decode`
```json
{
 "capture_id": 2
}
```
**结果** → `isError: false`，10 个字段：`all_bytes`, `capture_id`, `frame_count`, `frames`, `levels`, `scl_channel`, `sda_channel`, `signal_quality`, `trustworthy`, `warnings`

<details><summary>展开原始响应</summary>

```json
{
 "all_bytes": [
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0
 ],
 "capture_id": 2,
 "frame_count": 15,
 "frames": [
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 1,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.186
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 2,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.497
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 3,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.807
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 4,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.116
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 5,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.426
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 6,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.737
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes
…（略）
```
</details>

### 第 9 轮（16.6s）

> *思考：Same pattern. Confirmed.  Now let me consider whether to also test debounce sensitivity — i.e., is my "trustworthy" conclusion robust? unknown_level_samples: 0, clean. Good.  Let me also consider: should I test with a different debounce to ensure the decode isn't an artifact? Probably not needed.  N…*

一致的：又是清一色的 `0x44 + W`、`[0x00, 0x1A]`，全 ACK，没有一次读（`read:false`）。

不过我得诚实对待一个局限：每次采集窗口只有 4.78 ms，而 watch 在 3 s 里只落了 15 个窗口，覆盖的“总线时间”其实只有约 2.4%。为了别漏掉慢节奏的读操作，我再取一轮，并且试着用更低采样率把窗口拉宽做交叉验证。

**调用** `scope_watch`
```json
{
 "duration_ms": 3000,
 "max_points": 128
}
```
**结果** → `isError: false`，6 个字段：`captures`, `gaps`, `note`, `overruns`, `requested_ms`, `sample`

<details><summary>展开原始响应</summary>

```json
{
 "captures": 15,
 "gaps": 0,
 "note": "用重复单次采集实现（设备侧流模式尚未实现），两次采集之间必有间隙；                     gaps 是其中没等到触发的次数。只有第一次采集进了历史，\"sample\" 的                      capture_id 可直接用于 scope_measure / scope_read_waveform。",
 "overruns": 0,
 "requested_ms": 3000,
 "sample": {
  "capture_id": 17,
  "channels": [
   {
    "ac_rms_lsb": 1420.612593907798,
    "channel": 0,
    "duty_pct": 50.014692918013516,
    "freq_hz": 99995.7020276227,
    "max_lsb": 3481,
    "mean_lsb": 2239.28662109375,
    "min_lsb": 614,
    "pp_lsb": 2867,
    "rise_ns": 933.3342666676008,
    "rising_edges": 414,
    "rms_lsb": 2651.8945517080624,
    "sample_count": 4096,
    "vpp_v": 2.3098388671875
   },
   {
    "ac_rms_lsb": 1266.2548115615334,
    "channel": 1,
    "duty_pct": 50.583244962884415,
    "freq_hz": 28631.99681866384,
    "max_lsb": 3481,
    "mean_lsb": 1375.546875,
    "min_lsb": 614,
    "pp_lsb": 2867,
    "rise_ns": 933.3342666675967,
    "rising_edges": 79,
    "rms_lsb": 1869.6337751348524,
    "sample_count": 4096,
    "vpp_v": 2.3098388671875
   }
  ],
  "duration_ms": 4.778,
  "note": "只给统计量与 minmax 预览。全量样点走 scope_read_waveform（分页）或 scope_save_capture（落盘）—— 4096 点 ≈ 上万 token，不进上下文。",
  "overrun": false,
  "preview": {
   "bucket": 32,
   "channel": 0,
   "dt_us": 37.333370666704,
   "points": 128,
   "t0_us": 0.0,
   "y_max": [
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481,
    3481
…（略）
```
</details>

### 第 10 轮（3.7s）

> *思考：Now decode capture 17. And then do the wide-window cross check.  Let me decode 17 and configure lower rate + capture.  These are dependent on config change. Let me decode 17 first, then reconfigure.  Actually I can decode 17 in parallel with... no, decode is a read-only op on stored capture; the con…*

**调用** `scope_i2c_decode`
```json
{
 "capture_id": 17
}
```
**结果** → `isError: false`，10 个字段：`all_bytes`, `capture_id`, `frame_count`, `frames`, `levels`, `scl_channel`, `sda_channel`, `signal_quality`, `trustworthy`, `warnings`

<details><summary>展开原始响应</summary>

```json
{
 "all_bytes": [
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26
 ],
 "capture_id": 17,
 "frame_count": 16,
 "frames": [
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 1,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.116
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 2,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.427
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 3,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.737
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 4,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.046
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 5,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.356
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 6,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.667
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
  
…（略）
```
</details>

**调用** `scope_configure`
```json
{
 "sample_rate_hz": 428571,
 "capture_samples": 4096,
 "mode": "single"
}
```
**结果** → `isError: false`，2 个字段：`applied`, `warnings`

<details><summary>展开原始响应</summary>

```json
{
 "applied": {
  "capture_samples": 4096,
  "decimation": 1,
  "mode": "single",
  "sample_rate_hz": 428571
 },
 "warnings": []
}
```
</details>

### 第 11 轮（1.8s）

> *思考：Third window: identical. Now capture at 428571 and decode to widen window.  Let me capture then decode.*

第三段窗口依旧一模一样。现在做拉宽窗口的交叉验证（半采样率，窗口翻倍到约 9.6 ms）。

**调用** `scope_capture`
```json
{
 "max_preview_points": 64,
 "timeout_ms": 2000
}
```
**结果** → `isError: false`，9 个字段：`capture_id`, `channels`, `duration_ms`, `note`, `overrun`, `preview`, `rate_hz`, `sample_count`, `trigger_index`

<details><summary>展开原始响应</summary>

```json
{
 "capture_id": 32,
 "channels": [
  {
   "ac_rms_lsb": 1421.448838463657,
   "channel": 0,
   "duty_pct": 49.98540145985402,
   "freq_hz": 99979.04496350365,
   "max_lsb": 3481,
   "mean_lsb": 2232.987060546875,
   "min_lsb": 614,
   "pp_lsb": 2867,
   "rise_ns": 1866.6685333351998,
   "rising_edges": 831,
   "rms_lsb": 2647.0262584529555,
   "sample_count": 4096,
   "vpp_v": 2.3098388671875
  },
  {
   "ac_rms_lsb": 1255.6585587915376,
   "channel": 1,
   "duty_pct": 49.864645370871685,
   "freq_hz": 28540.461829994587,
   "max_lsb": 3481,
   "mean_lsb": 1355.9482421875,
   "min_lsb": 614,
   "pp_lsb": 2867,
   "rise_ns": 1866.6685333352016,
   "rising_edges": 154,
   "rms_lsb": 1848.0460091019954,
   "sample_count": 4096,
   "vpp_v": 2.3098388671875
  }
 ],
 "duration_ms": 9.557,
 "note": "只给统计量与 minmax 预览。全量样点走 scope_read_waveform（分页）或 scope_save_capture（落盘）—— 4096 点 ≈ 上万 token，不进上下文。",
 "overrun": false,
 "preview": {
  "bucket": 64,
  "channel": 0,
  "dt_us": 149.333482666816,
  "points": 64,
  "t0_us": 0.0,
  "y_max": [
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481,
   3481
  ],
  "y_min": [
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   614,
   
…（略）
```
</details>

### 第 12 轮（1.2s）

**调用** `scope_i2c_decode`
```json
{
 "capture_id": 32
}
```
**结果** → `isError: false`，10 个字段：`all_bytes`, `capture_id`, `frame_count`, `frames`, `levels`, `scl_channel`, `sda_channel`, `signal_quality`, `trustworthy`, `warnings`

<details><summary>展开原始响应</summary>

```json
{
 "all_bytes": [
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26,
  0,
  26
 ],
 "capture_id": 32,
 "frame_count": 31,
 "frames": [
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 1,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.088
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 2,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.399
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 3,
   "nack": false,
   "repeated_start": false,
   "time_ms": 0.709
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 4,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.019
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 5,
   "nack": false,
   "repeated_start": false,
   "time_ms": 1.327
  },
  {
   "address": {
    "acked": true,
    "read": false,
    "ten_bit": false,
    "value": 68
   },
   "bytes": [
    0,
    26
   ],
   "complete": true,
   "index": 6,
   "nack":
…（略）
```
</details>

### 第 13 轮（12.6s）

> *思考：The half-rate capture decodes identically: 31 frames of 0x44 W [0x00, 0x1A], all ACKed, no reads, over 9.56 ms. Nice — the decode held up at 4.3 samples/cycle too (rise_ns 1866, doubled as expected).  Now I have ~4 windows: 4.78, 4.78, 4.78, 9.56 ms of continuous bus activity, all identical write-on…*

## 结论：总线上根本没有读操作——问题不在总线，在代码

总线本身是**健康的**，你这条 I2C 上唯一的异常是：**从来没有人去读那个传感器**。

### 总线上实际在发生什么

我连着抓了几段，解码出来是完全同一个模式，反复循环：

```
S · 0x44+W · ACK · 0x00 · ACK · 0x1A · ACK · P     ← 每 ~310 µs 重复一次
```

即：**往地址 0x44 的寄存器 0x00 写 0x1A**，写完 STOP，隔 20~30 µs 再来一次，一刻不停。

四段窗口累计 74 个完整帧，统计如下：

| 项 | 结果 |
|---|---|
| 写事务（R/W = 0） | **74 / 74，全部** |
| 读事务（R/W = 1） | **0 次** |
| 每帧从机 ACK | 全部 ACK，**0 个 NACK** |

**（没有工具调用 → 给出最终答复）**

---

## 4. 弯路：一次真实的自纠错

第 3 轮它给 `scope_capture` 传了一个 `capture_id`：

```json
{"name": "scope_capture", "input": {"capture_id": 1, "mode": "single", "max_preview_points": 256}}
```

`scope_capture` 的参数里没有这个字段（`capture_id` 是**采集的产物**，
不是它的入参）。于是它拿到的是一条：

```json
{"error": "参数不合法：unknown field `capture_id`, expected one of `mode`, `timeout_ms`, `max_preview_points`",
 "hint": "本工具接受的参数：max_preview_points:整数? / mode:single|stream? / timeout_ms:整数?。用 tools/list 可以看每一项的完整说明"}
```

第 4 轮它自己改对了，去掉那个字段、补上 `timeout_ms`，一次通过。

**这段值得单列出来，因为它是整个参数类型化改造的兑现现场。** 改造之前，
`scope_capture` 的实现是 `p.get("字段名")` —— 多传一个 `capture_id`
会被**静默忽略**，工具照常返回成功。模型会以为自己指定了什么，实际上
什么都没发生，而它**永远不会知道**。

现在这条路径给的是：确切的字段名 + 这个工具接受哪些参数。模型一轮就恢复了。

---

## 5. 这次实录证明了什么 / 暴露了什么

### 证明了的

- **schema 是够用的契约**：13 次调用里 12 次一次成功，唯一一次失败是模型
  自己多塞了一个字段，而不是 schema 没讲清楚某个参数该怎么填。
- **Agent 会主动声明自己的局限**。结论里有两处它自己划的边界：
  「我在约 9 秒内累计只覆盖了约 0.15 秒的总线时间，**我无法排除窗口之外存在
  极低频的读操作**」；「地址 0x44 是我从波形解出来的……**请对照你的传感器
  数据手册确认**」。这两句不是我们教的，是它自己加的。
- **三层 token 防护是有效的**。整个调试过程（含读原始样点、两次 3 秒连续观察）
  一共 18.6k 输入 / 15.2k 输出 token。它没有一次拿到过全量波形 ——
  需要细节时是自己去 `scope_read_waveform` 分页取的。

### 暴露了的

1. **模拟器场景不是一份像样的总线快照**。`i2c_100k` 里**只有写循环、从来没有读**，
   而那正是这次任务问的事。Agent 于是给了一个「读请求压根没发出去」的诊断 ——
   这个诊断在模拟器上是对的，但对一条**真实**总线来说是它推出来的。
   `scope_connect` 确实回了 `simulated: true`，工具没骗人；但**场景本身不完整**，
   而任务框定会把 Agent 往"这是真总线"的方向拽。

2. **README 给 P3 定的验收场景，模拟器做不到**。README 写的是
   「抓一次 I2C 写时序并告诉我为什么 NACK」，而 `waveform.rs` 的
   `transaction()` 对每个字节**无条件追加 ACK**，造不出 NACK。
   而 NACK 恰恰是 I2C 调试里最常要查的东西。

3. **三层 token 防护的代价**：结论里那句「累计只覆盖了约 0.15 秒」
   是真的 —— 8 KB 采集环决定了单窗口最长 4.8 ms。要长时间连续观察，
   得等设备侧的流模式（属 P1）。

---

## 6. 怎么复现

```bash
./host/run.sh build -p scope-mcp
export DEEPSEEK_API_KEY=sk-...        # 或 ANTHROPIC_API_KEY
python tools/agent_demo/agent.py
```

换模型 / 换供应商只要改两个参数（脚本走的是 Anthropic Messages 的形状）：

```bash
python tools/agent_demo/agent.py \
  --base-url https://api.anthropic.com/v1/messages --model claude-fable-5-1
```

换任务：

```bash
python tools/agent_demo/agent.py --task "总线上是不是有器件不响应？帮我确认一下。"
```

> ⚠ **这份实录里的模型输出会随模型、随机性变化。** 上面那次是
> `deepseek-flash` 在同一天的某一跑。换个模型来跑，「弯路」那一节大概率
> 会长得不一样 —— 那正是我们要看的东西。
