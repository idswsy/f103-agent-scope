//! AI 分析 —— 将证据包发送至语言模型，取回一段解释文本。
//!
//! # 职责范围：本模块仅处理网络，不涉及界面
//!
//! 状态存于 `app.rs`，渲染由 `panels.rs` 完成。本模块只承担三个步骤：
//! **构造请求体 → 发送 → 解析响应**，并将每类失败转换为
//! 「错误说明 + 处理措施」（项目约定：错误不得只给错误码）。
//!
//! # 使用独立线程的原因
//!
//! `worker.rs` 的线程**常驻并阻塞于设备 IO**（`Acquire` 最长等待 2 秒以上），
//! 而单次模型调用耗时 15–60 秒。若并入同一队列，分析期间全部设备操作将被阻塞。
//!
//! # 采用「每请求一线程」的原因
//!
//! 阻塞式 `ureq` **无法中断**，取消仅能表现为丢弃结果。若采用常驻线程，
//! 用户在取消后再次发起分析时，新请求须排队等待旧请求超时。
//! 每请求独立线程则无此问题：新请求立即开始，旧线程到期自行结束。
//!
//! # 请求关联（`req_id`）
//!
//! 用户可能在分析进行期间再次采集。因此每个请求携带自增编号；返回时若编号
//! 不匹配，**不予丢弃**（请求已计费），而是标注其对应的采集批次。
//! 处理细节见 `app.rs`。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::AiConfig;

/// 连接超时。握手阶段卡住通常意味着网络或 DNS 有问题，早点报出来。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 整体超时。**已包含连接耗时** —— 不是「连接 10 s + 读取 60 s」。
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(60);

/// 回复的长度上限。
///
/// 不设大值：这是一段解释，不是一篇报告。而且 `max_tokens` 直接影响
/// 响应时间和费用。
const MAX_TOKENS: u32 = 2048;

/// 一次分析的结果。
#[derive(Debug, Clone)]
pub struct AiAnswer {
    /// 模型给出的文字。
    pub text: String,
    /// 实际用的模型名（服务端回显的，不是我们请求的 —— 可能被路由）。
    pub model: String,
    /// 耗时。
    pub elapsed: Duration,
    /// 输入 token 数（服务端报的）。
    pub input_tokens: u64,
    /// 输出 token 数。
    pub output_tokens: u64,
}

/// 一次失败。**`hint` 是必填的** —— 界面上要显示「怎么办」。
#[derive(Debug, Clone)]
pub struct AiError {
    /// 出了什么事。
    pub message: String,
    /// 怎么办。
    pub hint: String,
}

impl AiError {
    fn new(message: impl Into<String>, hint: impl Into<String>) -> Self {
        AiError {
            message: message.into(),
            hint: hint.into(),
        }
    }
}

/// 交给 AI 线程的一次请求。
///
/// **不含 `req_id` 字段** —— 请求号由 [`AiWorker::start`] 分配，它是唯一
/// 掌握序列状态的一方。若由调用方传入再被覆盖，只会引入一个无法验证的
/// 「传入值与实际值是否一致」问题。
#[derive(Debug)]
pub struct AiJob {
    /// 这次分析针对哪个采集。
    pub capture_id: u16,
    /// 采集时那个采集的快照描述（界面上用来标注「基于哪一次」）。
    pub anchor: String,
    /// 已经渲染好的证据包。
    pub evidence: String,
}

/// AI 线程交回来的结果。
#[derive(Debug)]
pub struct AiUpdate {
    /// 对应哪个请求。
    pub req_id: u64,
    /// 针对哪个采集。
    pub capture_id: u16,
    /// 请求时的锚点描述。
    pub anchor: String,
    /// 成功或失败。
    pub outcome: Result<AiAnswer, AiError>,
}

/// 发送给语言模型的系统提示。
///
/// 其中每一条均对应一类已识别的失效模式：杜撰数值、将模拟器数据当作实测、
/// 对包络桶内细节作推断、信息不足时强行作答。
const SYSTEM_PROMPT: &str = "\
你是一台数字示波器 / I2C 总线分析仪的解读助手。用户刚完成一次采集，你将收到一份证据包。

须遵守以下规定：

1. **仅可引用证据包中出现的数值**。不得自行估算、推算或作近似表述；
   需要引用某个量时，直接采用其原始数值。
2. **须区分「实测」与「推断」**。凡由数值直接得出的结论与属于推测的内容，
   须以不同措辞分别表述，不得混同。
3. **波形包络为降采样结果**，每桶仅保留 min/max。**不得对桶内的毛刺、振铃、
   边沿形状作任何结论** —— 该信息在包络中不存在。
4. **须核对数据源标注**。若标注为模拟器，必须说明「本结论基于模拟器数据，
   不适用于真实硬件」。
5. **信息不足时须如实说明**，并指出所缺内容（例如需要更换场景重新采集、
   或需要真机数据）。不得为求答复完整而杜撰内容。

输出使用中文，分节书写：**结论** / **依据** / **建议下一步** / **不确定之处**。
「不确定之处」无内容时写「无」。全文控制在 400 字以内。";

/// 构造请求体（Anthropic Messages 格式）。
///
/// 与 `tools/agent_demo/agent.py` 采用同一格式，该格式已验证可用。
/// 唯一差异是本处使用 `system` 字段（该脚本把指令并入 user 消息）：
/// 本次调用为**单发**，无需维持多轮上下文。
pub fn build_body(cfg: &AiConfig, evidence: &str) -> Result<String, AiError> {
    let body = serde_json::json!({
        "model": cfg.model,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": evidence,
        }],
    });
    serde_json::to_string(&body)
        .map_err(|e| AiError::new(format!("构造请求体失败：{e}"), "属程序缺陷，请提交 issue"))
}

/// 解响应。
///
/// `http_status_as_error(false)` 之后，4xx/5xx 也会走到这里 ——
/// 这是有意的：API 的错误说明在响应体里，比光看状态码有用得多。
pub fn parse_response(
    status: u16,
    body: &str,
    fallback_model: &str,
    elapsed: Duration,
) -> Result<AiAnswer, AiError> {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            // 非 JSON 的响应体通常是网关/代理插进来的 HTML
            let head: String = body.chars().take(200).collect();
            return Err(match status {
                200..=299 => AiError::new(
                    format!("响应体无法解析（前 200 字符）：{head}"),
                    "端点可能配置有误。请确认 base_url 指向 messages 接口",
                ),
                _ => AiError::new(
                    format!("HTTP {status}，响应体无法解析：{head}"),
                    "确认 base_url 正确，并检查网络是否需要代理",
                ),
            });
        }
    };

    if !(200..=299).contains(&status) {
        return Err(http_error(status, &v));
    }

    // content 是一个块数组；只取 text 块。
    // （这个 API 还可能有 thinking 块 —— 那是模型的内部推理，不该显示给用户。）
    let text = v
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
        .unwrap_or_default();

    if text.trim().is_empty() {
        let stop = v
            .get("stop_reason")
            .and_then(|s| s.as_str())
            .unwrap_or("(未给出)");
        return Err(AiError::new(
            format!("模型未返回文本内容（stop_reason = {stop}）"),
            "若 stop_reason 为 max_tokens，说明响应被截断。请调整提问方式或缩短证据包",
        ));
    }

    let usage = v.get("usage");
    Ok(AiAnswer {
        text,
        model: v
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(fallback_model)
            .to_string(),
        elapsed,
        input_tokens: usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
        output_tokens: usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
    })
}

/// 把 HTTP 错误码翻译成「说明 + 怎么办」。
fn http_error(status: u16, v: &serde_json::Value) -> AiError {
    // 服务端自己的说明优先 —— 它比我们猜的准
    let detail = v
        .pointer("/error/message")
        .and_then(|m| m.as_str())
        .or_else(|| v.get("message").and_then(|m| m.as_str()))
        .unwrap_or("")
        .to_string();

    let (what, how) = match status {
        401 | 403 => ("API key 被拒绝", "确认密钥有效、未过期、账户配额充足"),
        404 => (
            "端点不存在（404）",
            "base_url 配置有误，应指向 messages 接口，例如 \
             https://api.deepseek.com/anthropic/v1/messages",
        ),
        429 => (
            "请求频率过高或配额耗尽（429）",
            "稍后重试；若持续出现，请在服务商控制台核对配额",
        ),
        500..=599 => ("服务端错误", "非本端配置问题，稍后重试"),
        _ => ("请求被拒绝", "确认 base_url 与密钥属于同一服务商"),
    };

    let message = if detail.is_empty() {
        format!("{what}（HTTP {status}）")
    } else {
        format!("{what}（HTTP {status}）：{detail}")
    };
    AiError::new(message, how)
}

/// 真正发一次请求。**阻塞**，所以只能在 AI 线程里调。
fn ask(cfg: &AiConfig, evidence: &str, cancel: &AtomicBool) -> Result<AiAnswer, AiError> {
    if !cfg.has_key() {
        return Err(AiError::new(
            "未配置 API 密钥",
            "请在面板的「设置」中填写密钥",
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::new("请求已取消", "可重新发起分析"));
    }

    let body = build_body(cfg, evidence)?;
    let t0 = Instant::now();

    let result = ureq::post(cfg.base_url.trim())
        .header("content-type", "application/json")
        .header("x-api-key", cfg.api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .config()
        // 4xx/5xx 也走 Ok 分支 —— 我们要读响应体里的错误说明
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(GLOBAL_TIMEOUT))
        .build()
        .send(body.as_str());

    let elapsed = t0.elapsed();

    match result {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            parse_response(status, &text, &cfg.model, elapsed)
        }
        Err(e) => Err(transport_error(&e)),
    }
}

/// 传输层失败（连不上、超时、DNS 挂了）。
fn transport_error(e: &ureq::Error) -> AiError {
    use ureq::Error as E;
    match e {
        E::Timeout(_) => AiError::new(
            format!("请求超时（{} 秒）", GLOBAL_TIMEOUT.as_secs()),
            "网络延迟或服务端无响应。可重试；若持续超时，请检查网络与代理设置",
        ),
        E::HostNotFound => AiError::new(
            "域名解析失败",
            "确认 base_url 中的域名拼写正确，且本机可访问外网",
        ),
        E::ConnectionFailed => {
            AiError::new("无法连接服务器", "确认网络连通性、防火墙策略与代理设置")
        }
        E::BadUri(u) => AiError::new(
            format!("地址格式错误：{u}"),
            "base_url 须为完整 URL，含 http(s):// 前缀",
        ),
        // TLS 握手失败会落到这里 —— 在 Windows + GNU 工具链下这是真会发生的
        E::Io(io) => AiError::new(
            format!("网络 IO 错误：{io}"),
            "若为证书或 TLS 相关错误，可能被中间设备拦截。请更换网络或配置代理",
        ),
        other => AiError::new(
            format!("请求失败：{other}"),
            "检查网络与 base_url；若持续失败请提交 issue",
        ),
    }
}

/// AI 线程的句柄。**每请求一条线程**，理由见模块头。
pub struct AiWorker {
    tx: Sender<AiUpdate>,
    rx: Receiver<AiUpdate>,
    cancel: Arc<AtomicBool>,
    /// 当前在跑的那条线程。`None` = 还没跑过。
    in_flight: Option<std::thread::JoinHandle<()>>,
    next_id: u64,
    ctx: egui::Context,
}

impl AiWorker {
    /// 建句柄。**不起常驻线程** —— 线程是按请求起的。
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        AiWorker {
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            in_flight: None,
            next_id: 1,
            ctx,
        }
    }

    /// 发起一次分析，返回请求号。
    pub fn start(&mut self, cfg: AiConfig, job: AiJob) -> u64 {
        // 新一轮，把上一次的取消标志清掉
        self.cancel.store(false, Ordering::Relaxed);

        let req_id = self.next_id;
        self.next_id += 1;
        let capture_id = job.capture_id;
        let anchor = job.anchor.clone();
        let evidence = job.evidence;

        let tx = self.tx.clone();
        let cancel = self.cancel.clone();
        let ctx = self.ctx.clone();
        // 闭包里要用一份，起线程失败那条兜底路径也要用一份
        let anchor_for_thread = anchor.clone();

        // 上一条线程可能还在跑（阻塞在 ureq 里，打断不了）。
        // **不 join 它** —— 挡住界面没有意义，让它在超时后自己结束，
        // 它的结果会因为 req_id 对不上而被标注为「过期」。
        let spawned = std::thread::Builder::new()
            .name("scope-ai".into())
            .spawn(move || {
                let outcome = ask(&cfg, &evidence, &cancel);
                // 发送失败只说明界面已经关了 —— 不是错误
                if tx
                    .send(AiUpdate {
                        req_id,
                        capture_id,
                        anchor: anchor_for_thread,
                        outcome,
                    })
                    .is_ok()
                {
                    ctx.request_repaint();
                }
            });

        match spawned {
            Ok(h) => self.in_flight = Some(h),
            Err(e) => {
                // 起不了线程是很罕见的情况，但**不能静默** ——
                // 界面会永远停在「分析中」
                let _ = self.tx.send(AiUpdate {
                    req_id,
                    capture_id,
                    anchor,
                    outcome: Err(AiError::new(
                        format!("无法创建后台线程：{e}"),
                        "系统资源不足。请关闭部分程序后重试",
                    )),
                });
                self.ctx.request_repaint();
            }
        }
        req_id
    }

    /// 排空回执。**收进 Vec 再处理** —— 和 `worker.rs` 一个路子，
    /// 避免在闭包里可变借走 App。
    pub fn drain(&self, mut f: impl FnMut(AiUpdate)) {
        while let Ok(u) = self.rx.try_recv() {
            f(u);
        }
    }

    /// 请求取消。**在途的 HTTP 打断不了**，这只是让结果被标成过期。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// AI 面板的界面状态。
///
/// 「是否正在分析」以 [`AiState::is_running`]（即 `awaiting`）为唯一判据。
/// **不要**改用线程存活状态：请求被取消后，在途 HTTP 仍会跑到超时，
/// 而那时界面应当已经回到就绪态。
pub struct AiState {
    /// 网络线程句柄。
    pub worker: AiWorker,
    /// 当前配置（含密钥）。
    pub cfg: crate::config::AiConfig,
    /// 配置文件路径。`None` = 无可用写入位置，配置仅保存在内存中。
    pub config_path: Option<std::path::PathBuf>,
    /// 面板是否展开。
    pub panel_open: bool,
    /// 正在等待结果的请求号。
    awaiting: Option<u64>,
    /// 最近一次结果。**迟到结果亦予保留** —— 请求已计费，丢弃不若标注。
    pub answer: Option<AiAnswer>,
    /// 结果对应的请求号。
    answer_req: u64,
    /// 结果对应的采集编号。
    pub answer_capture_id: u16,
    /// 生成结果时该采集的锚点描述。
    pub answer_anchor: String,
    /// 结果迟于其请求返回（此前已取消，或已发起新的分析）。
    pub answer_late: bool,
    /// 最近一次失败。
    pub error: Option<AiError>,
    /// 用户输入的分析要求。
    pub question: String,
    /// 密钥是否明文显示。
    pub show_key: bool,
}

impl AiState {
    /// 读取配置并建立状态。
    ///
    /// 返回 `(状态, 配置来源说明)`。说明描述配置文件的位置与读取结果
    /// （不存在 / 解析失败 / 无可用目录），**由调用方记入日志**，
    /// 不在操作界面上常驻显示。
    pub fn load(ctx: egui::Context) -> (AiState, Option<String>) {
        let path = crate::config::default_config_path();
        let (cfg, note) = crate::config::load(path.as_deref());
        let state = AiState {
            worker: AiWorker::new(ctx),
            cfg,
            config_path: path,
            panel_open: false,
            awaiting: None,
            answer: None,
            answer_req: 0,
            answer_capture_id: 0,
            answer_anchor: String::new(),
            answer_late: false,
            error: None,
            question: String::new(),
            show_key: false,
        };
        (state, note)
    }

    /// 当前是否允许发起分析。
    pub fn can_start(&self) -> bool {
        self.cfg.has_key() && self.awaiting.is_none()
    }

    /// 是否正在等待结果。界面上的「分析中」状态以此为准。
    pub fn is_running(&self) -> bool {
        self.awaiting.is_some()
    }

    /// 发起一次分析。返回是否成功发出。
    pub fn start(&mut self, evidence: String, capture_id: u16, anchor: String) -> bool {
        if !self.can_start() {
            return false;
        }
        // 清空上一次结果，避免新旧结论并列显示
        self.error = None;
        self.answer_late = false;
        let req_id = self.worker.start(
            self.cfg.clone(),
            AiJob {
                capture_id,
                anchor,
                evidence,
            },
        );
        self.awaiting = Some(req_id);
        true
    }

    /// 取消。界面立刻回到就绪态；在途的请求会在超时后返回，届时被标成「迟到」。
    pub fn cancel(&mut self) {
        self.worker.cancel();
        self.awaiting = None;
    }

    /// 收一条回执。
    pub fn apply(&mut self, u: AiUpdate) {
        // 不是正在等的那个 —— 说明用户取消过，或者又发起了一次。
        // **不丢弃**：钱已经花了，标一下比扔掉有用。
        self.answer_late = self.awaiting != Some(u.req_id);
        self.awaiting = None;

        match u.outcome {
            Ok(a) => {
                self.answer = Some(a);
                self.answer_req = u.req_id;
                self.answer_capture_id = u.capture_id;
                self.answer_anchor = u.anchor;
            }
            Err(e) => {
                self.error = Some(e);
            }
        }
    }

    /// 结果对应的采集和当前显示的不是同一次。
    pub fn answer_is_about_another_capture(&self, current: Option<u16>) -> bool {
        self.answer.is_some() && current != Some(self.answer_capture_id)
    }

    /// 保存配置到磁盘。返回一句可直接写入日志的结果说明。
    ///
    /// **不再往状态里存一份** —— 曾经有个 `config_note` 字段同时被面板和返回值读，
    /// 面板改成不显示之后它就变成了「写进去、没人读」。部署细节归日志，
    /// 调用方拿到这个字符串记一行即可。
    pub fn save_config(&self) -> String {
        let Some(p) = self.config_path.clone() else {
            return "未能保存：无可用配置目录（APPDATA / XDG_CONFIG_HOME / HOME 均未设置）。\
                    设置仅保存在内存中。"
                .into();
        };
        match crate::config::save(&p, &self.cfg) {
            Ok(()) => format!("AI 配置已保存至 {}", p.display()),
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiConfig;

    fn cfg() -> AiConfig {
        AiConfig {
            api_key: "sk-test".into(),
            base_url: "https://example.test/v1/messages".into(),
            model: "test-model".into(),
        }
    }

    // ── 请求体 ───────────────────────────────────────────────────────

    #[test]
    fn body_has_the_shape_the_api_expects() {
        let b = build_body(&cfg(), "证据在这里").unwrap();
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();

        assert_eq!(v["model"], "test-model");
        assert_eq!(v["max_tokens"], MAX_TOKENS);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "证据在这里");
        // system 中必须包含「禁止杜撰数值」一条 —— 缺失时模型将自行发挥
        assert!(
            v["system"]
                .as_str()
                .unwrap()
                .contains("仅可引用证据包中出现的数值"),
            "system prompt 中的反杜撰规定缺失"
        );
    }

    #[test]
    fn system_prompt_forbids_guessing_bucket_interiors() {
        let b = build_body(&cfg(), "x").unwrap();
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        let sys = v["system"].as_str().unwrap();
        assert!(sys.contains("桶内"), "须禁止对包络桶内细节作结论");
        assert!(sys.contains("模拟器"), "须要求声明模拟器数据源");
        assert!(
            sys.contains("信息不足时须如实说明"),
            "须允许并明确要求「信息不足」这一结论"
        );
    }

    // ── 响应解析：正常路径 ───────────────────────────────────────────

    #[test]
    fn parses_a_normal_response() {
        let body = r#"{
            "model": "deepseek-flash",
            "content": [{"type": "text", "text": "结论：一切正常"}],
            "usage": {"input_tokens": 1234, "output_tokens": 56}
        }"#;
        let a = parse_response(200, body, "fallback", Duration::from_millis(1500)).unwrap();
        assert_eq!(a.text, "结论：一切正常");
        assert_eq!(a.model, "deepseek-flash");
        assert_eq!(a.input_tokens, 1234);
        assert_eq!(a.output_tokens, 56);
    }

    /// thinking 块是模型的内部推理，**不该显示给用户**。
    #[test]
    fn thinking_blocks_are_not_shown() {
        let body = r#"{"model":"m","content":[
            {"type":"thinking","thinking":"内部推理，用户不该看到"},
            {"type":"text","text":"这是给用户看的"}
        ]}"#;
        let a = parse_response(200, body, "m", Duration::from_millis(1)).unwrap();
        assert_eq!(a.text, "这是给用户看的");
        assert!(!a.text.contains("内部推理"));
    }

    #[test]
    fn multiple_text_blocks_are_joined() {
        let body = r#"{"model":"m","content":[
            {"type":"text","text":"第一段"},
            {"type":"text","text":"第二段"}
        ]}"#;
        let a = parse_response(200, body, "m", Duration::from_millis(1)).unwrap();
        assert!(a.text.contains("第一段") && a.text.contains("第二段"));
    }

    #[test]
    fn model_falls_back_when_server_omits_it() {
        let body = r#"{"content":[{"type":"text","text":"ok"}]}"#;
        let a = parse_response(200, body, "my-requested-model", Duration::from_millis(1)).unwrap();
        assert_eq!(a.model, "my-requested-model");
    }

    // ── 响应解析：错误路径，每一条都要有「怎么办」 ───────────────────

    #[test]
    fn empty_content_is_an_error_not_an_empty_answer() {
        let body = r#"{"model":"m","content":[],"stop_reason":"max_tokens"}"#;
        let e = parse_response(200, body, "m", Duration::from_millis(1)).expect_err("空回复要报错");
        assert!(e.message.contains("未返回文本内容"));
        assert!(
            !e.hint.is_empty() && e.hint.contains("max_tokens"),
            "要指出可能是被截断了：{}",
            e.hint
        );
    }

    #[test]
    fn unauthorized_points_at_the_key() {
        let body = r#"{"error":{"message":"Authentication Fails"}}"#;
        let e = parse_response(401, body, "m", Duration::from_millis(1)).unwrap_err();
        assert!(e.message.contains("401"));
        assert!(
            e.message.contains("Authentication Fails"),
            "要带上服务端的原话"
        );
        assert!(e.hint.contains("密钥"), "处理措施里要提到密钥：{}", e.hint);
    }

    #[test]
    fn not_found_points_at_the_url() {
        let body = r#"{"error":{"message":"Not Found"}}"#;
        let e = parse_response(404, body, "m", Duration::from_millis(1)).unwrap_err();
        assert!(e.hint.contains("base_url"), "404 应当指向 base_url");
    }

    #[test]
    fn server_error_says_it_is_not_your_fault() {
        let body = r#"{"error":{"message":"internal"}}"#;
        let e = parse_response(503, body, "m", Duration::from_millis(1)).unwrap_err();
        assert!(
            e.hint.contains("非本端配置问题"),
            "5xx 须说明这不是本地配置错误：{}",
            e.hint
        );
    }

    /// 网关返回 HTML 是常见的 —— 不能崩，而且要给可操作的提示。
    #[test]
    fn html_instead_of_json_is_handled() {
        let html = "<html><body>502 Bad Gateway</body></html>";
        let e = parse_response(502, html, "m", Duration::from_millis(1)).unwrap_err();
        assert!(e.message.contains("无法解析"));
        assert!(!e.hint.is_empty(), "即使这样也要给怎么办");
    }

    /// 边界：200 但返回 HTML（端点写成了一个网页）。
    #[test]
    fn html_with_200_status_is_also_handled() {
        let html = "<!DOCTYPE html><html>hello</html>";
        let e = parse_response(200, html, "m", Duration::from_millis(1)).unwrap_err();
        assert!(
            e.hint.contains("base_url"),
            "200 + 非 JSON 说明端点指错了地方：{}",
            e.hint
        );
    }

    // ── 每一条错误都必须带「怎么办」 ─────────────────────────────────

    #[test]
    fn every_error_carries_a_hint() {
        let cases: [(u16, &str); 6] = [
            (400, r#"{"error":{"message":"bad"}}"#),
            (401, "{}"),
            (404, "{}"),
            (429, "{}"),
            (500, "{}"),
            (418, "not json"),
        ];
        for (status, body) in cases {
            let e = parse_response(status, body, "m", Duration::from_millis(1)).unwrap_err();
            assert!(
                !e.hint.trim().is_empty(),
                "HTTP {status} 的错误没有「怎么办」"
            );
            assert!(!e.message.trim().is_empty(), "HTTP {status} 的错误没有说明");
        }
    }

    // ── 没填 key 时不发请求 ──────────────────────────────────────────

    #[test]
    fn missing_key_fails_before_any_network_call() {
        let empty = AiConfig {
            api_key: String::new(),
            ..cfg()
        };
        let cancel = AtomicBool::new(false);
        // 端点是个不存在的域名 —— 如果它真去发了，这条会以网络错误的形式失败，
        // 而不是以「未配置密钥」的形式
        let e = ask(&empty, "证据", &cancel).unwrap_err();
        assert!(e.message.contains("未配置"), "实测：{}", e.message);
    }

    #[test]
    fn already_cancelled_does_not_send() {
        let cancel = AtomicBool::new(true);
        let e = ask(&cfg(), "证据", &cancel).unwrap_err();
        assert!(e.message.contains("已取消"), "实测：{}", e.message);
    }

    // ── 需要外网的那一条，默认不跑 ───────────────────────────────────

    /// **真的发一次 HTTPS 请求**，用一个假 key 打真实端点。
    ///
    /// 目的不是拿到答案，是验证 **DNS + TCP + TLS + HTTP + 错误映射**
    /// 这整条链在这台机器上真的通。**期望结果就是 401**（假 key）——
    /// 真返回成功反而说明哪里不对。
    ///
    /// 这条是**唯一**能覆盖 TLS/ring 在 MinGW 下链接结果的自动化手段，
    /// 所以放成 `#[ignore]` 而不是删掉。手动跑：
    ///
    /// ```text
    /// ./host/run.sh test -p scope-gui -- --ignored live_endpoint
    /// ```
    #[test]
    #[ignore = "需要外网；手动跑：./host/run.sh test -p scope-gui -- --ignored live_endpoint"]
    fn live_endpoint_round_trip_maps_the_auth_error() {
        let live = AiConfig {
            api_key: "sk-definitely-not-a-real-key".into(),
            ..AiConfig::default()
        };
        let cancel = AtomicBool::new(false);
        let e = ask(&live, "这是一个测试用的证据包。", &cancel)
            .expect_err("假 key 不该成功 —— 成功了说明端点上有什么东西不对");

        assert!(
            e.message.contains("401") || e.message.contains("403"),
            "期望看到鉴权失败，实测：{} / 怎么办：{}",
            e.message,
            e.hint
        );
        assert!(!e.hint.trim().is_empty(), "错误必须带「怎么办」");
        // 这条同时证明传输层没挂：能走到 401 说明握手成功了
        assert!(
            !e.message.contains("连不上") && !e.message.contains("超时"),
            "传输层就失败了，问题在 TLS/网络而不在 key：{}",
            e.message
        );
    }
}
