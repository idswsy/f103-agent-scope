//! 定点测量 —— 对一个通道算出峰峰值、频率、占空比、上升时间。
//!
//! 主统计量直接用 [`Capture::summary`] 已经算好的结果，这里只补三样它没有的：
//! 平均频率、占空比、上升时间。
//!
//! # 三个容易算错的地方（都踩过）
//!
//! 1. **幅值与电平的换算不一样**。峰峰值 / RMS 是**幅值**，只乘 V/LSB；
//!    最小值 / 最大值 / 均值是**电平**，要先减零点再乘。混用会让
//!    2.31 V 的峰峰值显示成 0.66 V（甚至把 RMS 算成负数）。
//! 2. **频率不能用「边沿数 ÷ 总时长」**。I2C 在两次事务之间 SCL 保持高电平，
//!    那段时间没有边沿却占着时长 —— 100 kHz 会被算成 87 kHz。
//! 3. **周期估计要「先筛后平均」**。用中位数筛掉空闲段，再对保留下来的间隔
//!    取均值。直接用中位数当周期也不行：8.57 个采样点的周期会让 8/9 交替的
//!    采样抖动把中位数带到 9 上，报出 95.2 kHz。

use crate::capture::{Capture, ChannelScale};

/// 一个通道的测量结果。
///
/// 可选字段为 `None` 表示**测不出来**（直流没有周期、没有边沿就没有上升时间），
/// 而不是 0 —— 项目纪律：宁可说不知道，也不给看似精确的垃圾数。
#[derive(Debug, Clone, PartialEq)]
pub struct Measurements {
    /// 峰峰值（V）。
    pub vpp: f64,
    /// 最小值（V）。
    pub min: f64,
    /// 最大值（V）。
    pub max: f64,
    /// 均值（V）。
    pub mean: f64,
    /// 扣除直流后的 RMS（波动），也就是标准差 —— 交流信号的有效值看这个。
    pub ac_rms: f64,
    /// 含直流的真 RMS。直流信号也有值，别拿它当"有效值"理解。
    pub rms: f64,
    /// 平均频率（Hz）。没有上升沿则为 `None`。
    pub freq_hz: Option<f64>,
    /// 高电平占比（%）。没有边沿则为 `None`。
    pub duty_pct: Option<f64>,
    /// 10%→90% 上升时间（ns）。找不到干净的边沿则为 `None`。
    pub rise_ns: Option<f64>,
}

/// 对一个通道做测量。
///
/// 主统计量直接用 core 已经算好的 [`scope_core::ChannelSummary`]，
/// 这里只补三样它没有的：平均频率、占空比、上升时间。
pub fn measure(cap: &Capture, ch: usize, scale: &ChannelScale) -> Option<Measurements> {
    let sum = cap.summary(ch)?;
    let s = cap.samples(ch)?;

    // ⚠ 两种换算不能混：
    //   电平（min/max/mean）→ 减零点再乘 V/LSB
    //   幅值（Vpp/RMS/AC-RMS）→ **只乘 V/LSB**，减零点会把幅值算成电平
    // 回归：这里曾经一律用 lsb_to_volts，于是 Vpp 2.31 V 被报成 0.66 V，
    // 而 AC-RMS 直接变成了负数。
    let vpl = scale.volts_per_lsb;
    let level = |lsb: f64| scale.lsb_to_volts(lsb.round().clamp(0.0, 4095.0) as u16);
    let magnitude = |lsb: f64| lsb * vpl;

    let (mut lo, mut hi) = (u16::MAX, u16::MIN);
    for &v in s {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi <= lo {
        // 直流：有电平，但没有周期/占空比/上升时间可言
        return Some(Measurements {
            vpp: 0.0,
            min: level(lo as f64),
            max: level(hi as f64),
            mean: level(sum.mean_lsb),
            ac_rms: magnitude(sum.ac_rms_lsb),
            rms: magnitude(sum.rms_lsb),
            freq_hz: None,
            duty_pct: None,
            rise_ns: None,
        });
    }

    let mid = (lo as f64 + hi as f64) / 2.0;
    let edges = rising_edges(s, mid);

    // ── 周期：取相邻上升沿间隔的**中位数** ──
    //
    // 不能拿「边沿数 ÷ 总时长」：I2C 在两次事务之间 SCL 保持高电平，
    // 那段时间没有边沿却占着时长，于是 100 kHz 的 SCL 会被算成 87 kHz。
    // 空闲间隔是时钟周期的几十倍，落在中位数之外，自然被排除。
    // 先用中位数定出「真实周期」的量级（它对空闲段的几十倍间隔免疫），
    // 再对**通过筛选的间隔取均值**。
    //
    // 为什么最终用均值而不是中位数：采样周期 8.57 个点时，相邻间隔是 8/9
    // 交替抖动，中位数会落在 9 上 → 报 95.2 kHz；均值才回到 8.57 → 100.0 kHz。
    // 回归：这里曾经直接用中位数当周期。
    let period = median_gap(&edges).map(|median| {
        let cutoff = median * 1.5;
        let kept: Vec<f64> = edges
            .windows(2)
            .map(|w| (w[1] - w[0]) as f64)
            .filter(|g| *g <= cutoff)
            .collect();
        if kept.is_empty() {
            median
        } else {
            kept.iter().sum::<f64>() / kept.len() as f64
        }
    });
    let freq_hz = period.map(|p| cap.rate_hz as f64 / p);

    // ── 占空比：只统计「真实时钟周期」内的高电平占比 ──
    let duty_pct = period.map(|p| {
        let cutoff = p * 1.6;
        let (mut high, mut total) = (0usize, 0usize);
        for w in edges.windows(2) {
            let gap = (w[1] - w[0]) as f64;
            if gap > cutoff {
                continue; // 空闲段，跳过
            }
            for &v in &s[w[0]..w[1]] {
                if v as f64 > mid {
                    high += 1;
                }
                total += 1;
            }
        }
        if total == 0 {
            return None;
        }
        Some(high as f64 * 100.0 / total as f64)
    });

    Some(Measurements {
        vpp: magnitude(sum.pp_lsb as f64),
        min: level(lo as f64),
        max: level(hi as f64),
        mean: level(sum.mean_lsb),
        ac_rms: magnitude(sum.ac_rms_lsb),
        rms: magnitude(sum.rms_lsb),
        freq_hz,
        duty_pct: duty_pct.flatten(),
        rise_ns: rise_time_ns(s, cap.rate_hz, lo, hi),
    })
}

/// 上升沿所在的样点索引（用中点判定）。
fn rising_edges(s: &[u16], mid: f64) -> Vec<usize> {
    (1..s.len())
        .filter(|&i| (s[i - 1] as f64) < mid && (s[i] as f64) >= mid)
        .collect()
}

/// 相邻上升沿间隔的中位数 —— 对「真实周期」的稳健估计。
fn median_gap(edges: &[usize]) -> Option<f64> {
    if edges.len() < 3 {
        return None; // 少于 3 个边沿，中位数没有意义
    }
    let mut gaps: Vec<f64> = edges.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(gaps[gaps.len() / 2])
}

/// 10% → 90% 上升时间（ns）。
///
/// 找**第一个从下半区跨到上半区的边沿**，然后在它之后找 10% 与 90% 的穿越点。
/// 搜索不设小窗口 —— 1 kHz 正弦的上升沿本身就要几百微秒，窗口开小了会漏。
fn rise_time_ns(s: &[u16], rate_hz: u32, lo: u16, hi: u16) -> Option<f64> {
    if rate_hz == 0 || s.len() < 4 || hi <= lo {
        return None;
    }
    let (lo_f, hi_f) = (lo as f64, hi as f64);
    let mid = (lo_f + hi_f) / 2.0;
    let t10 = lo_f + (hi_f - lo_f) * 0.1;
    let t90 = lo_f + (hi_f - lo_f) * 0.9;

    // 第一个上行穿越
    let start = (1..s.len()).find(|&i| (s[i - 1] as f64) < mid && (s[i] as f64) >= mid)?;

    // 从这里往后找 10% / 90% 的穿越点（插值定位，采样间隔就是分辨率下限）
    let mut c10: Option<f64> = None;
    let mut c90: Option<f64> = None;
    for i in (start.max(1))..s.len() {
        let a = s[i - 1] as f64;
        let b = s[i] as f64;
        if a == b {
            continue;
        }
        let cross = |t: f64| -> f64 { (i - 1) as f64 + (t - a) / (b - a) };
        if c10.is_none() && a < t10 && b >= t10 {
            c10 = Some(cross(t10));
        }
        if c10.is_some() && c90.is_none() && a < t90 && b >= t90 {
            c90 = Some(cross(t90));
            break;
        }
    }
    match (c10, c90) {
        (Some(a), Some(b)) if b > a => Some((b - a) * 1e9 / rate_hz as f64),
        _ => None,
    }
}
