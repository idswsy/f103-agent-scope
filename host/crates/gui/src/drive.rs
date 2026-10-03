//! 「让 AI 自己配置并采集」—— GUI 把设备让给 `scope-mcp` 子进程，自己当 MCP 客户端。
//!
//! # 为什么是子进程而不是进程内
//!
//! 真机串口**独占**（`docs/07-dev-env.md:232`：GUI 占着 COM7 时 `scope-mcp`
//! 打不开同一个口），模拟器则每个进程各建一台（`Transport::sim()`）。
//! 所以同一时刻只能有一个设备持有者 —— 要么 GUI，要么子进程。
//!
//! 选子进程，是因为它**复用整套已测过的 MCP 工具层**（14 个工具、schema、
//! 越界校验、三层 token 防护）。进程内重做要先把这个 bin-only 的 crate 拆成 lib，
//! 而且很容易长出第二份策略 —— 那正是本项目反复打的病。
//!
//! # 这个模块管什么
//!
//! 三件事，按依赖顺序：
//!
//! 1. **找 `scope-mcp` 可执行文件** —— 按 `current_exe()` 同级找，
//!    不按 cwd（GUI 的 cwd 与仓库根不同）
//! 2. **stdio JSON-RPC 客户端** —— 起进程、发请求、读响应，外加把所有
//!    「看起来能跑但会卡死」的地方堵上（见 [`McpProcess`] 的说明）
//! 3. **工具循环** —— 移植自 `tools/agent_demo/agent.py`，那是一个已经跑通的
//!    参考实现；但它有几处做法**不能照抄**，逐条写在 [`run_session`] 上面
//!
//! # 显示由 GUI 自己镜像，不用 AI 的读数
//!
//! AI 可能只看统计量**根本不读样点**（那屏幕就是空白的）；真读时 4096 点的
//! JSON 又会被 [`RESULT_CHAR_CAP`] 截断，半截样点喂进上下文 ——
//! **既烧 token 又误导模型**。
//!
//! 所以镜像由 GUI 自己做：每轮工具执行完调一次 `scope_list_captures`，
//! 对没见过的编号自己发 `scope_read_waveform` 拉全通道，重建 [`Capture`] 上屏。
//! **这些调用绝不进 LLM 的消息。**

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use scope_core::Capture;
use serde_json::{json, Value};

use crate::ai::AiError;
use crate::config::AiConfig;

/// 单次工具结果喂给模型时的字符上限。
///
/// 与 `tools/agent_demo/agent.py` 的 `RESULT_CHAR_CAP` 一致。
///
/// ⚠ 截断**必须带说明**。静默截断会让模型以为「数据就这么多」，
/// 而它可能正据此下一个「没有异常」的结论。
pub const RESULT_CHAR_CAP: usize = 12_000;

/// 工具循环的轮数上限。与 `agent.py` 的 `MAX_TURNS` 一致。
pub const MAX_TURNS: usize = 24;

/// 回复长度上限。
pub const MAX_TOKENS: u32 = 8192;

/// 单次 MCP 请求的等待上限。
///
/// ⚠ **`agent.py` 这里是没有超时的**（阻塞死等）。照抄的话，子进程一旦卡住
/// 就再也回不来 —— 而它跑在一条专用线程上，用户只能看着界面转圈。
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// 单次 LLM 请求的超时。
pub const LLM_TIMEOUT: Duration = Duration::from_secs(180);

/// 子进程优雅退出的等待上限；超过就 kill。
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// 找 `scope-mcp` 可执行文件。
///
/// **按 `current_exe()` 同级找，不按 cwd** —— GUI 的 cwd 是仓库根，
/// 而可执行文件在 `target/debug/`（`run.sh` 还可能把它重定向到别处）。
/// `agent.py` 是靠列一串候选路径解决的，那在 GUI 里没必要：
/// 两个 exe 是 `cargo` 一起放进同一个目录的。
pub fn find_scope_mcp() -> Result<PathBuf, AiError> {
    let me = std::env::current_exe().map_err(|e| {
        AiError::new(
            format!("取不到自身可执行文件的位置（{e}）"),
            "这是程序缺陷，请提交 issue",
        )
    })?;
    let dir = me.parent().ok_or_else(|| {
        AiError::new(
            "自身可执行文件没有上级目录".to_string(),
            "这是程序缺陷，请提交 issue",
        )
    })?;

    let name = if cfg!(windows) {
        "scope-mcp.exe"
    } else {
        "scope-mcp"
    };

    // 找两处：
    //   1. 同目录 —— 正常运行时的情况（`cargo` 把两个 exe 放进同一个目录）
    //   2. **上一级** —— 测试二进制跑在 `target/debug/deps/` 下，
    //      它的同目录只有测试产物，真正的 exe 在上一级
    //
    // 第 2 条不是补丁：`--drive` 这条命令行入口正是靠它才能在
    // `cargo test` 里被验到（否则只能手工起窗口试）。
    let mut tried = Vec::new();
    for d in [Some(dir), dir.parent()].into_iter().flatten() {
        let candidate = d.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        tried.push(candidate);
    }

    Err(AiError::new(
        format!(
            "找不到 {}",
            tried
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" 或 ")
        ),
        "与本程序放在同一目录即可（通常是 host/target/debug/）—— \
         用 ./host/run.sh run -p scope-gui 启动会自动放好",
    ))
}

// ══════════════════════════════════════════════════════════════════
// 纯函数（CI 可测）
// ══════════════════════════════════════════════════════════════════

/// 解析子进程的一行输出。
///
/// 返回 `None` 表示**这行不是 JSON，应当跳过**。
///
/// ⚠ `agent.py` 不跳 —— 它直接 `json.loads`，于是子进程往 stdout 打一行日志
/// 就会抛 JSONDecodeError 把整个会话打断。而「往 stdout 打日志」是子进程
/// 完全可能做的事（本项目就有 `SCOPE_MCP_DEBUG`）。
pub fn parse_line(line: &str) -> Option<Value> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(t).ok()
}

/// 校验响应里的 `id` 是不是我们等的那一条。
///
/// 没有 `id` 的（通知）直接跳过；有 `id` 但对不上的**报错** ——
/// 我们一次只发一条请求，出现别的 id 就是协议出问题了，静默收下会让
/// 调用方拿着别人的结果当自己的。
pub fn check_id(resp: &Value, want: u64) -> Result<bool, AiError> {
    match resp.get("id") {
        None => Ok(false), // 通知，跳过
        Some(v) => {
            if v.as_u64() == Some(want) {
                Ok(true)
            } else {
                Err(AiError::new(
                    format!("响应 id 对不上：等 {want}，收到 {v}"),
                    "子进程与客户端不同步了 —— 终止本次会话重来",
                ))
            }
        }
    }
}

/// 把工具结果截到 [`RESULT_CHAR_CAP`]，**带说明**。
pub fn truncate_for_llm(text: &str) -> String {
    let n = text.chars().count();
    if n <= RESULT_CHAR_CAP {
        return text.to_string();
    }
    let head: String = text.chars().take(RESULT_CHAR_CAP).collect();
    format!("{head}\n…（已截断：原文 {n} 字符，此处保留前 {RESULT_CHAR_CAP} 字符）")
}

/// 从 `scope_list_captures` 的一条记录重建 `Capture` 的**非样点部分**。
///
/// 样点由 `scope_read_waveform` 另外拉 —— 那是全量数据，不进模型上下文。
pub fn rebuild_capture(meta: &Value, channels: Vec<Vec<u16>>) -> Result<Capture, AiError> {
    let bad = |what: &str| {
        AiError::new(
            format!("list_captures 的记录里缺 {what} —— 无法重建采集"),
            "这是程序缺陷或版本不匹配，请提交 issue",
        )
    };
    let id = meta["capture_id"]
        .as_u64()
        .ok_or_else(|| bad("capture_id"))? as u16;
    let rate_hz = meta["rate_hz"].as_u64().ok_or_else(|| bad("rate_hz"))? as u32;
    let expected = meta["expected_samples"]
        .as_u64()
        .ok_or_else(|| bad("expected_samples"))? as u32;

    let mut cap = Capture::new(id, rate_hz, channels.len().max(1), expected);
    cap.channels = channels;
    cap.trigger_index = meta["trigger_index"].as_u64().map(|v| v as u32);
    cap.device_tick_us = meta["device_tick_us"].as_u64().unwrap_or(0) as u32;
    cap.wall_time = meta["wall_time"].as_u64().unwrap_or(0);
    cap.overrun = meta["overrun"].as_bool().unwrap_or(false);
    Ok(cap)
}

// ══════════════════════════════════════════════════════════════════
// 子进程 + JSON-RPC
// ══════════════════════════════════════════════════════════════════

/// 一个 `scope-mcp` 子进程，加一条 stdio JSON-RPC 通道。
///
/// # 与 `agent.py` 的三处不同（都是「看起来能跑但会卡死」的坑）
///
/// 1. **stderr 要排空。** `agent.py` 把它接进管道却从不读 —— 子进程往 stderr
///    写够一个管道缓冲就**永久阻塞**。这里单开一条线程读掉。
/// 2. **读要有超时。** `agent.py` 是阻塞死等。这里把 stdout 交给一条读线程，
///    请求方 `recv_timeout` —— 子进程卡住时能报错而不是陪着一起卡。
/// 3. **非法行要跳过、id 要校验**（见 [`parse_line`] / [`check_id`]）。
pub struct McpProcess {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpProcess {
    /// 起一个子进程。
    pub fn spawn(exe: &PathBuf) -> Result<McpProcess, AiError> {
        let fail = |e: std::io::Error| {
            AiError::new(
                format!("起不了 scope-mcp（{e}）"),
                "确认它与本程序在同一目录、且有执行权限",
            )
        };

        let mut child = Command::new(exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(fail)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| fail(std::io::Error::other("no stdin")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| fail(std::io::Error::other("no stdout")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| fail(std::io::Error::other("no stderr")))?;

        // stdout → 通道。读线程随子进程退出而结束（read_line 返回 0）。
        let (tx, lines) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("scope-mcp-stdout".into())
            .spawn(move || {
                let mut r = BufReader::new(stdout);
                loop {
                    let mut s = String::new();
                    match r.read_line(&mut s) {
                        Ok(0) | Err(_) => break, // EOF 或出错：子进程没了
                        Ok(_) => {
                            if tx.send(s).is_err() {
                                break; // 接收端没了
                            }
                        }
                    }
                }
            })
            .map_err(fail)?;

        // stderr **必须读掉**，否则子进程写满管道缓冲就永久阻塞。
        // 内容丢弃（那是诊断信息，界面上不显示；真要看可以开 SCOPE_MCP_DEBUG）。
        std::thread::Builder::new()
            .name("scope-mcp-stderr".into())
            .spawn(move || {
                let mut r = BufReader::new(stderr);
                let mut s = String::new();
                while matches!(r.read_line(&mut s), Ok(n) if n > 0) {
                    s.clear();
                }
            })
            .map_err(fail)?;

        Ok(McpProcess {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            lines,
            next_id: 1,
        })
    }

    fn send(&mut self, msg: &Value) -> Result<(), AiError> {
        let mut line = serde_json::to_string(msg).map_err(|e| {
            AiError::new(format!("序列化请求失败：{e}"), "属程序缺陷，请提交 issue")
        })?;
        line.push('\n');
        let mut w = self.stdin.lock().unwrap();
        w.write_all(line.as_bytes())
            .and_then(|_| w.flush())
            .map_err(|e| {
                AiError::new(
                    format!("写给 scope-mcp 失败：{e}"),
                    "子进程可能已经退出了 —— 终止本次会话重来",
                )
            })
    }

    /// 发一条请求并等它的响应。
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, AiError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;

        let deadline = Instant::now() + RPC_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(AiError::new(
                        format!("等 {method} 的响应超时（{} 秒）", RPC_TIMEOUT.as_secs()),
                        "子进程卡住了 —— 终止本次会话重来",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(AiError::new(
                        "scope-mcp 没有响应就退出了".to_string(),
                        "它可能崩溃了 —— 终止本次会话后重试",
                    ));
                }
            };

            let Some(resp) = parse_line(&line) else {
                continue; // 不是 JSON —— 跳过（子进程可能打了日志）
            };
            if !check_id(&resp, id)? {
                continue; // 通知
            }
            if let Some(err) = resp.get("error") {
                return Err(AiError::new(
                    format!("{method} 返回错误：{err}"),
                    "协议层出错 —— 终止本次会话重来",
                ));
            }
            return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 优雅收场：让子进程自己断开设备，再关 stdin 等它退出，超时才 kill。
    ///
    /// **顺序不能反**：先杀的话，真机上设备可能停在 Armed ——
    /// 而 GUI 随后要把它连回来。
    ///
    /// **kill 之后必须 `wait()`**：Windows 上进程句柄没释放就重连，
    /// 会偶发「端口被占用」。
    pub fn shutdown(mut self) {
        // 1) 请它自己断开（它内部会 bus.stop()，真机回到 Idle）
        let _ = self.call(
            "tools/call",
            json!({"name": "scope_disconnect", "arguments": {}}),
        );

        // 2) 关 stdin —— 子进程读到 EOF 会自己退出
        if let Ok(mut w) = self.stdin.lock() {
            let _ = w.flush();
        }
        drop(self.stdin);

        // 3) 等它退出；超时才杀
        let child = Arc::clone(&self.child);
        let Ok(mut c) = child.lock() else { return };
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match c.try_wait() {
                Ok(Some(_)) => return, // 自己退了
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    let _ = c.kill();
                    let _ = c.wait(); // ← 必须等，否则句柄没释放
                    return;
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 镜像：GUI 自己把 AI 采的波拉回来上屏
// ══════════════════════════════════════════════════════════════════

/// 已经镜像过的采集编号 —— 用来判断「这一窗是不是新的」。
#[derive(Default)]
pub struct Mirrored {
    seen: BTreeSet<u16>,
}

impl Mirrored {
    /// 扫一遍子进程的采集列表，把**没见过的**拉回样点、重建成 `Capture`。
    ///
    /// 返回这一轮新出现的采集（可能为空）。
    ///
    /// **这些调用绝不进 LLM 的消息** —— 它们是「给人看的」，
    /// 让模型看见只会白烧 token，而且 4096 点还会被截断成半截。
    pub fn poll(&mut self, mcp: &mut McpProcess) -> Result<Vec<Capture>, AiError> {
        let list = mcp.call(
            "tools/call",
            json!({"name": "scope_list_captures", "arguments": {}}),
        )?;
        let text = tool_text(&list);

        // ⚠ 解析失败**必须报错**，不能返回空列表。
        //
        // 第一版这里是 `return Ok(Vec::new())` —— 于是「拉不到波形」与
        // 「确实还没有采集」表现完全一样：屏幕空白，而没有任何线索。
        // 静默降级成「什么也没有」正是本项目反复打的那类病。
        let parsed = parse_tool_json(&text).ok_or_else(|| {
            AiError::new(
                format!("list_captures 返回的不是 JSON：{}", first_line(&text)),
                "子进程与客户端版本可能不匹配 —— 重新构建后重试",
            )
        })?;
        let rows = parsed["captures"].as_array().ok_or_else(|| {
            AiError::new(
                format!(
                    "list_captures 的返回里没有 captures 数组：{}",
                    first_line(&text)
                ),
                "子进程与客户端版本可能不匹配 —— 重新构建后重试",
            )
        })?;

        let mut fresh = Vec::new();
        for meta in rows {
            let Some(id) = meta["capture_id"].as_u64() else {
                continue;
            };
            let id = id as u16;
            if self.seen.contains(&id) {
                continue;
            }
            let nch = meta["channels"].as_u64().unwrap_or(1).max(1) as usize;
            let mut channels = Vec::with_capacity(nch);
            for ch in 0..nch {
                let r = mcp.call(
                    "tools/call",
                    json!({
                        "name": "scope_read_waveform",
                        "arguments": { "capture_id": id, "channel": ch, "count": 4096 },
                    }),
                )?;
                channels.push(read_samples(&tool_text(&r)));
            }
            self.seen.insert(id);
            fresh.push(rebuild_capture(meta, channels)?);
        }
        Ok(fresh)
    }
}

/// `tools/call` 的返回形如 `{content:[{type:"text",text:"…"}]}`，取其中文本。
pub fn tool_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 工具返回的文本里嵌着一层 JSON，解出来。
pub fn parse_tool_json(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text.trim()).ok()
}

/// 从 `scope_read_waveform` 的返回里取样点。
pub fn read_samples(text: &str) -> Vec<u16> {
    parse_tool_json(text)
        .and_then(|v| {
            v.get("samples").and_then(|s| s.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_u64())
                    .map(|x| x.min(4095) as u16)
                    .collect()
            })
        })
        .unwrap_or_default()
}

// ══════════════════════════════════════════════════════════════════
// 会话
// ══════════════════════════════════════════════════════════════════

/// 交给 AI 的一次会话。
pub struct DriveJob {
    /// 用户的自然语言需求。
    pub task: String,
    /// 密钥 / 端点 / 模型（与单发分析共用一份配置）。
    pub cfg: AiConfig,
    /// **钉死的** `scope_connect` 参数。
    ///
    /// AI 调 `scope_connect` 时，参数由这里覆盖 —— 不让它自己挑设备。
    /// 否则它可能连到别的口上去，而 GUI 手上那份「交接快照」就对不上了。
    pub connect_args: Value,
}

/// 会话过程中报给界面的事件。
///
/// ⚠ **不是聊天记录**，是动作轨迹。用户要能看出「AI 改了哪个参数、采了哪几窗」——
/// 因为在示波器上选错时基/触发电平会得到一份**看起来合理**的新波形，
/// 错误是沉默的。
#[derive(Debug)]
pub enum DriveEvent {
    /// 阶段变化（起进程、握手、跑循环…）。
    Phase(String),
    /// AI 决定调用某个工具。
    ToolCall {
        /// 工具名。
        name: String,
        /// 参数的紧凑描述（给人看）。
        args: String,
    },
    /// 工具执行完了。
    ToolResult {
        /// 工具名。
        name: String,
        /// 业务上成功了没有。
        ok: bool,
        /// 一句话摘要（给人看）。
        summary: String,
    },
    /// 工具返回里带的警告（例如采样率被量化、SW2/SW3 是手拨开关）。
    Warnings(Vec<String>),
    /// 子进程采到了新的一窗 —— GUI 已经替它拉回样点并重建。
    Capture(Box<Capture>),
    /// 正常结束。
    Finished {
        /// 最终回复。
        text: String,
        /// 实际跑了几轮。
        turns: usize,
        /// 是不是撞了轮数上限被迫停的。
        hit_limit: bool,
    },
}

/// 跑一次完整会话。**阻塞**，只能在专用线程里调。
///
/// # 与 `tools/agent_demo/agent.py` 的关系
///
/// 循环的骨架是从它移植的（那是唯一一个已经跑通 MCP 工具循环的参考实现），
/// 但有几处**故意不同**：
///
/// | `agent.py` | 这里 | 为什么 |
/// |---|---|---|
/// | stderr 接了不读 | 单开线程排空 | 不排空子进程会阻塞在写 stderr 上 |
/// | 无读超时 | [`RPC_TIMEOUT`] | 卡住要能报错，不能陪着一起卡 |
/// | 不校验 id、不跳非 JSON 行 | 都做 | 子进程打一行日志就会把会话打断 |
/// | 结果由模型决定读什么 | **GUI 自己镜像** | 见模块头 —— 模型可能不读样点 |
///
/// 轮数的终止条件仍是「有没有 `tool_use`」—— 与它一致（**不看 `stop_reason`**）。
pub fn run_session(
    job: &DriveJob,
    mcp: &mut McpProcess,
    emit: &mut dyn FnMut(DriveEvent),
) -> Result<(), AiError> {
    emit(DriveEvent::Phase("正在初始化 MCP 会话".into()));

    // 与 agent.py 一致：只 initialize + tools/list，**不发 initialized 通知**
    // （服务端不要求，且发了也没人读）。
    mcp.call(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "scope-gui", "version": scope_core::VERSION },
        }),
    )?;

    let tools_resp = mcp.call("tools/list", json!({}))?;
    let tools = to_llm_tools(&tools_resp);
    let tool_count = tools.as_array().map(|a| a.len()).unwrap_or(0);

    // 连设备。**参数由 GUI 钉死** —— 不让 AI 自己挑目标。
    emit(DriveEvent::Phase("正在连接设备".into()));
    mcp.call(
        "tools/call",
        json!({ "name": "scope_connect", "arguments": job.connect_args }),
    )?;

    emit(DriveEvent::Phase(format!(
        "开始（{tool_count} 个工具可用）"
    )));

    let mut messages = vec![json!({ "role": "user", "content": job.task })];
    let mut mirrored = Mirrored::default();

    for turn in 0..MAX_TURNS {
        let body = json!({
            "model": job.cfg.model,
            "max_tokens": MAX_TOKENS,
            "tools": tools,
            "messages": messages,
        });
        let resp = post_llm(&job.cfg, &body)?;
        let blocks = resp
            .get("content")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();

        // assistant 的 content 块**原样回传** —— thinking 块带的 signature
        // 必须一起带回去，模型才认得出自己的思考。
        messages.push(json!({ "role": "assistant", "content": blocks }));

        let calls: Vec<&Value> = blocks
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            .collect();

        if calls.is_empty() {
            // 没有工具调用 = 它答完了
            let text = blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            emit(DriveEvent::Finished {
                text,
                turns: turn + 1,
                hit_limit: false,
            });
            return Ok(());
        }

        let mut results = Vec::new();
        for c in calls {
            let name = c
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("?")
                .to_string();
            let call_id = c
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();
            let mut arguments = c.get("input").cloned().unwrap_or(json!({}));

            // 把 scope_connect 钉死在 GUI 交接的那个目标上
            if name == "scope_connect" {
                arguments = job.connect_args.clone();
            }

            emit(DriveEvent::ToolCall {
                name: name.clone(),
                args: summarize_args(&arguments),
            });

            let (text, is_err) = match mcp.call(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            ) {
                Ok(r) => {
                    let t = tool_text(&r);
                    let e = r.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                    (t, e)
                }
                Err(e) => (format!("{}｜{}", e.message, e.hint), true),
            };

            // 工具返回里可能带 warnings（采样率量化、手拨开关…）—— 必须上屏
            if let Some(w) = parse_tool_json(&text).and_then(|v| {
                v.get("warnings").and_then(|w| w.as_array()).map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            }) {
                emit(DriveEvent::Warnings(w));
            }

            emit(DriveEvent::ToolResult {
                name: name.clone(),
                ok: !is_err,
                summary: first_line(&text),
            });

            // —— GUI 自己镜像：把新采的那一窗拉回来上屏 ——
            // 这一步的花费**不进** messages。
            match mirrored.poll(mcp) {
                Ok(caps) => {
                    for cap in caps {
                        emit(DriveEvent::Capture(Box::new(cap)));
                    }
                }
                Err(e) => {
                    // 镜像失败不该打断 AI 的活 —— 记一句，继续
                    emit(DriveEvent::ToolResult {
                        name: "镜像".into(),
                        ok: false,
                        summary: format!("拉回波形失败：{}", e.message),
                    });
                }
            }

            results.push(json!({
                "type": "tool_result",
                "tool_use_id": call_id,
                "content": truncate_for_llm(&text),
                "is_error": is_err,
            }));
        }

        // 一次调用的全部结果**塞进同一条 user 消息**（与 agent.py 一致）
        messages.push(json!({ "role": "user", "content": results }));
    }

    emit(DriveEvent::Finished {
        text: String::new(),
        turns: MAX_TURNS,
        hit_limit: true,
    });
    Ok(())
}

/// 把 MCP 的 `tools/list` 结果转成 LLM 要的形状。
///
/// **只改字段名** `inputSchema` → `input_schema`，其余原样 ——
/// 与 `agent.py` 的做法一致，那条路已经验证可用。
pub fn to_llm_tools(resp: &Value) -> Value {
    let tools: Vec<Value> = resp
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .map(|t| {
                    json!({
                        "name": t.get("name").cloned().unwrap_or(Value::Null),
                        "description": t.get("description").cloned().unwrap_or(Value::Null),
                        "input_schema": t.get("inputSchema").cloned().unwrap_or(json!({"type":"object"})),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Value::Array(tools)
}

/// 发一次 LLM 请求。形状与 `ai.rs` 的单发一致，多一个 `tools`。
fn post_llm(cfg: &AiConfig, body: &Value) -> Result<Value, AiError> {
    if !cfg.has_key() {
        return Err(AiError::new(
            "未配置 API 密钥",
            "请在面板的「设置」中填写密钥",
        ));
    }
    let text = serde_json::to_string(body)
        .map_err(|e| AiError::new(format!("构造请求体失败：{e}"), "属程序缺陷，请提交 issue"))?;

    let result = ureq::post(cfg.base_url.trim())
        .header("content-type", "application/json")
        .header("x-api-key", cfg.api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .config()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(LLM_TIMEOUT))
        .build()
        .send(text.as_str());

    match result {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            if !(200..=299).contains(&status) {
                return Err(crate::ai::http_error_for_drive(status, &body));
            }
            serde_json::from_str::<Value>(&body).map_err(|e| {
                AiError::new(
                    format!("响应不是 JSON：{e}"),
                    "端点可能配错了 —— 确认 base_url 指向 messages 接口",
                )
            })
        }
        Err(e) => Err(crate::ai::transport_error_for_drive(&e)),
    }
}

/// 参数的紧凑描述（给人看，进轨迹）。
fn summarize_args(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= 120 {
        s
    } else {
        format!("{}…", s.chars().take(120).collect::<String>())
    }
}

/// 取第一行非空内容当摘要。
fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.chars().count() <= 120 {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(120).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 行解析 ───────────────────────────────────────────────────────

    #[test]
    fn non_json_lines_are_skipped_not_fatal() {
        // 回归风险：agent.py 直接 json.loads，子进程打一行日志就整个会话完蛋。
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("[scope-mcp] 正在启动…").is_none());
        assert!(parse_line("{\"partial\": ").is_none());

        // 正常的一行要能解出来
        assert!(parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).is_some());
        // 前后有空白也要认得
        assert!(parse_line("  {\"id\":1} \n").is_some());
    }

    #[test]
    fn a_mismatched_id_is_an_error_not_silently_accepted() {
        let want = 7;

        // 对得上
        assert!(check_id(&json!({"id": 7}), want).unwrap());
        // 通知（没有 id）→ 跳过，不算错
        assert!(!check_id(&json!({"method": "notifications/x"}), want).unwrap());
        // 对不上 → 报错。静默收下会让调用方拿着别人的结果当自己的。
        let e = check_id(&json!({"id": 8}), want).unwrap_err();
        assert!(
            e.message.contains("7") && e.message.contains("8"),
            "{}",
            e.message
        );
        assert!(!e.hint.trim().is_empty(), "错误必须带怎么办");
    }

    // ── 截断 ─────────────────────────────────────────────────────────

    #[test]
    fn truncation_always_says_so() {
        let short = "x".repeat(100);
        assert_eq!(truncate_for_llm(&short), short, "没超限不该动");

        let long = "x".repeat(RESULT_CHAR_CAP + 500);
        let out = truncate_for_llm(&long);
        assert!(out.contains("已截断"), "截断必须带说明");
        assert!(
            out.contains(&format!("{}", RESULT_CHAR_CAP + 500)),
            "要说清原文多长"
        );
        assert!(
            out.chars().count() <= RESULT_CHAR_CAP + 100,
            "截完还是太长：{}",
            out.chars().count()
        );
    }

    // ── 重建 Capture ─────────────────────────────────────────────────

    fn meta(id: u16) -> Value {
        json!({
            "capture_id": id, "rate_hz": 857_142, "sample_count": 4,
            "channels": 2, "duration_ms": 0.005,
            "trigger_index": 2, "overrun": true,
            "device_tick_us": 12345, "wall_time": 1_700_000_000,
            "expected_samples": 4096,
        })
    }

    #[test]
    fn a_rebuilt_capture_matches_every_field() {
        let cap = rebuild_capture(&meta(3), vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]]).unwrap();

        assert_eq!(cap.id, 3);
        assert_eq!(cap.rate_hz, 857_142);
        assert_eq!(cap.channels.len(), 2);
        assert_eq!(cap.channels[0], vec![1, 2, 3, 4]);
        assert_eq!(cap.trigger_index, Some(2));
        assert_eq!(cap.device_tick_us, 12345);
        assert_eq!(cap.wall_time, 1_700_000_000);
        assert!(cap.overrun);
        // ⚠ 这一条最要紧：拿不到它，report.rs 的缺口告警就永久静音了
        assert_eq!(
            cap.expected_samples, 4096,
            "expected_samples 必须是设备报的期望值，不是实际长度"
        );
        assert_ne!(
            cap.expected_samples,
            cap.channels[0].len() as u32,
            "测试数据要能区分「期望」与「实际」—— 否则这条断言是空的"
        );
    }

    #[test]
    fn missing_metadata_is_reported_not_defaulted() {
        let mut m = meta(1);
        m.as_object_mut().unwrap().remove("expected_samples");
        let e = rebuild_capture(&m, vec![vec![1]]).unwrap_err();
        assert!(e.message.contains("expected_samples"), "{}", e.message);
        assert!(!e.hint.trim().is_empty());
    }

    #[test]
    fn trigger_index_none_round_trips() {
        let mut m = meta(1);
        m["trigger_index"] = Value::Null;
        assert_eq!(
            rebuild_capture(&m, vec![vec![1]]).unwrap().trigger_index,
            None
        );
    }

    // ── 工具表转换 ───────────────────────────────────────────────────

    #[test]
    fn tools_list_is_renamed_not_reshaped() {
        let resp = json!({
            "tools": [{
                "name": "scope_capture",
                "description": "采集一次",
                "inputSchema": { "type": "object", "properties": { "count": { "type": "integer" } } },
            }]
        });
        let out = to_llm_tools(&resp);
        let t = &out.as_array().unwrap()[0];

        assert_eq!(t["name"], "scope_capture");
        assert_eq!(t["description"], "采集一次");
        assert_eq!(t["input_schema"]["type"], "object");
        assert_eq!(
            t["input_schema"]["properties"]["count"]["type"], "integer",
            "只改字段名，schema 内容必须原样"
        );
        assert!(t.get("inputSchema").is_none(), "旧名字不该留");
    }

    #[test]
    fn an_empty_tool_list_does_not_panic() {
        assert_eq!(to_llm_tools(&json!({})).as_array().unwrap().len(), 0);
        assert_eq!(
            to_llm_tools(&json!({"tools": []}))
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    // ── 工具返回的解析 ───────────────────────────────────────────────

    #[test]
    fn tool_text_joins_text_blocks_only() {
        let r = json!({ "content": [
            {"type": "thinking", "thinking": "内部推理"},
            {"type": "text", "text": "第一段"},
            {"type": "text", "text": "第二段"},
        ]});
        let t = tool_text(&r);
        assert!(t.contains("第一段") && t.contains("第二段"));
        assert!(!t.contains("内部推理"), "thinking 不该混进来");
    }

    #[test]
    fn samples_are_clamped_to_12_bits() {
        let text = r#"{"samples":[0, 4095, 99999]}"#;
        assert_eq!(read_samples(text), vec![0, 4095, 4095]);
    }

    #[test]
    fn unparsable_samples_give_an_empty_vec_not_a_panic() {
        assert!(read_samples("不是 JSON").is_empty());
        assert!(read_samples("{}").is_empty());
        assert!(read_samples(r#"{"samples":"字符串"}"#).is_empty());
    }

    // ── 参数摘要 ─────────────────────────────────────────────────────

    #[test]
    fn arg_summary_is_bounded() {
        let big = json!({ "k": "x".repeat(500) });
        assert!(summarize_args(&big).chars().count() <= 121);
        assert_eq!(summarize_args(&json!({"a":1})), r#"{"a":1}"#);
    }

    // ── 与真子进程对接的那一条，默认不跑 ─────────────────────────────

    /// **镜像这条路通不通 —— 对着真的 `scope-mcp` 子进程验，但不碰 LLM。**
    ///
    /// 这条覆盖的是「AI 采了一窗，GUI 自己把它拉回来画出来」：
    /// 起子进程 → 连模拟器 → 采一窗 → [`Mirrored::poll`] 重建 `Capture`。
    ///
    /// 为什么值得单独有一条：真跑一次完整会话要花 LLM 的钱，而镜像**不经过
    /// LLM** —— 把它单独拎出来验，既便宜又能定位问题（会话没出图时，
    /// 一眼就能分清是镜像坏了还是模型根本没采）。
    ///
    /// ```text
    /// ./host/run.sh test -p scope-gui -- --ignored mirror_pulls_a_capture --nocapture
    /// ```
    #[test]
    #[ignore = "要起 scope-mcp 子进程；手动跑"]
    fn mirror_pulls_a_capture_from_a_real_subprocess() {
        let exe = find_scope_mcp().expect("找不到 scope-mcp —— 先 ./host/run.sh build");
        let mut mcp = McpProcess::spawn(&exe).expect("起不了子进程");

        mcp.call(
            "initialize",
            json!({"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}}),
        )
        .expect("握手失败");

        // ⚠ **必须检查 `isError`**，不能只看协议层有没有报错。
        //
        // 第一版这里只 `unwrap_or_else` 处理传输错误，于是「工具被拒」
        // （例如传了 schema 里没有的参数）与「调用成功」表现完全一样 ——
        // 我为此白跑了两轮才看出 store 是空的。
        let tool = |mcp: &mut McpProcess, name: &str, args: Value| {
            let r = mcp
                .call("tools/call", json!({"name": name, "arguments": args}))
                .unwrap_or_else(|e| panic!("{name} 传输失败：{}｜{}", e.message, e.hint));
            if r.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
                panic!("{name} 被拒：{}", tool_text(&r));
            }
            r
        };

        tool(
            &mut mcp,
            "scope_connect",
            json!({"transport": "sim", "sim_scenario": "i2c_100k"}),
        );
        // `scope_capture` 的参数只有 mode / timeout_ms / max_preview_points
        // —— 没有 count（那是 `scope_read_waveform` 的）
        tool(&mut mcp, "scope_capture", json!({}));

        // 先看一眼原始返回 —— poll 失败时这是唯一的线索
        let raw = tool(&mut mcp, "scope_list_captures", json!({}));
        let raw_text = tool_text(&raw);
        println!(
            "list_captures 原文（前 400 字）：\n{}",
            raw_text.chars().take(400).collect::<String>()
        );

        let mut mirrored = Mirrored::default();
        let caps = mirrored.poll(&mut mcp).expect("镜像失败");
        assert_eq!(caps.len(), 1, "应当拉回恰好一窗");

        let c = &caps[0];
        println!(
            "镜像回来：采集 #{} · {} 点 × {} 通道 · {} Hz · 触发点 {:?} · 溢出 {}",
            c.id,
            c.channels[0].len(),
            c.channels.len(),
            c.rate_hz,
            c.trigger_index,
            c.overrun
        );

        assert_eq!(c.channels.len(), 2, "i2c_100k 是双通道");
        assert_eq!(c.channels[0].len(), 4096, "应当拿到全窗（硬顶就是 4096）");
        assert!(c.rate_hz > 0, "采样率不该是 0");
        assert!(
            c.channels[0].iter().any(|&v| v > 2048) && c.channels[0].iter().any(|&v| v <= 2048),
            "样点应当跨越中点 —— 全 0 或全满说明拉空了"
        );
        assert!(
            c.expected_samples > 0,
            "expected_samples 没拿到 = report.rs 的缺口告警会永久静音"
        );

        // 再 poll 一次不该重复拉（同一个编号只镜像一次）
        let again = mirrored.poll(&mut mcp).expect("第二次镜像失败");
        assert!(again.is_empty(), "同一个采集不该被镜像两次");

        mcp.shutdown();
    }
}
