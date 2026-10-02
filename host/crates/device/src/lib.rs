//! # scope-device —— 运行时可在「模拟器」与「串口真机」之间切换的设备包装。
//!
//! GUI 与 MCP 都需要同一件事：**连接方式在运行时才确定**（用户点选 / 参数指定），
//! 而 [`scope_core::CommandBus`] 对 [`scope_core::DevicePort`] 是泛型的。
//!
//! # 为什么不能用 `Box<dyn DevicePort>`
//!
//! [`scope_core::DevicePort`] **没有 downcast**（没有 `as_any`）。一旦擦成
//! `Box<dyn DevicePort>`，就再也拿不回 `SimDevice` —— `set_scenario`、`faults`
//! 全都够不着了，而这两样是没有硬件时唯一能演示的东西。
//!
//! 所以用显式 enum 保留具体类型。
//!
//! # 六个方法必须**全部**转发
//!
//! 尤其 [`DevicePort::is_simulated`] 与 [`DevicePort::byte_rate`] —— 它们有默认
//! 实现，漏转发不会编译报错，但后果很实在：`is_simulated()` 会谎报 false
//! （MCP 会因此不注册模拟器专属工具），模拟器会被按 92160 B/s 估超时
//! （每个分片白等约 270 ms）。下面的单测把这两条锁住了。

#![deny(clippy::all)]
#![warn(missing_docs)]

use scope_core::{DevicePort, LinkError};
use scope_sim::{Scenario, SimDevice};
use scope_transport_serial::SerialDevice;

/// 连接方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// 内置模拟器 —— 不需要硬件。
    Sim,
    /// 串口真机。
    Serial,
}

impl TransportKind {
    /// 从字符串解析（`"sim"` / `"serial"`），大小写不敏感。
    pub fn parse(s: &str) -> Option<TransportKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sim" | "simulator" => Some(TransportKind::Sim),
            "serial" | "uart" | "port" => Some(TransportKind::Serial),
            _ => None,
        }
    }
}

/// 一台已经打开的示波器。
pub enum Transport {
    /// 内置模拟器。
    Sim(Box<SimDevice>),
    /// 串口真机。
    Serial(Box<SerialDevice>),
}

impl Transport {
    /// 按连接方式打开。`Sim` 时只用 `scenario`，`Serial` 时只用 `port` / `baud`。
    pub fn open(
        kind: TransportKind,
        port: &str,
        baud: u32,
        scenario: Scenario,
    ) -> Result<Transport, LinkError> {
        match kind {
            TransportKind::Sim => Ok(Transport::sim(scenario)),
            TransportKind::Serial => Transport::serial(port, baud),
        }
    }

    /// 建一个模拟器传输。
    pub fn sim(scenario: Scenario) -> Transport {
        Transport::Sim(Box::new(SimDevice::new(scenario)))
    }

    /// 建一个串口传输。失败时返回链路错误（端口打不开 / 被占用）。
    pub fn serial(port: &str, baud: u32) -> Result<Transport, LinkError> {
        // timeout 5 ms：CH340 驱动缓冲较深，超时取大了会让轮询变慢
        SerialDevice::open(port, baud, 5).map(|d| Transport::Serial(Box::new(d)))
    }

    /// 拿到模拟器分支的可变引用；串口分支返回 `None`。
    ///
    /// 切换场景、注入故障都从这里走。
    pub fn sim_mut(&mut self) -> Option<&mut SimDevice> {
        match self {
            Transport::Sim(d) => Some(d),
            Transport::Serial(_) => None,
        }
    }

    /// 是不是模拟器（与 [`DevicePort::is_simulated`] 同义，方便直接问具体类型）。
    pub fn is_sim(&self) -> bool {
        matches!(self, Transport::Sim(_))
    }
}

impl DevicePort for Transport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        match self {
            Transport::Sim(d) => d.write_all(bytes),
            Transport::Serial(d) => d.write_all(bytes),
        }
    }

    fn read_some(&mut self) -> Result<Vec<u8>, LinkError> {
        match self {
            Transport::Sim(d) => d.read_some(),
            Transport::Serial(d) => d.read_some(),
        }
    }

    fn describe(&self) -> String {
        match self {
            Transport::Sim(d) => d.describe(),
            Transport::Serial(d) => d.describe(),
        }
    }

    /// **别删。** 漏了它，`is_simulated()` 会对模拟器谎报 false。
    fn is_simulated(&self) -> bool {
        match self {
            Transport::Sim(d) => d.is_simulated(),
            Transport::Serial(d) => d.is_simulated(),
        }
    }

    /// **别删。** 漏了它，模拟器会被按 92160 B/s（默认实现的值）估超时，
    /// 每个分片白等约 270 ms。模拟器实际是 `u32::MAX`。
    fn byte_rate(&self) -> u32 {
        match self {
            Transport::Sim(d) => d.byte_rate(),
            Transport::Serial(d) => d.byte_rate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarding_does_not_fall_back_to_defaults() {
        // 这两条锁住「六个方法必须全部转发」那个坑：
        // `is_simulated` 与 `byte_rate` 都有默认实现，漏转发不会编译报错，
        // 但 is_simulated 会谎报 false（MCP 就不再注册模拟器专属工具），
        // byte_rate 会退回 92160（模拟器每个分片白等 270 ms）。
        let t = Transport::sim(Scenario::I2c100k);
        assert!(t.is_simulated(), "Transport::Sim 必须转发 is_simulated");
        assert!(t.is_sim(), "具体类型上的判断也应一致");
        assert_eq!(
            t.byte_rate(),
            u32::MAX,
            "模拟器不受链路限制，byte_rate 应转发而不是用默认的 92160"
        );
    }

    #[test]
    fn describe_comes_from_the_inner_device() {
        let t = Transport::sim(Scenario::Dc);
        assert!(
            t.describe().starts_with("sim("),
            "describe 应转发给 SimDevice，实测 {}",
            t.describe()
        );
    }

    #[test]
    fn kind_parses_the_names_mcp_clients_use() {
        assert_eq!(TransportKind::parse("sim"), Some(TransportKind::Sim));
        assert_eq!(TransportKind::parse("SIM"), Some(TransportKind::Sim));
        assert_eq!(
            TransportKind::parse(" serial "),
            Some(TransportKind::Serial)
        );
        assert_eq!(TransportKind::parse("nope"), None);
    }

    #[test]
    fn open_dispatches_by_kind() {
        let t = Transport::open(TransportKind::Sim, "", 0, Scenario::Sine1k3v3).unwrap();
        assert!(t.is_sim());
        // 串口分支不在这里测 —— 它会去真的打开 COM 口
    }
}
