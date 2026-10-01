//! 运行时可在「模拟器」与「串口真机」之间切换的设备包装。
//!
//! # 为什么不能用 `Box<dyn DevicePort>`
//!
//! [`scope_core::DevicePort`] **没有 downcast**（没有 `as_any`）。一旦把它擦成
//! `Box<dyn DevicePort>`，就再也拿不回 `SimDevice` —— `set_scenario`、`faults`
//! 全都够不着了，而"切换场景"是 GUI 在没有硬件时唯一能演示的东西。
//!
//! 所以这里用一个显式的 enum，保留具体类型。
//!
//! # 六个方法必须**全部**转发
//!
//! 尤其 [`DevicePort::is_simulated`] 与 [`DevicePort::byte_rate`] —— 它们有默认实现，
//! 漏转发不会编译报错，但后果很实在：模拟器会被按 92160 B/s 估超时
//! （每个分片白等约 270 ms，手感明显发滞），`is_simulated()` 也会谎报 false。

use scope_core::{DevicePort, LinkError};
use scope_sim::{Scenario, SimDevice};
use scope_transport_serial::SerialDevice;

/// GUI 要连接的两种设备。
pub enum Transport {
    /// 内置模拟器。**不需要硬件**，GUI 的主要开发与演示路径。
    Sim(Box<SimDevice>),
    /// 串口真机。
    Serial(Box<SerialDevice>),
}

impl Transport {
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

    /// **别删。** 漏了它，`is_simulated()` 会对模拟器谎报 false，
    /// 依赖它决定"是否注册模拟器专属功能"的逻辑就会静默走错分支。
    fn is_simulated(&self) -> bool {
        match self {
            Transport::Sim(d) => d.is_simulated(),
            Transport::Serial(d) => d.is_simulated(),
        }
    }

    /// **别删。** 漏了它，模拟器会被按 92160 B/s 估超时（默认实现的值），
    /// 每个分片白等约 270 ms。模拟器实际是 `usize::MAX`。
    fn byte_rate(&self) -> u32 {
        match self {
            Transport::Sim(d) => d.byte_rate(),
            Transport::Serial(d) => d.byte_rate(),
        }
    }
}
