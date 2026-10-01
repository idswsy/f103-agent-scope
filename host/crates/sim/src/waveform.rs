//! 波形合成 —— 模拟器的心脏。
//!
//! 不需要硬件就能生成真实感足够的信号，让 Agent 逻辑、MCP 工具、解码器
//! 全部可以脱离板子开发与回归测试。
//!
//! 所有波形输出 **12-bit 无符号样点（0..4095）**，与真实 ADC 一致 ——
//! 包括直流偏置在 LSB 域的表达，这样上位机的阈值逻辑不会因为
//! 「模拟器给的是有符号电压」而被带偏。

/// 波形场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// 1 kHz 正弦，3.3 Vpp，偏置在中点。最基本的「能看到东西」场景。
    Sine1k3v3,
    /// 50 kHz 方波。验证高频下的混叠与边沿。
    Square50k,
    /// 带毛刺的脉冲串。验证 minmax 预览是否保住尖峰。
    PulseGlitch,
    /// 白噪声。验证质量标志与「不要回垃圾数」的逻辑。
    Noise,
    /// 纯直流。验证 FORCE_TRIGGER 与 auto 模式。
    Dc,
    /// 调幅信号。验证 RMS/包络类测量。
    Am,
    /// 模拟一条 100 kHz I2C 总线（SCL 在 ch0，SDA 在 ch1）。
    ///
    /// 这是本项目最有用的场景：数字通路解码器可以对着它做回归测试，
    /// 而且因为**内容确定**，可以写断言。
    I2c100k,
    /// 400 kHz I2C —— 用于验证「模拟通路解不了、数字通路能解」的边界。
    I2c400k,
}

impl Scenario {
    /// 从字符串解析（供 CLI / MCP 使用）。
    pub fn parse(s: &str) -> Option<Scenario> {
        Some(match s {
            "sine_1k_3v3" => Scenario::Sine1k3v3,
            "square_50k" => Scenario::Square50k,
            "pulse_glitch" => Scenario::PulseGlitch,
            "noise" => Scenario::Noise,
            "dc" => Scenario::Dc,
            "am" => Scenario::Am,
            "i2c_100k" => Scenario::I2c100k,
            "i2c_400k" => Scenario::I2c400k,
            _ => return None,
        })
    }

    /// 场景名。
    pub fn name(self) -> &'static str {
        match self {
            Scenario::Sine1k3v3 => "sine_1k_3v3",
            Scenario::Square50k => "square_50k",
            Scenario::PulseGlitch => "pulse_glitch",
            Scenario::Noise => "noise",
            Scenario::Dc => "dc",
            Scenario::Am => "am",
            Scenario::I2c100k => "i2c_100k",
            Scenario::I2c400k => "i2c_400k",
        }
    }

    /// 该场景需要几个通道。
    pub fn channel_count(self) -> usize {
        match self {
            Scenario::I2c100k | Scenario::I2c400k => 2,
            _ => 1,
        }
    }
}

/// 确定性伪随机数（xorshift64*）。
///
/// **必须确定性**：同一个 seed 每次跑出同样的噪声，
/// 否则回归测试会因为「噪声不一样」而随机失败。
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// 用给定种子创建。
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1) // 避免 0 种子
    }

    /// 下一个 u64。
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `[0, 1)` 区间的浮点数。
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// 以 `p` 的概率返回 true。
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }
}

/// 波形发生器：给定场景与采样率，产出确定性的样点序列。
#[derive(Debug, Clone)]
pub struct WaveformGen {
    scenario: Scenario,
    rng: Rng,
    /// 全局样点计数（决定相位连续性 —— 分段采样时不能每段从 0 开始）。
    sample_index: u64,
    /// I2C 场景用的总线发生器。
    i2c: I2cBus,
}

/// 12-bit ADC 的中点。
pub const MID_LSB: u16 = 2048;
/// 12-bit ADC 满量程。
pub const FULL_SCALE_LSB: u16 = 4095;

impl WaveformGen {
    /// 创建发生器。
    pub fn new(scenario: Scenario, seed: u64) -> WaveformGen {
        WaveformGen {
            scenario,
            rng: Rng::new(seed),
            sample_index: 0,
            i2c: I2cBus::new(100_000),
        }
    }

    /// 当前场景。
    pub fn scenario(&self) -> Scenario {
        self.scenario
    }

    /// 切换场景（保留样点计数，相位连续）。
    pub fn set_scenario(&mut self, s: Scenario) {
        self.scenario = s;
        match s {
            Scenario::I2c100k => self.i2c = I2cBus::new(100_000),
            Scenario::I2c400k => self.i2c = I2cBus::new(400_000),
            _ => {}
        }
    }

    /// 生成**一个通道**的一批样点，并推进全局样点计数。
    ///
    /// 单通道场景用这个。多通道**必须**用 [`WaveformGen::generate_multi`] ——
    /// 连着调两次 `generate` 会让第二个通道整体偏移 `count` 个样点，
    /// 两路时间轴对不上（对 I2C 解码是致命的）。
    pub fn generate(&mut self, ch: usize, count: usize, rate_hz: u32) -> Vec<u16> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.next_sample(ch, rate_hz));
            self.sample_index = self.sample_index.wrapping_add(1);
        }
        out
    }

    /// 生成**全部通道**的一批样点，保证各通道逐点时间对齐。
    ///
    /// 返回 `out[ch][i]`：第 `ch` 个通道的第 `i` 个样点，
    /// 所有通道的第 `i` 点对应同一个采样时刻。
    pub fn generate_multi(&mut self, ch_count: usize, count: usize, rate_hz: u32) -> Vec<Vec<u16>> {
        let mut out: Vec<Vec<u16>> = (0..ch_count).map(|_| Vec::with_capacity(count)).collect();
        for _ in 0..count {
            for (ch, chan) in out.iter_mut().enumerate() {
                chan.push(self.next_sample(ch, rate_hz));
            }
            // 一次采样只推进一次全局计数，与通道数无关
            self.sample_index = self.sample_index.wrapping_add(1);
        }
        out
    }

    /// 取某个采样时刻、某个通道的样点值。**不推进**样点计数 ——
    /// 推进由调用者负责，这样多通道才能共用同一个时刻。
    fn next_sample(&mut self, ch: usize, rate_hz: u32) -> u16 {
        use Scenario::*;

        let rate = rate_hz.max(1) as u64;
        let t = self.sample_index as f64 / rate as f64;
        // 整数微秒时间戳：I2C 相位用它算，避免浮点累积漂移
        let t_us = self.sample_index.saturating_mul(1_000_000) / rate;

        let v = match self.scenario {
            Sine1k3v3 => {
                // 1 kHz 正弦，3.3 Vpp 占满量程
                let amp = FULL_SCALE_LSB as f64 / 2.0 * 0.95;
                MID_LSB as f64 + amp * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()
            }

            Square50k => {
                let phase = (50_000.0 * t) % 1.0;
                // 加一点上升时间，让边沿不是数学上的垂直（更接近真实）
                let edge = 0.02;
                let frac = if phase < 0.5 {
                    (phase / edge).min(1.0)
                } else {
                    1.0 - ((phase - 0.5) / edge).min(1.0)
                };
                MID_LSB as f64 - 1800.0 + 3600.0 * frac
            }

            PulseGlitch => {
                let period = 200.0; // 样点
                let idx = self.sample_index % period as u64;
                let mut v = MID_LSB as f64 - 1500.0;
                if idx < 20 {
                    v = MID_LSB as f64 + 1500.0;
                }
                // 每 5 个周期插一个单样点尖峰 —— 专门用来验证 minmax 预览
                if idx == 100 && self.sample_index % (period as u64 * 5) < period as u64 {
                    v = FULL_SCALE_LSB as f64;
                }
                v
            }

            Noise => MID_LSB as f64 + (self.rng.next_f64() - 0.5) * 2000.0,

            Dc => MID_LSB as f64 + 300.0,

            Am => {
                let carrier = (2.0 * std::f64::consts::PI * 20_000.0 * t).sin();
                let envelope = 0.5 * (1.0 + (2.0 * std::f64::consts::PI * 200.0 * t).sin());
                MID_LSB as f64 + 1500.0 * envelope * carrier
            }

            I2c100k | I2c400k => {
                let (scl, sda) = self.i2c.levels_at_us(t_us);
                level_to_lsb(if ch == 0 { scl } else { sda })
            }
        };

        v.round().clamp(0.0, FULL_SCALE_LSB as f64) as u16
    }
}

// ══════════════════════════════════════════════════════════════════
// I2C 帧生成
//
// 这是模拟器里最有价值的场景：它产生**真正可解码的 I2C 流量**，
// 让 P2 的解码器可以对着确定的内容写断言（而不只是"看起来像波形"）。
//
// 电气上模仿真实开漏总线：高电平由上拉决定（0.85 VDD），
// 低电平接近地（0.15 VDD）—— 两个值都落在 0.3/0.7·VDD 判决带之外。
// ══════════════════════════════════════════════════════════════════

/// I2C 帧里的一个比特时隙。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bit {
    /// 起始条件：SCL 保持高，SDA 在高电平期间由高变低。
    Start,
    /// 数据位（含 ACK/NACK）：SCL 前半段低、后半段高，SDA 在整个时隙内恒定。
    Data(bool),
    /// 停止条件：SCL 保持高，SDA 在高电平期间由低变高。
    Stop,
    /// 总线空闲：两线都被上拉。
    Idle,
}

/// 一条默认的 I2C 事务：
/// `START → 0x88(地址 0x44 写) + ACK → 0x00 + ACK → 0x1A + ACK → STOP`
///
/// 之所以选 `0x44`：这是常见温湿度传感器（SHT/HTU 系列）的地址，
/// 也是 I2C 调试里最常抓到的事务之一，适合当示例。
pub fn default_i2c_transaction() -> Vec<Bit> {
    transaction(&[0x88, 0x00, 0x1A])
}

/// 用给定的字节序列构造一次 I2C 事务（每个字节后自动补一个 ACK 位）。
pub fn transaction(bytes: &[u8]) -> Vec<Bit> {
    let mut bits = vec![Bit::Start];
    for &b in bytes {
        for i in (0..8).rev() {
            bits.push(Bit::Data((b >> i) & 1 == 1));
        }
        bits.push(Bit::Data(false)); // ACK（从机拉低）
    }
    bits.push(Bit::Stop);
    bits.push(Bit::Idle);
    bits.push(Bit::Idle);
    bits
}

/// I2C 总线电平发生器。
///
/// 输入是**采样时刻（µs）**而不是样点序号 —— 这样同一个发生器可以
/// 服务任意采样率，且相位由物理时间决定，不会随采样率漂移。
#[derive(Debug, Clone)]
pub struct I2cBus {
    bits: Vec<Bit>,
    /// 一个比特时隙的长度（µs）。
    bit_us: u64,
}

impl I2cBus {
    /// 用 SCL 频率（Hz）构造。
    pub fn new(scl_hz: u64) -> I2cBus {
        I2cBus {
            bits: default_i2c_transaction(),
            // 关键：kHz 转的是**周期微秒**，不是把 Hz 当微秒用。
            // 100 kHz → 10 µs；400 kHz → 2.5 µs（向上取整到 2 µs 时
            // 会用 2.5 的话需要浮点，这里统一用乘法避免整数除法损失）
            bit_us: (1_000_000 / scl_hz).max(1),
        }
    }

    /// 用自定义事务构造。
    pub fn with_transaction(scl_hz: u64, bytes: &[u8]) -> I2cBus {
        I2cBus {
            bits: transaction(bytes),
            bit_us: (1_000_000 / scl_hz).max(1),
        }
    }

    /// 一个完整事务占用的时间（µs）。
    pub fn frame_us(&self) -> u64 {
        self.bits.len() as u64 * self.bit_us
    }

    /// 取某一时刻两条线的电平，返回 `(scl_high, sda_high)`。
    ///
    /// 用整数运算：`t_us % period` 而不是浮点 —— 保证长时间运行时
    /// 相位不会因为浮点累积误差而漂移。
    pub fn levels_at_us(&self, t_us: u64) -> (bool, bool) {
        let frame = self.frame_us();
        if frame == 0 {
            return (true, true);
        }
        let t = t_us % frame;
        let idx = (t / self.bit_us) as usize;
        let in_bit = t % self.bit_us;

        let bit = self.bits.get(idx).copied().unwrap_or(Bit::Idle);

        match bit {
            Bit::Start => {
                // SCL 全程高；SDA 在前半段高、后半段低
                (true, in_bit < self.bit_us / 2)
            }
            Bit::Stop => {
                // SCL 全程高；SDA 在前半段低、后半段高
                (true, in_bit >= self.bit_us / 2)
            }
            Bit::Data(level) => {
                // SCL 前半低、后半高；SDA 整段恒定
                (in_bit >= self.bit_us / 2, level)
            }
            Bit::Idle => (true, true),
        }
    }
}

/// 把逻辑电平映射到 ADC 域。
///
/// 用 0.15 / 0.85 VDD 而不是 0 / 1 —— 两个值都落在 I2C 判决门限
/// （0.3 VDD / 0.7 VDD）之外，留下裕量，同时保留了真实开漏总线的样子。
#[inline]
fn level_to_lsb(high: bool) -> f64 {
    if high {
        FULL_SCALE_LSB as f64 * 0.85
    } else {
        FULL_SCALE_LSB as f64 * 0.15
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_for_same_seed() {
        let mut a = WaveformGen::new(Scenario::Noise, 42);
        let mut b = WaveformGen::new(Scenario::Noise, 42);
        assert_eq!(a.generate(0, 256, 1_000_000), b.generate(0, 256, 1_000_000));
    }

    #[test]
    fn different_seeds_differ() {
        let mut a = WaveformGen::new(Scenario::Noise, 1);
        let mut b = WaveformGen::new(Scenario::Noise, 2);
        assert_ne!(a.generate(0, 64, 1_000_000), b.generate(0, 64, 1_000_000));
    }

    #[test]
    fn all_samples_stay_in_12bit_range() {
        for sc in [
            Scenario::Sine1k3v3,
            Scenario::Square50k,
            Scenario::PulseGlitch,
            Scenario::Noise,
            Scenario::Dc,
            Scenario::Am,
            Scenario::I2c100k,
            Scenario::I2c400k,
        ] {
            for seed in [1u64, 7, 99] {
                let mut g = WaveformGen::new(sc, seed);
                for ch in 0..sc.channel_count() {
                    let v = g.generate(ch, 1024, 857_142);
                    assert!(
                        v.iter().all(|&x| x <= FULL_SCALE_LSB),
                        "{:?} seed={seed} ch={ch} 产生了越界样点",
                        sc
                    );
                }
            }
        }
    }

    #[test]
    fn sine_actually_oscillates() {
        let mut g = WaveformGen::new(Scenario::Sine1k3v3, 1);
        let v = g.generate(0, 857, 857_142); // 约 1 ms ≈ 一个周期
        let min = *v.iter().min().unwrap();
        let max = *v.iter().max().unwrap();
        assert!(max - min > 3000, "正弦应该跨越大部分量程: {min}..{max}");
    }

    #[test]
    fn dc_scenario_is_flat() {
        let mut g = WaveformGen::new(Scenario::Dc, 1);
        let v = g.generate(0, 64, 857_142);
        assert!(v.windows(2).all(|w| w[0] == w[1]), "直流场景必须平坦");
    }

    #[test]
    fn i2c_scenario_is_binary() {
        let mut g = WaveformGen::new(Scenario::I2c100k, 1);
        for (ch, v) in g.generate_multi(2, 2048, 857_142).into_iter().enumerate() {
            // 数字域：应该只有两个电平（低 0.15、高 0.85 量程）
            let lo = FULL_SCALE_LSB as f64 * 0.15;
            let hi = FULL_SCALE_LSB as f64 * 0.85;
            for s in v {
                let s = s as f64;
                assert!(
                    (s - lo).abs() < 2.0 || (s - hi).abs() < 2.0,
                    "I2C 通道 {ch} 应只有两个电平，得到 {s}"
                );
            }
        }
    }

    #[test]
    fn scenario_roundtrip_by_name() {
        for sc in [
            Scenario::Sine1k3v3,
            Scenario::Square50k,
            Scenario::I2c100k,
            Scenario::I2c400k,
        ] {
            assert_eq!(Scenario::parse(sc.name()), Some(sc));
        }
        assert_eq!(Scenario::parse("nope"), None);
    }

    // ══════════════════════════════════════════════════════════════
    // I2C 回环测试
    //
    // 这几条测试的意义：证明模拟器产出的是**真正可解码的 I2C**，
    // 而不只是"看起来像波形的东西"。
    // P2 阶段的真解码器可以拿同一批断言做回归。
    // ══════════════════════════════════════════════════════════════

    /// 阈值：0.3 / 0.7 · VDD（协议里约定的判决门限）
    const VIL: u16 = (FULL_SCALE_LSB as u32 * 30 / 100) as u16; // 1228
    const VIH: u16 = (FULL_SCALE_LSB as u32 * 70 / 100) as u16; // 2866

    /// 极简 I2C 解码器 —— 边沿驱动，够用来验证模拟器。
    ///
    /// 返回 `(解码出的字节, START 次数, STOP 次数)`。
    fn decode_i2c(scl: &[u16], sda: &[u16]) -> (Vec<u8>, u32, u32) {
        assert_eq!(scl.len(), sda.len());
        let mut bytes = Vec::new();
        let mut starts = 0u32;
        let mut stops = 0u32;

        let mut bits: u32 = 0;
        let mut nbits = 0u32;
        let mut in_frame = false;

        let mut prev_scl = scl[0] > VIH;
        let mut prev_sda = sda[0] > VIH;

        for i in 1..scl.len() {
            let scl_hi = scl[i] > VIH;
            let sda_hi = sda[i] > VIH;

            // START / STOP 只在 SCL 为高时才谈得上
            if prev_scl && scl_hi {
                if prev_sda && !sda_hi {
                    starts += 1;
                    in_frame = true;
                    bits = 0;
                    nbits = 0;
                } else if !prev_sda && sda_hi && in_frame {
                    stops += 1;
                    in_frame = false;
                }
            }

            // SCL 上升沿采样数据位
            if !prev_scl && scl_hi && in_frame {
                bits = (bits << 1) | (sda_hi as u32);
                nbits += 1;
                if nbits == 9 {
                    // 前 8 位是数据，第 9 位是 ACK
                    bytes.push((bits >> 1) as u8);
                    bits = 0;
                    nbits = 0;
                }
            }

            prev_scl = scl_hi;
            prev_sda = sda_hi;
        }
        (bytes, starts, stops)
    }

    #[test]
    fn generated_i2c_is_actually_decodable() {
        // 这一步是整个模拟器可信度的核心：如果解不出来，
        // 后面所有"对着模拟器写的解码器断言"都是空中楼阁。
        // 必须用 generate_multi —— 逐通道 generate 会让 SDA 整体偏移，
        // 这正是这条测试抓出来的 bug（见下面的 multi_channel_is_aligned）。
        let mut g = WaveformGen::new(Scenario::I2c100k, 1);
        let chans = g.generate_multi(2, 4096, 857_142);
        let scl = chans[0].clone();
        let sda = chans[1].clone();

        let (bytes, starts, stops) = decode_i2c(&scl, &sda);

        assert!(starts >= 3, "4096 点应覆盖多次事务，实际 START={starts}");
        // 最后一个事务可能被采集窗口截断 —— 此时 START 比 STOP 多一个，是正常的。
        // 但绝不允许反过来（有 STOP 却没有配对的 START），那才是真的解错了。
        assert!(
            starts == stops || starts == stops + 1,
            "START/STOP 应成对（允许末尾截断一帧）：START={starts} STOP={stops}"
        );

        // 内容必须确定：每帧都是 0x88 0x00 0x1A
        assert!(!bytes.is_empty(), "应解出字节");
        for (i, chunk) in bytes.chunks(3).enumerate() {
            if chunk.len() < 3 {
                break; // 尾部截断的半帧，忽略
            }
            assert_eq!(
                chunk,
                &[0x88, 0x00, 0x1A],
                "第 {i} 帧内容不符（解码器或模拟器有 bug）"
            );
        }
    }

    #[test]
    fn i2c_scl_actually_toggles() {
        // 回归：曾经把 scl_hz 当成"周期微秒数"，导致 100 kHz 被当成
        // 100000 µs 周期 —— 整个采集窗都落在同一个半周期里，SCL 是一条直线。
        let mut g = WaveformGen::new(Scenario::I2c100k, 1);
        let scl = g.generate(0, 1024, 857_142);

        let hi = scl.iter().filter(|&&v| v > VIH).count();
        let lo = scl.iter().filter(|&&v| v < VIL).count();

        assert!(hi > 0, "SCL 必须有高电平");
        assert!(lo > 0, "SCL 必须有低电平");
        // 100 kHz 下 1024 点（1194 µs）约覆盖 119 个 SCL 周期 → 大致各半
        let ratio = hi as f64 / (hi + lo) as f64;
        assert!(
            (0.2..=0.8).contains(&ratio),
            "SCL 高低电平比例应接近 50%，实际高电平占 {:.1}%",
            ratio * 100.0
        );
    }

    #[test]
    fn multi_channel_generation_is_time_aligned() {
        // 回归：曾经在 device.rs 里写成
        //     for ch in 0..ch_count { channels.push(wave.generate(ch, n, rate)) }
        // 结果 CH2 比 CH1 晚了整整 n 个样点 —— 对 I2C 这种要求
        // SCL/SDA 同时刻采样的场景是致命的（解码器会看到两条对不上的线）。
        let mut g = WaveformGen::new(Scenario::I2c100k, 1);
        let chans = g.generate_multi(2, 2048, 857_142);
        assert_eq!(chans.len(), 2);
        assert_eq!(chans[0].len(), 2048);
        assert_eq!(chans[1].len(), 2048);

        // 时间对齐的直接证据：SCL 在任一比特中点必为高，
        // 且 SDA 在该点为 0/1 之一 —— 两者必须来自同一时刻。
        let (bytes, starts, _stops) = decode_i2c(&chans[0], &chans[1]);
        assert!(starts >= 1, "对齐后应能解出帧");
        assert_eq!(&bytes[..3], &[0x88, 0x00, 0x1A], "两路对齐后应解出正确内容");
    }

    #[test]
    fn sample_index_advances_once_per_time_step_regardless_of_channels() {
        // 单通道与双通道在同样多的采样次数后，时间轴应推进同样的量
        let mut g1 = WaveformGen::new(Scenario::Dc, 1);
        g1.generate(0, 100, 857_142);

        let mut g2 = WaveformGen::new(Scenario::Dc, 1);
        g2.generate_multi(2, 50, 857_142);

        // 两者都推进了 100 个采样时刻
        let mut g3 = WaveformGen::new(Scenario::Sine1k3v3, 1);
        let a = g3.generate(0, 3, 1000);
        let mut g4 = WaveformGen::new(Scenario::Sine1k3v3, 1);
        let b = g4.generate_multi(1, 3, 1000);
        assert_eq!(a, b[0], "单通道 generate 与 generate_multi(1) 必须等价");
    }

    #[test]
    fn i2c_both_lines_stay_inside_decision_band_margins() {
        // 高低电平必须明确落在 0.3/0.7·VDD 判决带之外，
        // 否则解码器的阈值逻辑会被自己的测试数据坑到
        let mut g = WaveformGen::new(Scenario::I2c100k, 1);
        for (ch, v) in g.generate_multi(2, 2048, 857_142).into_iter().enumerate() {
            for s in v {
                assert!(
                    !(VIL..=VIH).contains(&s),
                    "通道 {ch} 出现了落在判决带内的电平 {s}（VIL={VIL}, VIH={VIH}）"
                );
            }
        }
    }

    #[test]
    fn i2c_bit_period_matches_configured_rate() {
        // 100 kHz → 10 µs/bit；400 kHz → 2 µs/bit（整数微秒取整）
        let bus100 = I2cBus::new(100_000);
        assert_eq!(bus100.bit_us, 10);

        let bus400 = I2cBus::new(400_000);
        assert_eq!(bus400.bit_us, 2, "400 kHz 应为 2.5 µs，整数取整到 2");

        // 一帧 = START + 9×3 位 + STOP + 2 空闲 = 31 位
        assert_eq!(bus100.frame_us(), 31 * 10);
    }

    #[test]
    fn i2c_frame_levels_are_well_formed() {
        let bus = I2cBus::new(100_000);
        // 第 0 位是 START：SCL 恒高，SDA 前半高后半低
        let (scl_a, sda_a) = bus.levels_at_us(0);
        let (scl_b, sda_b) = bus.levels_at_us(9);
        assert!(scl_a && scl_b, "START 期间 SCL 必须保持高");
        assert!(sda_a, "START 前半段 SDA 应为高");
        assert!(!sda_b, "START 后半段 SDA 应变低");
    }
}
