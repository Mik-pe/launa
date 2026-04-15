#!/usr/bin/env python3
"""
Launa Sniffer Decoder — Real-time Balboa RS-485 frame decoder.

Subscribes to the MQTT sniff topic (launa/+/sniff) and decodes
Balboa BP6013G1 protocol frames in real-time on your PC.

Usage:
    python scripts/sniff-decode.py [--host localhost] [--port 1883] [--device-id launa_spa] [--save session.json]

Requirements:
    pip install paho-mqtt
"""

import argparse
import json
import signal
import sys
import time
from datetime import datetime

import paho.mqtt.client as mqtt

# ── ANSI Colors ────────────────────────────────────────────────────────────

class C:
    """ANSI color constants."""
    RESET = "\033[0m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    GREEN = "\033[32m"
    RED = "\033[31m"
    YELLOW = "\033[33m"
    CYAN = "\033[36m"
    MAGENTA = "\033[35m"
    WHITE = "\033[37m"
    BG_RED = "\033[41m"

    @staticmethod
    def support() -> bool:
        """Check if the terminal likely supports ANSI colors."""
        return hasattr(sys.stdout, "isatty") and sys.stdout.isatty()


# ── CRC-8 (Balboa) ────────────────────────────────────────────────────────
# Parameters: init=0x02, poly=0x07, reflect_in=False, reflect_out=False, xor_out=0x02

def crc8_compute(data: bytes) -> int:
    """Compute Balboa CRC-8 over the given data bytes."""
    crc = 0x02
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 0x80:
                crc = ((crc << 1) ^ 0x07) & 0xFF
            else:
                crc = (crc << 1) & 0xFF
    return crc ^ 0x02


# ── Frame Constants ────────────────────────────────────────────────────────

FRAME_MARKER = 0x7E
ESCAPE_CHAR = 0x7D


# ── HDLC Frame Decoder ────────────────────────────────────────────────────

def unescape_frame(data: bytes) -> bytes:
    """Remove HDLC-style byte stuffing from frame body."""
    result = bytearray()
    i = 0
    while i < len(data):
        if data[i] == ESCAPE_CHAR and i + 1 < len(data):
            result.append(data[i + 1] ^ 0x20)
            i += 2
        else:
            result.append(data[i])
            i += 1
    return bytes(result)


def parse_raw_frame(raw: bytes) -> dict | None:
    """
    Parse a raw Balboa frame (with or without 0x7E markers).

    Returns a dict with keys: message_type, payload, crc_ok, raw_bytes, frame_bytes
    or None if the data cannot be parsed as a frame.
    """
    # Strip 0x7E markers if present
    inner = raw
    if len(inner) >= 2 and inner[0] == FRAME_MARKER and inner[-1] == FRAME_MARKER:
        inner = inner[1:-1]

    if len(inner) < 4:
        return None

    # Un-escape
    inner = unescape_frame(inner)

    if len(inner) < 4:
        return None

    length = inner[0]
    expected_len = length + 2  # length byte + data + CRC byte

    if len(inner) < expected_len:
        return None

    # CRC is over bytes [0 .. length] (inclusive), CRC is at [length+1]
    body = inner[:length + 1]
    crc_byte = inner[length + 1]
    computed_crc = crc8_compute(body)
    crc_ok = computed_crc == crc_byte

    message_type = (inner[1], inner[2])
    payload = inner[3:length + 1]

    return {
        "message_type": message_type,
        "payload": payload,
        "crc_ok": crc_ok,
        "frame_length": length,
    }


# ── Hex Utilities ──────────────────────────────────────────────────────────

def hex_to_bytes(hex_str: str) -> bytes:
    """Convert a hex string to bytes, handling odd-length and whitespace."""
    hex_str = hex_str.strip()
    # Remove common prefixes
    for prefix in ("0x", "0X"):
        if hex_str.startswith(prefix):
            hex_str = hex_str[len(prefix):]
    # Remove whitespace
    hex_str = hex_str.replace(" ", "").replace("\t", "").replace("\n", "")
    if not hex_str:
        return b""
    # Pad to even length
    if len(hex_str) % 2 != 0:
        hex_str = "0" + hex_str
    try:
        return bytes.fromhex(hex_str)
    except ValueError:
        return b""


def hex_dump(data: bytes, width: int = 16) -> str:
    """Format bytes as a hex dump with ASCII sidebar."""
    lines = []
    for offset in range(0, len(data), width):
        chunk = data[offset:offset + width]
        hex_part = " ".join(f"{b:02X}" for b in chunk)
        ascii_part = "".join(
            chr(b) if 0x20 <= b < 0x7F else "." for b in chunk
        )
        lines.append(f"  {offset:04X}  {hex_part:<{width * 3 - 1}}  |{ascii_part}|")
    return "\n".join(lines)


# ── Message Type Identification ───────────────────────────────────────────

# Maps (type_byte_0, type_byte_1, optional_payload_byte_0) to a human-readable name
MESSAGE_TYPES = {
    # Status Update
    (0xFF, 0xAF): "Status Update",
    # Ready indicator
    (0x10, 0xBF): "Ready Indicator",
    # Registration messages
    (0xFE, 0xBF): "Registration",
}

# For 0x0A 0xBF messages, sub-type is first payload byte
OABF_SUBTYPES = {
    0x04: "Configuration Request",
    0x07: "No-op Ack",
    0x11: "Toggle Item",
    0x20: "Set Temperature",
    0x22: "Settings Request",
    0x23: "Filter Cycles Response",
    0x24: "Information Response",
    0x27: "Set Temperature Scale",
    0x28: "Fault Log Response",
    0x2E: "Control Configuration",
    0x94: "Configuration Response",
}

# For 0xFE 0xBF messages, sub-type is first payload byte
FEBF_SUBTYPES = {
    0x00: "Client ID Query",
    0x01: "Client ID Request",
    0x02: "Client ID Assignment",
    0x03: "Client ID Ack",
    0x06: "Ready to Send",
}


def identify_message(msg_type: tuple, payload: bytes) -> str:
    """Return a human-readable message type name."""
    if msg_type in MESSAGE_TYPES:
        name = MESSAGE_TYPES[msg_type]
        if msg_type == (0xFE, 0xBF) and len(payload) > 0:
            sub = payload[0]
            if sub in FEBF_SUBTYPES:
                return FEBF_SUBTYPES[sub]
            return f"Registration (sub 0x{sub:02X})"
        return name

    if msg_type == (0x0A, 0xBF) and len(payload) > 0:
        sub = payload[0]
        if sub in OABF_SUBTYPES:
            return OABF_SUBTYPES[sub]
        return f"0A BF (sub 0x{sub:02X})"

    return f"Unknown (0x{msg_type[0]:02X} 0x{msg_type[1]:02X})"


def msg_type_hex(msg_type: tuple) -> str:
    """Format message type bytes as hex string."""
    return f"{msg_type[0]:02X} {msg_type[1]:02X}"


# ── Status Update Decoder ─────────────────────────────────────────────────

FAULT_CODES = {
    15: "Sync",
    16: "Low Flow",
    17: "Flow Failed",
    18: "Settings Reset",
    19: "Priming",
    20: "Clock Failed",
    22: "Program Memory",
    26: "Sync — Call Service",
    27: "Heater Dry",
    28: "Heater Maybe Dry",
    29: "Water Too Hot",
    30: "Heater Too Hot",
    31: "Sensor A Fault",
    32: "Sensor B Fault",
    34: "Pump Stuck On",
    35: "Hot Fault",
    36: "GFCI Test Failed",
    37: "Standby / Hold",
}


def decode_status_update(payload: bytes) -> dict:
    """
    Decode a Status Update payload (message type FF AF 13).

    Payload layout (24 bytes):
     0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15 16 17 18 19 20 21 22 23
    ST IM CT HH MM HM RT SA SB F9 FA P1 P2 CB LF MR -- -- -- -- ST -- -- --
    """
    if len(payload) < 24:
        return {"error": f"Status payload too short ({len(payload)} bytes, need 24)"}

    # Temperature scale: bit 0 of byte 9
    is_celsius = bool(payload[9] & 0x01)
    temp_divisor = 2.0 if is_celsius else 1.0
    scale_label = "°C" if is_celsius else "°F"

    # Current temperature (byte 2): 0xFF = unknown
    if payload[2] == 0xFF:
        current_temp = None
        temp_str = "---"
    else:
        current_temp = payload[2] / temp_divisor
        temp_str = f"{current_temp:.0f}{scale_label}"

    # Set temperature (byte 20)
    set_temp = payload[20] / temp_divisor

    # Spa state (byte 0)
    spa_state = payload[0]
    spa_states = {0x00: "Running", 0x05: "Hold", 0x14: "A/B Temps", 0x17: "Test"}
    state_str = spa_states.get(spa_state, f"Unknown (0x{spa_state:02X})")

    # Init mode (byte 1)
    is_priming = payload[1] == 0x01
    is_hold = payload[0] == 0x05

    # Heating mode (byte 5): 0=Ready, 1=Rest, 3=Ready-in-Rest
    heating_modes = {0: "Ready", 1: "Rest", 3: "Ready-in-Rest"}
    heating_mode = heating_modes.get(payload[5] & 0x03, "Unknown")

    # Time (bytes 3-4)
    hour = payload[3]
    minute = payload[4]
    is_24h = bool(payload[9] & 0x02)

    if is_24h:
        time_str = f"{hour:02d}:{minute:02d}"
    else:
        ampm = "AM" if hour < 12 else "PM"
        h12 = hour % 12 or 12
        time_str = f"{h12}:{minute:02d} {ampm}"

    # Heating flags (byte 10): bit 2=temp range, bit 3=needs heat, bits 4-5=heating state
    is_heating = bool(payload[10] & 0x30)
    needs_heat = bool(payload[10] & 0x08)
    temp_range = "High" if payload[10] & 0x04 else "Low"

    # Filter mode (byte 9, bits 2-3)
    filter_mode = (payload[9] >> 2) & 0x03

    # Pumps (byte 11 = pumps 1-4, byte 12 = pumps 5-6)
    pump_names = {0: "OFF", 1: "LOW", 2: "HIGH"}
    p1 = payload[11]
    p2 = payload[12]
    pumps = [
        p1 & 0x03,
        (p1 >> 2) & 0x03,
        (p1 >> 4) & 0x03,
        (p1 >> 6) & 0x03,
        p2 & 0x03,
        (p2 >> 2) & 0x03,
    ]
    pump_labels = [pump_names.get(p, f"?{p}") for p in pumps]

    # Circ pump & blower (byte 13): circ=bit 1, blower=bits 2-3
    circ_pump = bool(payload[13] & 0x02)
    blower = bool(payload[13] & 0x0C)

    # Lights (byte 14): bits 0-1=Light1, bits 2-3=Light2
    light1 = bool(payload[14] & 0x03)
    light2 = bool(payload[14] & 0x0C)

    # Mister (byte 15)
    mister = bool(payload[15])

    return {
        "current_temp": temp_str,
        "set_temp": f"{set_temp:.0f}{scale_label}",
        "state": state_str,
        "heating_mode": heating_mode,
        "is_heating": is_heating,
        "needs_heat": needs_heat,
        "temp_range": temp_range,
        "time": time_str,
        "is_hold": is_hold,
        "is_priming": is_priming,
        "pumps": pump_labels,
        "circ_pump": circ_pump,
        "blower": blower,
        "light1": light1,
        "light2": light2,
        "mister": mister,
        "filter_mode": filter_mode,
    }


def decode_filter_cycles(payload: bytes) -> dict:
    """
    Decode a Filter Cycles Response payload (0A BF 23).
    Payload: 8 bytes: 1H 1M 1D 1E 2H 2M 2D 2E
    """
    if len(payload) < 9:  # sub-type byte + 8 data bytes
        return {"error": f"Filter cycles payload too short ({len(payload)} bytes)"}

    data = payload[1:]  # skip sub-type byte
    if len(data) < 8:
        return {"error": f"Filter cycles data too short ({len(data)} bytes)"}

    f1_enabled = True
    f2_enabled = bool(data[4] & 0x80)
    f2_hour = data[4] & 0x7F

    return {
        "filter1": {
            "start": f"{data[0]:02d}:{data[1]:02d}",
            "duration": f"{data[2]}h {data[3]}m",
            "enabled": f1_enabled,
        },
        "filter2": {
            "start": f"{f2_hour:02d}:{data[5]:02d}",
            "duration": f"{data[6]}h {data[7]}m",
            "enabled": f2_enabled,
        },
    }


def decode_information(payload: bytes) -> dict:
    """
    Decode an Information Response payload (0A BF 24).
    Payload: 21+ bytes.
    """
    if len(payload) < 22:  # sub-type + 21 data bytes
        return {"error": f"Information payload too short ({len(payload)} bytes)"}

    data = payload[1:]  # skip sub-type byte
    if len(data) < 21:
        return {"error": f"Information data too short ({len(data)} bytes)"}

    software_id = f"{data[0]:02X}{data[1]:02X}_{data[2]:02X}{data[3]:02X}"
    system_model = bytes(data[4:12]).decode("ascii", errors="replace").rstrip(" \x00")
    current_setup = data[12]
    config_sig = f"{data[13]:02X}{data[14]:02X}{data[15]:02X}{data[16]:02X}"

    heater_voltage = "240V" if data[17] == 0x01 else f"Unknown (0x{data[17]:02X})"
    heater_type = "Standard" if data[18] == 0x0A else f"Unknown (0x{data[18]:02X})"
    dip_switches = f"{data[19]:08b}{data[20]:08b}"

    return {
        "software_id": software_id,
        "system_model": system_model,
        "current_setup": current_setup,
        "config_signature": config_sig,
        "heater_voltage": heater_voltage,
        "heater_type": heater_type,
        "dip_switches": dip_switches,
    }


def decode_fault_log(payload: bytes) -> dict:
    """
    Decode a Fault Log Response payload (0A BF 28).
    Payload: 10+ bytes.
    """
    if len(payload) < 11:  # sub-type + 10 data bytes
        return {"error": f"Fault log payload too short ({len(payload)} bytes)"}

    data = payload[1:]  # skip sub-type byte
    if len(data) < 10:
        return {"error": f"Fault log data too short ({len(data)} bytes)"}

    fault_code = data[2]
    fault_name = FAULT_CODES.get(fault_code, f"Unknown (0x{fault_code:02X})")

    return {
        "fault_count": data[0],
        "entry_number": data[1],
        "fault_code": fault_code,
        "fault_name": fault_name,
        "days_ago": data[3],
        "time": f"{data[4]:02d}:{data[5]:02d}",
        "set_temperature": data[7],
        "sensor_a_temp": data[8],
        "sensor_b_temp": data[9],
    }


def decode_registration(payload: bytes) -> dict:
    """Decode a registration message (FE BF xx)."""
    if not payload:
        return {"sub_type": "empty"}

    sub = payload[0]
    result = {"sub_type": FEBF_SUBTYPES.get(sub, f"Unknown (0x{sub:02X})")}

    if sub == 0x02 and len(payload) >= 2:
        result["assigned_id"] = payload[1]

    return result


# ── Display Formatting ─────────────────────────────────────────────────────

def format_status_detail(decoded: dict) -> str:
    """Format decoded status update as a multi-line detail string."""
    lines = []
    lines.append(
        f"    Temp: {decoded['current_temp']} | "
        f"Set: {decoded['set_temp']} | "
        f"Heating: {decoded['heating_mode']} | "
        f"Range: {decoded['temp_range']}"
    )
    lines.append(
        f"    Hold: {'ON' if decoded['is_hold'] else 'OFF'} | "
        f"Priming: {'ON' if decoded['is_priming'] else 'OFF'} | "
        f"Heating: {'ON' if decoded['is_heating'] else 'OFF'} | "
        f"Time: {decoded['time']}"
    )

    pump_parts = []
    for i, label in enumerate(decoded["pumps"]):
        pump_parts.append(f"{i+1}={label}")
    lines.append(
        f"    Pumps: {' | '.join(pump_parts)}"
    )
    lines.append(
        f"    Circ: {'ON' if decoded['circ_pump'] else 'OFF'} | "
        f"Blower: {'ON' if decoded['blower'] else 'OFF'} | "
        f"Mister: {'ON' if decoded['mister'] else 'OFF'}"
    )
    lines.append(
        f"    Lights: 1={'ON' if decoded['light1'] else 'OFF'} "
        f"2={'ON' if decoded['light2'] else 'OFF'}"
    )
    return "\n".join(lines)


def format_filter_detail(decoded: dict) -> str:
    """Format decoded filter cycles as a detail string."""
    if "error" in decoded:
        return f"    {decoded['error']}"
    f1 = decoded["filter1"]
    f2 = decoded["filter2"]
    return (
        f"    Filter 1: {f1['start']} dur={f1['duration']} enabled={f1['enabled']}\n"
        f"    Filter 2: {f2['start']} dur={f2['duration']} enabled={f2['enabled']}"
    )


def format_info_detail(decoded: dict) -> str:
    """Format decoded information response as a detail string."""
    if "error" in decoded:
        return f"    {decoded['error']}"
    return (
        f"    Model: {decoded['system_model']} | "
        f"SW: {decoded['software_id']} | "
        f"Config: {decoded['config_signature']}\n"
        f"    Heater: {decoded['heater_voltage']} {decoded['heater_type']} | "
        f"DIP: {decoded['dip_switches']}"
    )


def format_fault_detail(decoded: dict) -> str:
    """Format decoded fault log entry as a detail string."""
    if "error" in decoded:
        return f"    {decoded['error']}"
    return (
        f"    #{decoded['entry_number']}/{decoded['fault_count']} "
        f"{decoded['fault_name']} | "
        f"{decoded['days_ago']}d ago @ {decoded['time']} | "
        f"Set={decoded['set_temperature']}° "
        f"A={decoded['sensor_a_temp']}° B={decoded['sensor_b_temp']}°"
    )


# ── Main Decoder ──────────────────────────────────────────────────────────

def decode_and_display(msg_type: tuple, payload: bytes, raw_bytes: bytes,
                       crc_ok: bool, timestamp: str, use_color: bool) -> dict:
    """
    Decode a frame and print the result. Returns a dict for session logging.
    """
    msg_name = identify_message(msg_type, payload)
    msg_hex = msg_type_hex(msg_type)
    total_len = len(raw_bytes)

    # Choose color based on status
    if use_color:
        if not crc_ok:
            color = C.RED
        elif "Unknown" in msg_name:
            color = C.YELLOW
        else:
            color = C.GREEN
    else:
        color = ""

    reset = C.RESET if use_color else ""

    # Header line
    crc_str = f"{color}CRC OK{reset}" if crc_ok else f"{C.RED if use_color else ''}CRC FAIL{reset}"
    print(f"  [{timestamp}] {color}{msg_name}{reset} ({msg_hex}) - {total_len} bytes - {crc_str}")

    # Decode detail based on message type
    decoded = None
    detail_lines = ""

    if msg_type == (0xFF, 0xAF):
        # Status Update
        decoded = decode_status_update(payload)
        if "error" not in decoded:
            detail_lines = format_status_detail(decoded)
        else:
            detail_lines = f"    {decoded['error']}"

    elif msg_type == (0x0A, 0xBF) and len(payload) > 0:
        sub = payload[0]
        if sub == 0x23:
            decoded = decode_filter_cycles(payload)
            detail_lines = format_filter_detail(decoded)
        elif sub == 0x24:
            decoded = decode_information(payload)
            detail_lines = format_info_detail(decoded)
        elif sub == 0x28:
            decoded = decode_fault_log(payload)
            detail_lines = format_fault_detail(decoded)
        elif sub == 0x11 and len(payload) >= 2:
            decoded = {"item": f"0x{payload[1]:02X}"}
            detail_lines = f"    Toggle item: 0x{payload[1]:02X}"
        elif sub == 0x20 and len(payload) >= 2:
            decoded = {"temperature": payload[1]}
            detail_lines = f"    Set temperature: {payload[1]}"
        elif sub == 0x2E or sub == 0x94:
            detail_lines = f"    Configuration data ({len(payload) - 1} bytes)"
        else:
            # Show raw payload hex
            detail_lines = f"    Payload: {' '.join(f'{b:02X}' for b in payload)}"

    elif msg_type == (0xFE, 0xBF):
        decoded = decode_registration(payload)
        if "assigned_id" in decoded:
            detail_lines = f"    Assigned ID: 0x{decoded['assigned_id']:02X} ({decoded['assigned_id']})"
        elif "sub_type" in decoded:
            detail_lines = f"    Sub-type: {decoded['sub_type']}"

    elif msg_type == (0x10, 0xBF):
        detail_lines = "    Bus free — safe to send"

    else:
        # Unknown message type — show raw payload
        if payload:
            detail_lines = f"    Payload: {' '.join(f'{b:02X}' for b in payload)}"
        else:
            detail_lines = "    (no payload)"

    if detail_lines:
        print(detail_lines)

    # Hex dump (dimmed)
    if use_color:
        print(f"{C.DIM}", end="")
    print(hex_dump(raw_bytes))
    if use_color:
        print(C.RESET, end="")

    print()  # blank line separator

    # Build session log entry
    entry = {
        "timestamp": timestamp,
        "message_type": msg_hex,
        "message_name": msg_name,
        "crc_ok": crc_ok,
        "length": total_len,
        "raw_hex": raw_bytes.hex().upper(),
    }
    if decoded and "error" not in decoded:
        entry["decoded"] = decoded

    return entry


# ── MQTT Message Handler ──────────────────────────────────────────────────

def handle_mqtt_message(topic: str, payload_bytes: bytes, session_log: list,
                        use_color: bool) -> None:
    """Process a single MQTT message from the sniff topic."""
    # Extract device ID from topic: launa/<device_id>/sniff
    parts = topic.split("/")
    device_id = parts[1] if len(parts) >= 3 else "?"

    timestamp = datetime.now().strftime("%H:%M:%S")

    if use_color:
        print(f"{C.CYAN}{C.BOLD}[{timestamp}] Device: {device_id}{C.RESET}")

    # Try JSON first
    try:
        json_msg = json.loads(payload_bytes.decode("utf-8", errors="replace"))
    except (json.JSONDecodeError, UnicodeDecodeError):
        json_msg = None

    if json_msg and isinstance(json_msg, dict):
        # JSON format from ESP32 sniffer: {"raw":"...", "type":"...", "len":N, "crc_ok":bool}
        raw_hex = json_msg.get("raw", json_msg.get("raw_hex", ""))
        if raw_hex:
            raw_bytes = hex_to_bytes(raw_hex)
        else:
            # No raw data — try message_type field
            mt = json_msg.get("type", json_msg.get("message_type", ""))
            if mt:
                raw_bytes = hex_to_bytes(mt)
            else:
                raw_bytes = b""

        # Override timestamp from JSON if present
        ts = json_msg.get("ts", json_msg.get("timestamp"))
        if ts:
            timestamp = ts

        # Get CRC status from JSON if available
        json_crc = json_msg.get("crc_ok")

        if raw_bytes:
            # Try to parse as a Balboa frame
            parsed = parse_raw_frame(raw_bytes)
            if parsed:
                crc_ok = json_crc if json_crc is not None else parsed["crc_ok"]
                entry = decode_and_display(
                    parsed["message_type"],
                    parsed["payload"],
                    raw_bytes,
                    crc_ok,
                    timestamp,
                    use_color,
                )
                entry["device_id"] = device_id
                session_log.append(entry)
            else:
                # Could not parse as frame — show raw
                crc_ok = json_crc if json_crc is not None else False
                if use_color:
                    color = C.YELLOW
                    reset = C.RESET
                else:
                    color = reset = ""

                print(f"  [{timestamp}] {color}Unparseable frame{reset} - {len(raw_bytes)} bytes - "
                      f"{'CRC OK' if crc_ok else 'CRC FAIL'}")
                print(hex_dump(raw_bytes))
                print()
                session_log.append({
                    "timestamp": timestamp,
                    "device_id": device_id,
                    "message_type": "unknown",
                    "message_name": "Unparseable",
                    "crc_ok": crc_ok,
                    "length": len(raw_bytes),
                    "raw_hex": raw_bytes.hex().upper(),
                })
        else:
            # No raw bytes at all
            msg_type_str = json_msg.get("type", json_msg.get("message_type", "??"))
            msg_len = json_msg.get("len", json_msg.get("length", 0))
            crc_ok = json_crc if json_crc is not None else False

            if use_color:
                color = C.YELLOW
                reset = C.RESET
            else:
                color = reset = ""

            print(f"  [{timestamp}] {color}JSON frame (no raw data){reset} - "
                  f"type={msg_type_str} len={msg_len} crc={'OK' if crc_ok else 'FAIL'}")
            print()
            session_log.append({
                "timestamp": timestamp,
                "device_id": device_id,
                "message_type": msg_type_str,
                "message_name": "JSON (no raw)",
                "crc_ok": crc_ok if crc_ok is not None else False,
                "length": msg_len,
                "raw_hex": "",
            })
    else:
        # Raw payload (not JSON) — try as hex string or raw bytes
        text = payload_bytes.decode("utf-8", errors="replace").strip()
        raw_bytes = hex_to_bytes(text) if all(c in "0123456789abcdefABCDEF \t\n\r" for c in text) else payload_bytes

        if raw_bytes:
            parsed = parse_raw_frame(raw_bytes)
            if parsed:
                entry = decode_and_display(
                    parsed["message_type"],
                    parsed["payload"],
                    raw_bytes,
                    parsed["crc_ok"],
                    timestamp,
                    use_color,
                )
                entry["device_id"] = device_id
                session_log.append(entry)
            else:
                if use_color:
                    color = C.YELLOW
                    reset = C.RESET
                else:
                    color = reset = ""

                print(f"  [{timestamp}] {color}Raw data (not a frame){reset} - {len(raw_bytes)} bytes")
                print(hex_dump(raw_bytes))
                print()
                session_log.append({
                    "timestamp": timestamp,
                    "device_id": device_id,
                    "message_type": "raw",
                    "message_name": "Raw data",
                    "crc_ok": False,
                    "length": len(raw_bytes),
                    "raw_hex": raw_bytes.hex().upper(),
                })
        else:
            if use_color:
                print(f"  {C.DIM}[{timestamp}] Empty payload{C.RESET}")
            else:
                print(f"  [{timestamp}] Empty payload")
            print()


# ── MQTT Client Setup ─────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Launa Sniffer Decoder — Real-time Balboa RS-485 frame decoder",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Subscribes to launa/+/sniff and decodes Balboa protocol frames.\n"
            "Requires: pip install paho-mqtt\n"
            "\n"
            "Examples:\n"
            "  python scripts/sniff-decode.py\n"
            "  python scripts/sniff-decode.py --host 192.168.1.100 --port 1883\n"
            "  python scripts/sniff-decode.py --save session.json\n"
        ),
    )
    parser.add_argument("--host", default="localhost", help="MQTT broker host (default: localhost)")
    parser.add_argument("--port", type=int, default=1883, help="MQTT broker port (default: 1883)")
    parser.add_argument("--device-id", default=None, help="Filter to specific device ID (default: all devices)")
    parser.add_argument("--save", default=None, metavar="FILE.json", help="Save all decoded frames to JSON file")
    parser.add_argument("--no-color", action="store_true", help="Disable ANSI color output")
    args = parser.parse_args()

    use_color = C.support() and not args.no_color

    session_log: list = []

    # Build topic
    if args.device_id:
        topic = f"launa/{args.device_id}/sniff"
    else:
        topic = "launa/+/sniff"

    # ── Signal handler for clean shutdown ──
    running = True

    def signal_handler(sig, frame):
        nonlocal running
        running = False
        if use_color:
            print(f"\n{C.YELLOW}Stopping...{C.RESET}")
        else:
            print("\nStopping...")

    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)

    # ── Connect to MQTT ──
    def on_connect(client, userdata, flags, rc, properties=None):
        if rc == 0:
            client.subscribe(topic)
            if use_color:
                print(f"  {C.GREEN}Connected to MQTT broker at {args.host}:{args.port}{C.RESET}")
            else:
                print(f"  Connected to MQTT broker at {args.host}:{args.port}")
            print(f"  Subscribed to: {topic}")
            print(f"  Press Ctrl+C to stop")
            print()
        else:
            rc_names = {
                1: "Incorrect protocol version",
                2: "Invalid client identifier",
                3: "Server unavailable",
                4: "Bad username or password",
                5: "Not authorized",
            }
            err = rc_names.get(rc, f"Unknown error (rc={rc})")
            print(f"  MQTT connection failed: {err}", file=sys.stderr)
            sys.exit(1)

    def on_message(client, userdata, msg):
        handle_mqtt_message(msg.topic, msg.payload, session_log, use_color)

    def on_disconnect(client, userdata, flags, rc, properties=None):
        if rc != 0 and running:
            if use_color:
                print(f"  {C.RED}Unexpected MQTT disconnect (rc={rc}), reconnecting...{C.RESET}")
            else:
                print(f"  Unexpected MQTT disconnect (rc={rc}), reconnecting...")

    client_id = f"launa-sniff-decode-{int(time.time())}"
    client = mqtt.Client(
        callback_api_version=mqtt.CallbackAPIVersion.VERSION2,
        client_id=client_id,
    )
    client.on_connect = on_connect
    client.on_message = on_message
    client.on_disconnect = on_disconnect

    if use_color:
        print(f"  {C.BOLD}Launa Sniffer Decoder{C.RESET}")
        print(f"  {'=' * 40}")
    else:
        print("  Launa Sniffer Decoder")
    print(f"  Connecting to {args.host}:{args.port}...")

    try:
        client.connect(args.host, args.port, keepalive=60)
    except Exception as e:
        print(f"  Failed to connect: {e}", file=sys.stderr)
        sys.exit(1)

    # Start network loop in background thread
    client.loop_start()

    try:
        while running:
            time.sleep(0.1)
    except KeyboardInterrupt:
        pass
    finally:
        client.loop_stop()
        client.disconnect()

        # Save session log
        if args.save and session_log:
            try:
                with open(args.save, "w") as f:
                    json.dump(session_log, f, indent=2)
                print(f"\n  Session saved to {args.save} ({len(session_log)} frames)")
            except OSError as e:
                print(f"\n  Failed to save session: {e}", file=sys.stderr)
        elif args.save:
            print(f"\n  No frames captured to save.")

        print(f"  Decoded {len(session_log)} frames total.")
        print("  Done.")


if __name__ == "__main__":
    main()
