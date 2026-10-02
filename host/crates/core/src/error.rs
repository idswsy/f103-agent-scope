//! 统一错误模型。
//!
//! 三层错误泾渭分明，**绝不混用**：
//!
//! | 层 | 类型 | 含义 |
//! |---|---|---|
//! | 链路 | [`LinkError`] | 串口打不开、超时、断连 |
//! | 协议 | [`DeviceError`] | 设备回了错误帧（有错误码） |
//! | 逻辑 | [`ScopeError`] | 参数非法、状态不允许、无数据 |
//!
//! 上层（CLI / MCP）**只用 [`ScopeError`]**，永远拿不到裸错误码 ——
//! 这是「UI/MCP 只用枚举不用裸码」纪律的落地点。

use scope_proto::{ErrorCode, Severity};
use thiserror::Error;

/// 链路层错误：通信本身出了问题。
#[derive(Debug, Error)]
pub enum LinkError {
    /// 打开链路失败。
    #[error("打开 {target} 失败: {source}")]
    Open {
        /// 目标描述（如 `COM3 @ 921600`）。
        target: String,
        /// 底层错误。
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// 写入失败。
    #[error("写入失败: {0}")]
    Write(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// 读取失败。
    #[error("读取失败: {0}")]
    Read(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// 该超时内没有收到期望的响应。
    #[error("等待响应超时 ({0:?})")]
    Timeout(std::time::Duration),

    /// 链路已断开。
    #[error("链路已断开")]
    Disconnected,

    /// 链路层已失去同步（连续 CRC 错）。
    #[error("链路失去同步：连续 {0} 个 CRC 错误")]
    Desynchronized(u32),
}

/// 协议层错误：设备回了带 ERROR 标志的帧。
#[derive(Debug, Error)]
pub struct DeviceError {
    /// 设备返回的错误码。
    pub code: ErrorCode,
    /// 严重级别。
    pub severity: Severity,
    /// 设备附带的文字说明（≤32 字节 ASCII）。
    pub message: String,
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "设备错误 {:?} ({:?}): {}",
            self.code, self.severity, self.message
        )
    }
}

/// 逻辑层错误：本地校验失败或设备状态不允许。
#[derive(Debug, Error)]
pub enum ScopeError {
    /// 链路问题。
    #[error(transparent)]
    Link(#[from] LinkError),

    /// 设备回了一个错误帧。
    #[error(transparent)]
    Device(#[from] DeviceError),

    /// 参数越界或非法。**在发到线上之前就被本地拦下**。
    #[error("参数非法: {field} = {value}，{reason}")]
    InvalidParam {
        /// 字段名。
        field: &'static str,
        /// 实际值。
        value: String,
        /// 为什么非法。
        reason: String,
    },

    /// 当前状态不允许该操作。
    #[error("当前状态 {current:?} 不允许执行 {action}；{hint}")]
    BadState {
        /// 设备当前状态。
        current: scope_proto::State,
        /// 想做的事。
        action: &'static str,
        /// 怎么办。
        hint: String,
    },

    /// 找不到指定采集。
    #[error("找不到采集 {0}（可能已被淘汰；用 list_captures 查看现存采集）")]
    NoSuchCapture(u16),

    /// 设备不支持。
    #[error("硬件不支持: {0}")]
    Unsupported(String),

    /// 未在预期时间内触发。
    #[error("未在 {0} ms 内触发；{1}")]
    NoTrigger(u64, String),

    /// 操作被本地取消（用户在等待期间按了停止）。
    ///
    /// 与 [`NoTrigger`](Self::NoTrigger) 分开：那是「设备没触发」，
    /// 这是「人不想等了」—— 两种情况的 UI 提示完全不同。
    #[error("已取消：{0}")]
    Cancelled(&'static str),
}

/// 结果别名。
pub type Result<T> = std::result::Result<T, ScopeError>;

impl ScopeError {
    /// 一句话说明 —— **不含**「怎么办」。
    ///
    /// 有四个变体的 `Display` 已经把 hint/reason 拼进正文了
    /// （见各变体的 `#[error(...)]`）。显示层如果直接 `to_string()` 再把
    /// [`hint`](Self::hint) 单独显示一次，同一句话会出现两遍。
    /// 所以显示层应该用 `summary()` + `hint()` 这一对，而不是 `Display`。
    pub fn summary(&self) -> String {
        match self {
            ScopeError::InvalidParam { field, value, .. } => {
                format!("参数非法: {field} = {value}")
            }
            ScopeError::BadState {
                current, action, ..
            } => format!("当前状态 {current:?} 不允许执行 {action}"),
            ScopeError::NoTrigger(ms, _) => format!("未在 {ms} ms 内触发"),
            ScopeError::Unsupported(_) => "该功能在此硬件上不可用".to_string(),
            other => other.to_string(),
        }
    }

    /// 「怎么办」。没有建议可给的变体返回 `None`。
    ///
    /// 项目纪律：错误要给「怎么办」，不能只给错误码
    /// （见 `docs/03-protocol.md` 的「UI/MCP 只用枚举不用裸码」）。
    pub fn hint(&self) -> Option<String> {
        match self {
            ScopeError::InvalidParam { reason, .. } => Some(reason.clone()),
            ScopeError::BadState { hint, .. } => Some(hint.clone()),
            ScopeError::NoTrigger(_, hint) => Some(hint.clone()),
            ScopeError::Unsupported(msg) => Some(msg.clone()),
            ScopeError::Device(d) => Some(d.hint().to_string()),
            ScopeError::Link(l) => Some(link_hint(l).to_string()),
            ScopeError::NoSuchCapture(id) => Some(format!(
                "采集 {id} 已被淘汰（历史只留最近 16 次），请重新采集"
            )),
            ScopeError::Cancelled(_) => None,
        }
    }
}

/// 链路错误的「怎么办」。
fn link_hint(l: &LinkError) -> &'static str {
    match l {
        LinkError::Timeout(_) => "设备无响应：确认已上电、波特率正确、没被串口助手占用",
        LinkError::Disconnected => "链路已断开：重新连接",
        LinkError::Desynchronized(_) => "连续 CRC 错，链路失步：重插一次 USB",
        LinkError::Open { .. } => "端口打不开：确认端口号，并关掉占用它的程序",
        _ => "检查接线与供电",
    }
}

impl DeviceError {
    /// 附带一条「怎么办」的提示，供 Agent 自我修复。
    pub fn hint(&self) -> &'static str {
        match self.code {
            ErrorCode::UnknownCmd => "固件版本可能过旧；先跑 GET_INFO 看 fw_ver",
            ErrorCode::BadLen => "payload 长度不合法；这通常是上位机 bug，请报 issue",
            ErrorCode::BadParam => "检查参数范围（采样率/样点数/电平）",
            ErrorCode::Busy => "设备正在采集；先发 STOP 再改配置",
            ErrorCode::BadState => "命令与当前状态不匹配；先 GET_STATUS",
            ErrorCode::NoData => "指定的 capture 不存在或已失效；重新 capture",
            ErrorCode::Overrun => "采样溢出：降低采样率或抽点倍数，或改用单次采集",
            ErrorCode::Timeout => "没等到触发；检查触发源与电平，或改 auto 模式",
            ErrorCode::VersionMismatch => "固件与上位机协议主版本不符；更新固件或上位机",
            ErrorCode::Unsupported => "该功能在此硬件上不可用（见 docs/04-performance.md）",
            ErrorCode::FlashErr => "Flash 操作失败；检查是否写保护",
            ErrorCode::Internal => "设备内部错误；读取 GET_LAST_ERROR 获取详情",
        }
    }
}
