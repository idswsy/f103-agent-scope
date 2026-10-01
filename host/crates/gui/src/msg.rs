//! UI 线程 ↔ worker 线程的消息协议。
//!
//! 规矩只有一条：**每个 [`Request`] 恰好产生一个终态 [`Update`]**
//! （`Acquired` / `ConfigApplied` / `Connected` / `Disconnected` / `Failed`），
//! 中间过程用非终态消息（`Busy` / `StateChanged`）。
//!
//! 这条规矩让 UI 侧的 busy 态可以简单地用「发请求时置位、收到终态时复位」维护，
//! 不需要超时兜底，也不会出现永远转圈的按钮。

use scope_core::{Capture, DeviceConfig, DeviceInfo, State};
use scope_sim::Scenario;

/// 一次连接的传输选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportKind {
    /// 内置模拟器。
    Sim,
    /// 串口。
    Serial,
}

/// UI → worker。
#[derive(Debug, Clone)]
pub enum Request {
    /// 枚举串口。
    ListPorts,
    /// 连接。`transport` 决定走模拟器还是串口。
    Connect {
        /// 传输类型。
        transport: TransportKind,
        /// 串口名（`transport = Serial` 时有效）。
        port: String,
        /// 波特率（`transport = Serial` 时有效）。
        baud: u32,
        /// 模拟器场景（`transport = Sim` 时有效）。
        scenario: Scenario,
    },
    /// 断开并停流。
    Disconnect,
    /// 一次性下发配置。内部会先读回显、再更新缓存。
    ApplyConfig {
        /// 请求的采样率。设备会量化，实际值以回显为准。
        rate_hz: u32,
        /// 采集点数。
        samples: u16,
        /// 触发模式：`0=auto` / `1=normal` / `2=single`。
        trigger_mode: u8,
        /// 触发源通道。
        trigger_source: u8,
        /// 触发边沿：`0=上升` / `1=下降`。
        trigger_edge: u8,
        /// 触发电平（ADC LSB）。
        trigger_level_lsb: u16,
        /// 预触发点数。
        pre_samples: u16,
        /// 触发保持（µs）。
        holdoff_us: u32,
    },
    /// 采一次。走 [`scope_core::acquire`] 的完整编排。
    Acquire {
        /// 采样点数。
        samples: u16,
        /// 请求采样率。
        rate_hz: u32,
        /// 触发电平（ADC LSB）。
        trigger_level_lsb: u16,
        /// 等待触发的超时（ms）。
        timeout_ms: u64,
    },
    /// 切换模拟器场景（仅模拟器连接有效）。
    SetScenario(Scenario),
    /// `RESET` —— 从 `Fault` 态里出来的唯一办法。
    Reset,
    /// 让 worker 收尾退出。
    Shutdown,
}

/// worker → UI。
#[derive(Debug, Clone)]
pub enum Update {
    /// 串口枚举结果：`(端口名, 描述, 是否疑似目标)`。
    Ports(Vec<(String, String, bool)>),
    /// 连接成功。**`state` 是补发一次 `GET_STATUS` 拿到的真值** ——
    /// `connect()` 自己不发 `GET_STATUS`，缺了它 `guard_config_allowed`
    /// 就没有判据，配置按钮在"已武装"时也不会被拦住。
    Connected {
        /// 设备能力。
        info: DeviceInfo,
        /// 设备当前配置。
        config: Option<DeviceConfig>,
        /// 设备当前状态。
        state: State,
    },
    /// 已断开。
    Disconnected,
    /// 配置已生效（值来自设备回显）。
    ConfigApplied(Box<DeviceConfig>),
    /// 采集完成。
    Acquired(Box<Capture>),
    /// 设备状态发生变化。
    StateChanged(State),
    /// 某个长操作开始/结束。
    Busy {
        /// 操作名（显示在按钮上）。
        op: &'static str,
        /// 是否正在进行。
        active: bool,
    },
    /// 失败。**已在这一侧把 `ScopeError` 拆成可显示的两段** ——
    /// `ScopeError` 不是 `Clone`，且 UI 需要的是文案 + 自救提示。
    Failed {
        /// 一句话说明。
        text: String,
        /// 「怎么办」。来自 `DeviceError::hint()` 等，没有则为 `None`。
        hint: Option<String>,
    },
}

/// 把错误拆成 UI 能用的两段：说明 + 自救提示。
///
/// 项目纪律：错误要给"怎么办"，不能只给错误码
/// （见 `docs/03-protocol.md` 「UI/MCP 只用枚举不用裸码」）。
pub fn describe_error(e: &scope_core::ScopeError) -> (String, Option<String>) {
    use scope_core::ScopeError;
    match e {
        ScopeError::Device(d) => (d.to_string(), Some(d.hint().to_string())),
        ScopeError::Link(l) => {
            use scope_core::LinkError;
            let hint = match l {
                LinkError::Timeout(_) => "设备无响应：确认已上电、波特率正确、没被串口助手占用",
                LinkError::Disconnected => "链路已断开：点「连接」重建",
                LinkError::Desynchronized(_) => "连续 CRC 错，链路失步：重插一次 USB",
                LinkError::Open { .. } => "端口打不开：确认端口号，并关掉占用它的程序",
                _ => "检查接线与供电",
            };
            (l.to_string(), Some(hint.to_string()))
        }
        ScopeError::BadState { hint, .. } => (e.to_string(), Some(hint.clone())),
        ScopeError::InvalidParam { reason, .. } => (e.to_string(), Some(reason.clone())),
        ScopeError::NoTrigger(_, hint) => (e.to_string(), Some(hint.clone())),
        ScopeError::NoSuchCapture(id) => (
            e.to_string(),
            Some(format!(
                "采集 {id} 已被淘汰（历史只留最近 16 次），请重新采集"
            )),
        ),
        ScopeError::Unsupported(msg) => (e.to_string(), Some(msg.clone())),
        ScopeError::Cancelled(_) => (e.to_string(), None),
    }
}
