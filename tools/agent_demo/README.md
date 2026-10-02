# agent_demo —— Agent 能不能真的用起来这套工具

> 这个目录存在的唯一理由：**证明「Agent 一句话完成一次总线调试」不是一句宣传语。**
>
> 它产出的东西是 [`docs/08-agent-walkthrough.md`](../../docs/08-agent-walkthrough.md) ——
> 一份**真实跑出来**的对话实录。

---

## 它和「用命令行调工具」的区别

这件事值得说清楚，因为两种做法看起来都能跑，但证明的东西不一样。

|  | 命令行驱动 | **本工具（原生工具调用）** |
|---|---|---|
| 模型发出什么 | 一段**字符串**：`printf '...' \| scope-mcp.exe` | 一个**结构化对象**：`{"name": ..., "input": {...}}` |
| 参数谁校验 | 没人 —— 模型自己拼 JSON，拼错了也是执行了才知道 | 调用方按 `tools/list` 的 schema 校验 |
| 结果怎么回来 | stdout 的**文本**，模型自己从里面读错误 | 结构化 `tool_result`，带 `is_error` 标志 |
| schema 的性质 | 一份**说明**（模型读它，但它不被强制） | 一份**契约**（边界上有人执行它） |

差别不在效率，在于**前者会把 schema 的问题掩盖掉**：模型在替 schema 干活
（自己拼参数、自己解析错误文本），于是「声明和实现之间的缝」被调用方的努力填上了，
你看不出这套工具设计得好不好。

---

## 怎么跑

```bash
# 1) 先构建 MCP server
./host/run.sh build -p scope-mcp

# 2) 设 key（**不要写进任何文件**）
export DEEPSEEK_API_KEY=sk-...        # 或 ANTHROPIC_API_KEY

# 3) 跑
python tools/agent_demo/agent.py
```

跑完会打印每轮的工具调用，完整轨迹落在 `last_run.json`（已 gitignore）。

### 换供应商 / 换模型

脚本走的是 **Anthropic Messages 的形状**（`tool_use` / `tool_result` 块），
`tools` 字段直接就是 `tools/list` 的产物、只把 `inputSchema` 改成 `input_schema`。
所以换一家只要改两个参数，代码一行不动：

```bash
python tools/agent_demo/agent.py \
  --base-url https://api.anthropic.com/v1/messages \
  --model claude-fable-5-1
```

### 换任务

```bash
python tools/agent_demo/agent.py --task "总线上是不是有器件不响应？帮我确认一下。"
```

任务**刻意不给任何工具提示** —— Agent 得自己从 `tools/list` 里找。
给了提示，就测不出「schema 写得够不够清楚」这件事了。

---

## 依赖

**零个。** 只用标准库：`urllib` 发 HTTP、`subprocess` 起 MCP server、`json` 解协议。

这个项目在 Windows + GNU 工具链上跑，少一个依赖少一类环境问题。
顺带也说明一件事：**一个 MCP 客户端要多少代码？答案是一百多行。**

---

## 已知的局限

- **只处理文本**。MCP 的 `content` 可以是图片/资源，这里只取 `type == "text"` 的块。
  这台设备的工具全部返回 JSON 文本，够用。
- **一次只发一条请求**。MCP 允许并发（靠 id 配对），这里"发一条收一行"，
  因为工具调用循环本来就是串行的。
- **`thinking` 块原样回传**。Anthropic 兼容端点会返回带 `signature` 的
  thinking 块，多轮对话里必须原样带回，否则有的实现会拒绝。
- **轮数上限 24 轮**。撞上限会在日志里**明说**是脚本强行停的、不是模型自己停的。
