#!/usr/bin/env python3
"""Agent 演示：让一个 LLM 通过 MCP 原生工具调用驱动这台示波器。

# 这个脚本存在的原因

项目的 P3 验收标准是「**Agent 一句话完成一次真实的总线调试**」。
要证明这句话，需要三样东西同时在位：

1. **真的 LLM** —— 不是脚本里写死的调用序列
2. **真的 MCP 协议** —— `tools/list` 拿 schema、`tools/call` 执行，
   而不是把命令拼成字符串喂给管道
3. **可复现** —— 别人 clone 下来设个 key 就能重跑

第 2 条是关键。如果 Agent 是通过 shell 命令行去调工具的（`printf '...' | scope-mcp.exe`），
那它是在**自己拼 JSON、自己从 stdout 文本里读错误** —— schema 退化成了"文档"，
没有任何东西在边界上强制它。而 MCP 的意义正是让 schema 成为**契约**：
参数以结构化对象发出、由调用方按 schema 校验、结果以 `is_error` 标记好坏。

# 零依赖

只用标准库。`urllib` 发 HTTP、`subprocess` 起 MCP server、`json` 解协议。
这个项目在 Windows + GNU 工具链上跑，少一个依赖少一类环境问题。

# 用法

    export DEEPSEEK_API_KEY=...        # 或 ANTHROPIC_API_KEY
    python tools/agent_demo/agent.py --out docs/08-agent-walkthrough.md

见同目录的 README.md。
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

# ══════════════════════════════════════════════════════════════
# 配置
# ══════════════════════════════════════════════════════════════

#: 默认任务 —— 就是 README 里 P3 那一句「Agent 一句话完成…」的形状。
#: 刻意用工程师的口吻、不给任何工具提示：Agent 得自己从 `tools/list` 里找。
DEFAULT_TASK = (
    "我这条 I2C 总线上挂了个传感器，读它一直返回全 0，怀疑是总线本身有问题。"
    "你帮我看看总线上到底在发生什么，给我个结论。"
)

#: 一次会话最多几轮工具调用。
#:
#: 有上限是必须的 —— 轮数没有上限的话，一个钻牛角尖的模型能把 token 烧光。
#: 但撞到上限要**如实说出来**，不能假装它自己停了。
MAX_TURNS = 24

#: 单条工具结果最多往对话里塞多少字符。
#:
#: 三层 token 防护已经把 `capture` / `read_waveform` 的响应压得很小了，
#: 这里再兜一道是为了防意外（比如某个工具回了 4096 点全量样点）。
#: 截断会**显式标注**，不是悄悄砍掉。
RESULT_CHAR_CAP = 12_000


def env_key() -> tuple[str, str]:
    """返回 (api_key, 环境变量名)。两个名字都认，方便换供应商。"""
    for name in ("DEEPSEEK_API_KEY", "ANTHROPIC_API_KEY"):
        v = os.environ.get(name, "").strip()
        if v:
            return v, name
    sys.exit(
        "没有找到 API key。请先设置环境变量（不要写进文件）：\n"
        "    export DEEPSEEK_API_KEY=...      # 或用 ANTHROPIC_API_KEY\n"
        "Windows PowerShell:\n"
        "    $env:DEEPSEEK_API_KEY = '...'"
    )


# ══════════════════════════════════════════════════════════════
# 最小 MCP 客户端
# ══════════════════════════════════════════════════════════════


class McpClient:
    """stdio 上的 MCP 客户端。

    MCP 在这条链路上是**换行分隔的 JSON-RPC 2.0**（不是 LSP 那种
    `Content-Length` 帧）。所以「发一条、收一行」就够了。

    这个类刻意做得极小 —— 它同时是这份演示的**一部分**：
    一个 MCP 客户端到底需要多少东西？答案是几十行。
    """

    def __init__(self, exe: str) -> None:
        # 转成绝对路径再起进程：相对路径跟随调用方的 cwd，
        # 而这个脚本会被从仓库根目录、从 tools/ 目录、从 CI 里分别调用。
        exe = os.path.abspath(exe)
        if not os.path.isfile(exe):
            sys.exit(
                f"找不到 MCP server：{exe}\n"
                "先构建：  ./host/run.sh build -p scope-mcp"
            )
        self.proc = subprocess.Popen(
            [exe],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            # 文本模式 + UTF-8。**不能用默认编码** —— Windows 控制台是 GBK，
            # 中文会变成乱码，json.loads 直接失败（这个坑项目里踩过好几次）。
            text=True,
            encoding="utf-8",
            bufsize=1,  # 行缓冲
        )
        self._id = 0

    def _rpc(self, method: str, params: dict | None = None) -> dict:
        self._id += 1
        req = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            req["params"] = params
        assert self.proc.stdin and self.proc.stdout
        self.proc.stdin.write(json.dumps(req, ensure_ascii=False) + "\n")
        self.proc.stdin.flush()

        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("MCP server 没有响应就退出了（进程死了？）")
        resp = json.loads(line)
        if "error" in resp:
            raise RuntimeError(f"MCP 协议错误：{resp['error']}")
        return resp["result"]

    def handshake(self) -> dict:
        """`initialize` —— 拿到 serverInfo 与协议版本。"""
        return self._rpc(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "agent_demo", "version": "0.1.0"},
            },
        )

    def tools(self) -> list[dict]:
        """`tools/list` —— 拿到工具表。

        返回的是**原样的 MCP schema**（`inputSchema` 字段名也是 MCP 的）。
        转成各家 LLM 的方言是调用方的事，见 `to_llm_tools()`。
        """
        return self._rpc("tools/list")["tools"]

    def call(self, name: str, args: dict) -> tuple[str, bool]:
        """`tools/call` —— 返回 (文本, 是不是错误)。

        MCP 把工具的**业务失败**包在 `content` 里、用 `isError` 标记，
        而不是走 JSON-RPC 的 `error` 字段。这个区分很重要：
        前者是「工具跑完了，但它说这事没成」，后者是「协议层就坏了」。
        """
        result = self._rpc("tools/call", {"name": name, "arguments": args})
        blocks = result.get("content") or []
        text = "\n".join(b.get("text", "") for b in blocks if b.get("type") == "text")
        return text, bool(result.get("isError"))

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def to_llm_tools(mcp_tools: list[dict]) -> list[dict]:
    """MCP 工具表 → Anthropic Messages API 的 `tools` 字段。

    **唯一的改动是字段名**：`inputSchema` → `input_schema`。
    schema 本体一个字节不动 —— 这正是我们要证明的事：
    `params.rs` 里那个 Rust 类型生成的 schema，可以被原样交给模型用。
    """
    return [
        {"name": t["name"], "description": t["description"], "input_schema": t["inputSchema"]}
        for t in mcp_tools
    ]


# ══════════════════════════════════════════════════════════════
# 最小 LLM 客户端
# ══════════════════════════════════════════════════════════════


class Llm:
    """Messages API 客户端。

    走的是 **Anthropic Messages 的形状**（`content` 是块数组、工具调用是
    `tool_use` 块、结果回 `tool_result` 块）。

    这个形状值得选，是因为 `tool_use.input` 是**结构化对象**而不是字符串 ——
    模型不需要"拼"参数，调用方也不需要"解析"参数。
    换成 OpenAI 那种 `function.arguments` 是 JSON 字符串的方言，
    边界上就多了一层序列化/反序列化，那层正是出错的地方。

    默认指向 DeepSeek 的 Anthropic 兼容端点；换 `--base-url` 与 `--model`
    就能打到 api.anthropic.com，代码一行不改。
    """

    def __init__(self, key: str, base_url: str, model: str) -> None:
        self.key = key
        self.base_url = base_url
        self.model = model
        #: 累计用量 —— 最后要如实报出来，这是"这次演示花了多少"的唯一凭据。
        self.usage = {"input_tokens": 0, "output_tokens": 0, "calls": 0}

    def send(self, messages: list[dict], tools: list[dict]) -> tuple[dict, float]:
        body = {
            "model": self.model,
            "max_tokens": 8192,
            "tools": tools,
            "messages": messages,
        }
        req = urllib.request.Request(
            self.base_url,
            data=json.dumps(body, ensure_ascii=False).encode("utf-8"),
            headers={
                "content-type": "application/json",
                "x-api-key": self.key,
                "anthropic-version": "2023-06-01",
            },
            method="POST",
        )
        t0 = time.time()
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                data = json.loads(r.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            detail = e.read().decode("utf-8", "replace")[:600]
            raise RuntimeError(f"API 报错 HTTP {e.code}：{detail}") from None
        dt = time.time() - t0

        u = data.get("usage") or {}
        self.usage["input_tokens"] += u.get("input_tokens", 0)
        self.usage["output_tokens"] += u.get("output_tokens", 0)
        self.usage["calls"] += 1
        return data, dt


# ══════════════════════════════════════════════════════════════
# 主循环
# ══════════════════════════════════════════════════════════════


def blocks_of(resp: dict) -> list[dict]:
    c = resp.get("content")
    return c if isinstance(c, list) else []


def text_of(resp: dict) -> str:
    return "".join(b.get("text", "") for b in blocks_of(resp) if b.get("type") == "text")


def truncate(s: str, cap: int = RESULT_CHAR_CAP) -> str:
    """超长就截断，但**显式标注**砍掉了多少 —— 静默截断会让模型以为自己看到了全部。"""
    if len(s) <= cap:
        return s
    return s[:cap] + f"\n…（已截断：原文 {len(s)} 字符，此处保留前 {cap} 字符）"


def run(exe: str, llm: Llm, task: str, log) -> dict:
    mcp = McpClient(exe)
    trace: list[dict] = []
    try:
        info = mcp.handshake()
        log(f"MCP 握手完成：{info['serverInfo']['name']} {info['serverInfo']['version']}")

        tools = mcp.tools()
        log(f"tools/list 拿到 {len(tools)} 个工具")

        messages = [{"role": "user", "content": task}]
        turns = 0
        stop_reason = "end_turn"
        final_text = ""

        while turns < MAX_TURNS:
            turns += 1
            resp, dur = llm.send(messages, to_llm_tools(tools))
            stop_reason = resp.get("stop_reason", "?")
            assistant_blocks = blocks_of(resp)
            messages.append({"role": "assistant", "content": assistant_blocks})

            thinking = "".join(
                b.get("thinking", "") for b in assistant_blocks if b.get("type") == "thinking"
            )
            said = text_of(resp)
            calls = [b for b in assistant_blocks if b.get("type") == "tool_use"]

            trace.append(
                {
                    "turn": turns,
                    "thinking": thinking,
                    "text": said,
                    "tool_calls": [
                        {"name": c["name"], "input": c["input"], "id": c["id"]} for c in calls
                    ],
                    "results": [],
                    "stop_reason": stop_reason,
                    "seconds": round(dur, 1),
                }
            )
            log(f"--- 第 {turns} 轮：stop={stop_reason}，工具调用 {len(calls)} 个（{dur:.1f}s）")

            if not calls:
                final_text = said  # 模型给出了最终答复
                break

            results = []
            for c in calls:
                text, is_err = mcp.call(c["name"], c["input"])
                trace[-1]["results"].append(
                    {"name": c["name"], "is_error": is_err, "text": text}
                )
                # 纯 ASCII 标记：Windows 控制台是 GBK，对勾/叉号会印成 ✓
                mark = "[ERR]" if is_err else "[ok] "
                log(f"    {mark} {c['name']}({json.dumps(c['input'], ensure_ascii=False)})")
                results.append(
                    {
                        "type": "tool_result",
                        "tool_use_id": c["id"],
                        "content": truncate(text),
                        "is_error": is_err,
                    }
                )
            messages.append({"role": "user", "content": results})
        else:
            log(f"⚠ 撞到 {MAX_TURNS} 轮上限，是脚本强行停下的，不是模型自己停的")

        return {
            "trace": trace,
            "usage": llm.usage,
            "turns": turns,
            "hit_turn_limit": turns >= MAX_TURNS and stop_reason == "tool_use",
            "final_text": final_text,
            "server": info,
            "tool_count": len(tools),
        }
    finally:
        mcp.close()


def log_console(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def main() -> int:
    ap = argparse.ArgumentParser(description="用 LLM 的原生工具调用驱动 MCP 示波器")
    ap.add_argument("--exe", default="host/target/debug/scope-mcp.exe", help="MCP server 可执行文件")
    ap.add_argument("--base-url", default=os.environ.get("AGENT_BASE_URL", "https://api.deepseek.com/anthropic/v1/messages"))
    ap.add_argument("--model", default=os.environ.get("AGENT_MODEL", "deepseek-flash"))
    ap.add_argument("--task", default=DEFAULT_TASK)
    ap.add_argument("--dump", default="tools/agent_demo/last_run.json", help="把完整轨迹写到这里")
    args = ap.parse_args()

    key, keyname = env_key()
    log_console(f"用 {keyname} 驱动 {args.model} @ {args.base_url}")

    out = run(args.exe, Llm(key, args.base_url, args.model), args.task, log_console)

    os.makedirs(os.path.dirname(args.dump) or ".", exist_ok=True)
    with open(args.dump, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)
    log_console(f"完整轨迹 → {args.dump}")

    print(json.dumps({"turns": out["turns"], "usage": out["usage"], "hit_turn_limit": out["hit_turn_limit"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
