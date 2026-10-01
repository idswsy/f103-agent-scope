//! [`DevicePort`] —— 全局唯一的解耦点。
//!
//! 命令层以上（CLI / GUI / MCP / Agent）**只依赖这个 trait**，
//! 完全不关心背后是真硬件、模拟器还是 TCP。
//!
//! 这就是「模拟器一等公民」得以成立的原因：没有硬件时，
//! 把 `SimDevice` 塞进来即可，上层一行代码都不用改。

use crate::error::LinkError;
use scope_proto::{Frame, ParseStatus, Parser};
use std::time::{Duration, Instant};

/// 一台示波器的字节级通道。
///
/// 实现者只需关心「发字节 / 收字节」，帧的切分与校验由本 trait 的
/// [`DevicePort::recv_frame`] 默认实现负责。
pub trait DevicePort: Send {
    /// 发送原始字节。
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), LinkError>;

    /// 读取一批字节。返回 `Ok(0)` 表示当前没有数据（非阻塞语义）。
    fn read_some(&mut self) -> Result<Vec<u8>, LinkError>;

    /// 链路的人类可读描述，用于日志与错误信息。
    /// 例如 `"COM3 @ 921600"` 或 `"sim(sine_1k_3v3)"`。
    fn describe(&self) -> String;

    /// 是否为模拟器。MCP 据此决定是否注册 `scope_sim_set_scenario`。
    fn is_simulated(&self) -> bool {
        false
    }

    /// 该链路的等效字节率（字节/秒），用于动态计算读取超时。
    ///
    /// UART 921600 → 92160；USB CDC → 约 700_000；模拟器 → `usize::MAX`。
    fn byte_rate(&self) -> u32 {
        92_160
    }

    // ── 以下为默认实现，实现者通常不需要覆盖 ─────────────────────

    /// 根据 payload 大小估算本次读取该等多久。
    ///
    /// 公式来自协议规格：`50ms + payload_bytes × 8 / baud × 1.5`。
    fn recv_timeout_for(&self, payload_bytes: usize) -> Duration {
        let rate = self.byte_rate().max(1) as f64;
        let base_ms = 50.0;
        let transfer_ms = (payload_bytes as f64 * 8.0) / rate * 1000.0 * 1.5;
        Duration::from_millis((base_ms + transfer_ms).ceil() as u64)
    }
}

/// 带增量解析缓冲的接收器。
///
/// 把「字节流 → 帧」的脏活集中在这里，任何 [`DevicePort`] 实现都能直接复用。
pub struct FrameReader {
    parser: Parser,
    frame: Frame,
    scratch: Vec<u8>,
}

impl Default for FrameReader {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReader {
    /// 创建主机侧接收器（缓冲上限 = `MAX_FRAME_TX`）。
    pub fn new() -> Self {
        FrameReader {
            parser: Parser::new(),
            frame: Frame {
                header: scope_proto::Header {
                    ver: 0,
                    flags: 0,
                    seq: 0,
                    cmd_raw: 0,
                    len: 0,
                },
                payload: Vec::new(),
            },
            scratch: Vec::new(),
        }
    }

    /// 已丢弃的字节数（链路失步的指标）。
    pub fn dropped_count(&self) -> u32 {
        self.parser.dropped_count
    }

    /// 累计 CRC 错误数。
    pub fn crc_err_count(&self) -> u32 {
        self.parser.crc_err_count
    }

    /// 从设备读取下一个完整帧。
    ///
    /// 在 `timeout` 内反复轮询；超时返回 [`LinkError::Timeout`]。
    /// 链路层的 CRC 错与非法长度**不会**中断读取（解析器自行重同步）。
    pub fn next_frame(
        &mut self,
        port: &mut dyn DevicePort,
        timeout: Duration,
    ) -> Result<Frame, LinkError> {
        let deadline = Instant::now() + timeout;

        loop {
            // 先尝试排空已有缓冲
            loop {
                match self.parser.feed(&[], &mut self.frame) {
                    ParseStatus::Ok | ParseStatus::BadVer => {
                        return Ok(self.frame.clone());
                    }
                    ParseStatus::NeedMore => break,
                    ParseStatus::CrcErr | ParseStatus::BadLen => continue,
                }
            }

            if Instant::now() >= deadline {
                return Err(LinkError::Timeout(timeout));
            }

            let chunk = port.read_some()?;
            if chunk.is_empty() {
                std::thread::sleep(Duration::from_micros(500));
                continue;
            }
            self.scratch = chunk;

            // 喂入新字节；内部循环会直到 NeedMore 或出一帧
            match self.parser.feed(&self.scratch, &mut self.frame) {
                ParseStatus::Ok | ParseStatus::BadVer => return Ok(self.frame.clone()),
                _ => continue,
            }
        }
    }
}
