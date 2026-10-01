#!/usr/bin/env python3
"""
生成 Golden Test Vectors —— 跨语言契约的唯一真相源。

设计意图：
    本脚本是规格的**独立第三方实现**（不是从 C 或 Rust 代码推导出来的）。
    C 端与 Rust 端都必须产出与本文件生成的 frame_hex 逐字节相同的结果。
    任何一端实现有偏差 → 它的测试立刻红。

    这并不是多余的：如果向量由 C 实现生成、再由 C 测试验证，就是循环论证。

用法：
    python gen_vectors.py > vectors.json

校验：
    脚本启动时会自检 CRC-16/CCITT-FALSE 的标准校验值 "123456789" → 0x29B1。
    自检失败直接退出，不产出任何文件。
"""

import json
import sys
from pathlib import Path

SYNC = b"\xAA\x55"
VER = 0x10
HDR_LEN = 10
CRC_LEN = 2
CHUNK_HEADER_LEN = 12
CHUNK_SAMPLES_MAX = 1024
# 分片头 12 B 是算在 payload 之内的 —— 1024 样点 × u16 + 12 = 2060
MAX_PAYLOAD_TX = CHUNK_HEADER_LEN + CHUNK_SAMPLES_MAX * 2   # 2060
MAX_PAYLOAD_RX = 512

# ── 命令码（必须与 protocol.h / lib.rs 一致）──────────────────────────
CMD_GET_INFO = 0x0101
CMD_PING = 0x0102
CMD_GET_STATUS = 0x0103
CMD_RESET = 0x0104
CMD_GET_CONFIG = 0x0200
CMD_SET_SAMPLE_RATE = 0x0201
CMD_SET_CHANNEL = 0x0202
CMD_SET_TRIGGER = 0x0203
CMD_SET_ACQ = 0x0204
CMD_ARM = 0x0301
CMD_STOP = 0x0302
CMD_FORCE_TRIGGER = 0x0303
CMD_READ_BUFFER = 0x0401
CMD_MEASURE = 0x0501
CMD_ECHO = 0x0601
CMD_EVENT_TRIGGER = 0x0682
CMD_ERROR = 0x0701

FLAG_RESP = 1 << 0
FLAG_ERROR = 1 << 1
FLAG_MORE = 1 << 2
FLAG_LAST = 1 << 3
FLAG_EVENT = 1 << 4
FLAG_NO_REPLY = 1 << 5


def crc16(data: bytes) -> int:
    """CRC-16/CCITT-FALSE: poly=0x1021 init=0xFFFF refin/refout=false xorout=0"""
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            if crc & 0x8000:
                crc = ((crc << 1) ^ 0x1021) & 0xFFFF
            else:
                crc = (crc << 1) & 0xFFFF
    return crc


def build_frame(flags: int, seq: int, cmd: int, payload: bytes) -> bytes:
    """帧 = SYNC(2) VER(1) FLAGS(1) SEQ(2) CMD(2) LEN(2) PAYLOAD CRC(2)，全小端"""
    head = bytes([VER, flags]) + seq.to_bytes(2, "little") \
         + cmd.to_bytes(2, "little") + len(payload).to_bytes(2, "little")
    body = head + payload
    crc = crc16(body)          # CRC 覆盖 VER..PAYLOAD 末尾（不含 SYNC）
    return SYNC + body + crc.to_bytes(2, "little")


# ── 小端打包 helper ───────────────────────────────────────────────────
def u8(v):  return bytes([v & 0xFF])
def u16(v): return (v & 0xFFFF).to_bytes(2, "little")
def i16(v): return (v & 0xFFFF).to_bytes(2, "little")
def u32(v): return (v & 0xFFFFFFFF).to_bytes(4, "little")


def pack12_pair(s0: int, s1: int) -> bytes:
    """2 个 12-bit 样点 → 3 字节"""
    b0 = s0 & 0xFF
    b1 = ((s0 >> 8) & 0x0F) | ((s1 & 0x0F) << 4)
    b2 = (s1 >> 4) & 0xFF
    return bytes([b0, b1, b2])


def main() -> int:
    # ── 自检：CRC 标准校验值 ──────────────────────────────────────
    check = crc16(b"123456789")
    if check != 0x29B1:
        print(f"FATAL: CRC self-test failed: got 0x{check:04X}, expected 0x29B1",
              file=sys.stderr)
        return 1

    frames = []
    parser_cases = []

    def add_frame(name, flags, seq, cmd, payload, note=""):
        fr = build_frame(flags, seq, cmd, payload)
        frames.append({
            "name": name,
            "flags": flags,
            "seq": seq,
            "cmd": cmd,
            "payload_hex": payload.hex(),
            "frame_hex": fr.hex(),
            "frame_len": len(fr),
            "note": note,
        })
        return fr

    # ══ 1. 常规命令帧 ═══════════════════════════════════════════════
    add_frame("get_info_req", 0, 1, CMD_GET_INFO, b"",
              "最短帧：LEN=0，主机→设备")
    add_frame("ping_req", 0, 2, CMD_PING, bytes(range(16)),
              "PING 回显数据")
    add_frame("get_status_req", 0, 3, CMD_GET_STATUS, b"", "")
    add_frame("reset_req", 0, 4, CMD_RESET, u8(0xA5), "magic=0xA5 防误触发")
    add_frame("set_sample_rate_857143", 0, 5, CMD_SET_SAMPLE_RATE, u32(857143),
              "F103 默认档；设备须回显 actual_hz")
    add_frame("set_sample_rate_max", 0, 6, CMD_SET_SAMPLE_RATE, u32(0xFFFFFFFF),
              "u32 边界值")
    add_frame("set_channel_ch0", 0, 7, CMD_SET_CHANNEL,
              u8(0) + u8(1) + u8(0) + u8(0) + i16(-2048),
              "offset_lsb 为负 → 验证 i16 小端补码")
    add_frame("set_trigger_rising", 0, 8, CMD_SET_TRIGGER,
              u8(1) + u8(0) + u8(0) + u16(2048) + u16(2048) + u32(1000),
              "normal 模式，上升沿，电平 2048 LSB")
    add_frame("set_acq_single_minmax", 0, 9, CMD_SET_ACQ,
              u8(0) + u16(4096) + u8(2) + u16(16),
              "单次 / 4096 点 / MINMAX / 抽点 16")
    add_frame("arm_req", 0, 10, CMD_ARM, b"", "非幂等，靠设备 seq 去重缓存")
    add_frame("stop_req", 0, 11, CMD_STOP, b"", "")
    add_frame("force_trigger_req", 0, 12, CMD_FORCE_TRIGGER, b"", "")

    add_frame("read_buffer_1024", FLAG_RESP, 13, CMD_READ_BUFFER,
              u16(7) + u32(0) + u16(1024) + u8(0) + u8(0),
              "拉第一片：capture_id=7 start=0 count=1024 RAW16")
    add_frame("read_buffer_mid", FLAG_RESP, 14, CMD_READ_BUFFER,
              u16(7) + u32(2048) + u16(1024) + u8(0) + u8(0),
              "按绝对样点号随机读取 → 天然幂等")

    add_frame("measure_req", 0, 15, CMD_MEASURE,
              u16(7) + u32(0) + u32(4096) + u32(0x1F),
              "全测量项")

    # ══ 2. FLAGS 组合 ═══════════════════════════════════════════════
    add_frame("resp_ack", FLAG_RESP, 20, CMD_ARM, b"", "普通响应")
    add_frame("resp_chunk_more", FLAG_RESP | FLAG_MORE, 21, CMD_READ_BUFFER,
              u16(7) + u32(0) + u16(1024) + u16(1) + u8(0) + u8(0),
              "分片头 + flags=MORE（非末片）")
    add_frame("resp_chunk_last", FLAG_RESP | FLAG_LAST, 22, CMD_READ_BUFFER,
              u16(7) + u32(3072) + u16(1024) + u16(1) + u8(0) + u8(1),
              "末片 + 分片头 flags=LAST")
    add_frame("event_trigger", FLAG_EVENT, 23, CMD_EVENT_TRIGGER,
              u16(7) + u32(2048) + u32(123456) + u32(857143) + u32(4096),
              "设备主动上报触发完成")
    add_frame("no_reply", FLAG_NO_REPLY, 24, CMD_ECHO, u8(0xAB), "免回复")
    add_frame("error_unknown_cmd", FLAG_RESP | FLAG_ERROR, 25, CMD_ERROR,
              u16(0x0001) + u8(1) + u8(13) + b"unknown cmd",
              "错误帧：code=0x0001 severity=1")

    # ══ 3. 长度边界 ═════════════════════════════════════════════════
    add_frame("payload_1_byte", 0, 30, CMD_ECHO, b"\x00", "LEN=1 边界")
    add_frame("payload_512_max_rx", 0, 31, CMD_ECHO, bytes(512),
              "主机→设备的硬上限 PROTO_MAX_PAYLOAD_RX=512")

    # 真实的最大分片：12 B 分片头 + 1024 样点 × u16 = 2060 B payload
    max_chunk = (
        u16(3) + u32(0) + u16(CHUNK_SAMPLES_MAX) + u16(1) + u8(0) + u8(1)
        + b"\xAB\x0C" * CHUNK_SAMPLES_MAX
    )
    assert len(max_chunk) == MAX_PAYLOAD_TX
    add_frame("payload_max_tx_chunk", FLAG_RESP | FLAG_LAST, 32, CMD_READ_BUFFER,
              max_chunk,
              "设备→主机的硬上限 PROTO_MAX_PAYLOAD_TX=2060（12B 分片头 + 1024 样点）")

    # payload 全 0xFF 会让 LEN 字段很大，验证解析器的长度先验
    add_frame("payload_all_ff", FLAG_RESP | FLAG_LAST, 33, CMD_READ_BUFFER,
              b"\xFF" * MAX_PAYLOAD_TX, "全 0xFF payload：验证 LEN 先验证再读")

    # payload 内部含 "AA 55"：验证同步头不会被误判
    add_frame("payload_contains_sync", 0, 34, CMD_ECHO,
              b"\x11\xAA\x55\x22\xAA\x55\x33",
              "payload 内含同步头字节序列 —— 解析器不得误切")

    # ══ 4. PACK12 ══════════════════════════════════════════════════
    pack12_cases = []
    for s0, s1 in [(0, 0), (0x0FFF, 0x0FFF), (0x0FFF, 0x0000), (0x0000, 0x0FFF),
                   (0x0123, 0x0456), (0x0ABC, 0x0DEF), (2048, 2047)]:
        pack12_cases.append({
            "s0": s0, "s1": s1,
            "bytes_hex": pack12_pair(s0, s1).hex(),
        })

    # ══ 5. 解析器行为（粘包 / 拆包 / 重同步）═════════════════════════
    f_a = build_frame(0, 1, CMD_GET_INFO, b"")
    f_b = build_frame(FLAG_RESP, 1, CMD_GET_INFO, u32(0x00010000))
    f_c = build_frame(0, 2, CMD_PING, b"\x01\x02\x03")
    f_d = build_frame(FLAG_RESP, 2, CMD_PING, b"\x01\x02\x03")

    parser_cases.append({
        "name": "single_frame",
        "stream_hex": f_a.hex(),
        "expect_ok": [{"cmd": CMD_GET_INFO, "seq": 1, "flags": 0, "payload_hex": ""}],
        "expect_crc_err": 0, "expect_dropped": 0,
        "note": "单个完整帧",
    })

    parser_cases.append({
        "name": "two_frames_glued",
        "stream_hex": (f_c + f_d).hex(),
        "expect_ok": [
            {"cmd": CMD_PING, "seq": 2, "flags": 0, "payload_hex": "010203"},
            {"cmd": CMD_PING, "seq": 2, "flags": FLAG_RESP, "payload_hex": "010203"},
        ],
        "expect_crc_err": 0, "expect_dropped": 0,
        "note": "粘包：一次喂入两个完整帧",
    })

    parser_cases.append({
        "name": "garbage_prefix",
        "stream_hex": (b"\xDE\xAD\xBE\xEF" + f_c).hex(),
        "expect_ok": [{"cmd": CMD_PING, "seq": 2, "flags": 0, "payload_hex": "010203"}],
        "expect_crc_err": 0, "expect_dropped": 4,
        "note": "垃圾前缀：逐字节重同步，丢弃 4 字节",
    })

    # CRC 错帧后紧跟正确帧：必须丢 1 字节重扫而不是按帧长跳过
    bad = bytearray(f_c)
    bad[-1] ^= 0xFF           # 破坏 CRC
    parser_cases.append({
        "name": "crc_err_then_good",
        "stream_hex": (bytes(bad) + f_c).hex(),
        "expect_ok": [{"cmd": CMD_PING, "seq": 2, "flags": 0, "payload_hex": "010203"}],
        "expect_crc_err": 1, "expect_dropped": 0,
        "note": "CRC 错帧：丢 1 字节重同步后仍能解出后续正确帧",
    })

    # 超大 LEN：声称 4096 字节 payload（越界），必须丢弃重同步
    evil = SYNC + bytes([VER, 0]) + u16(99) + u16(CMD_ECHO) + u16(4096) + b"\x00" * 20
    parser_cases.append({
        "name": "oversized_len_resync",
        "stream_hex": (evil + f_c).hex(),
        "expect_ok": [{"cmd": CMD_PING, "seq": 2, "flags": 0, "payload_hex": "010203"}],
        "expect_crc_err": 0, "expect_dropped": 0,
        "note": "LEN=4096 越界：先验长度即丢弃，最终仍解出后续帧",
    })

    parser_cases.append({
        "name": "partial_frame_then_rest",
        "stream_hex": f_c.hex(),
        "split_at": 5,
        "expect_ok": [{"cmd": CMD_PING, "seq": 2, "flags": 0, "payload_hex": "010203"}],
        "expect_crc_err": 0, "expect_dropped": 0,
        "note": "拆包：前 5 字节喂一次返回 NEED_MORE，剩余再喂一次解出",
    })

    # ══ 6. 状态机命令合法性 ═════════════════════════════════════════
    state_matrix = []
    all_cmds = [CMD_GET_INFO, CMD_PING, CMD_GET_STATUS, CMD_RESET,
                CMD_GET_CONFIG, CMD_SET_SAMPLE_RATE, CMD_SET_CHANNEL,
                CMD_SET_TRIGGER, CMD_SET_ACQ, CMD_ARM, CMD_STOP,
                CMD_FORCE_TRIGGER, CMD_READ_BUFFER, CMD_MEASURE, CMD_ECHO]
    base_ok = {CMD_GET_INFO, CMD_PING, CMD_GET_STATUS, CMD_STOP}
    for state in range(5):
        row = {}
        for cmd in all_cmds:
            if cmd in base_ok:
                ok = True
            elif state in (0, 3):          # IDLE / DONE
                ok = True
            elif state in (1, 2):          # ARMED / STREAMING
                ok = (cmd == CMD_FORCE_TRIGGER)
            elif state == 4:               # FAULT
                ok = (cmd == CMD_RESET)
            else:
                ok = False
            row[str(cmd)] = ok
        state_matrix.append({"state": state, "allowed": row})

    # ══ 输出 ═══════════════════════════════════════════════════════
    out = {
        "_comment": "GENERATED by proto/tests/gen_vectors.py -- DO NOT EDIT BY HAND",
        "_source": "docs/03-protocol.md",
        "proto_ver": VER,
        "header_len": HDR_LEN,
        "max_payload_tx": MAX_PAYLOAD_TX,
        "max_payload_rx": MAX_PAYLOAD_RX,
        "crc_check": {
            "input_ascii": "123456789",
            "expected": check,
            "expected_hex": f"0x{check:04X}",
        },
        "frames": frames,
        "pack12": pack12_cases,
        "parser": parser_cases,
        "state_matrix": state_matrix,
    }

    json.dump(out, sys.stdout, indent=1, ensure_ascii=False)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
