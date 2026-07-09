use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const VENDOR_IDS: [u16; 2] = [0x2717, 0x1A86];
pub const FRAME_SIZE: usize = 32;
pub const HEAD: u8 = 0xA5;

pub const CMD_HELLO: u8 = 0x00;
pub const CMD_GET_BATTERY_INFO: u8 = 0x01;
pub const CMD_GET_CELL_STATUS: u8 = 0x02;
pub const CMD_GET_HISTORY: u8 = 0x03;
pub const CMD_GET_BATTERY_ID: u8 = 0x04;
pub const CMD_DISCONNECT: u8 = 0x05;
pub const CMD_ENABLE_QI2: u8 = 0x06;
pub const CMD_GET_QI2_STATUS: u8 = 0x07;
pub const CMD_GET_CELL_TEMP_MODEL: u8 = 0x08;
pub const CMD_HEARTBEAT: u8 = 0x0A;

pub const RSP_HELLO: u8 = 0x10;
pub const RSP_BATTERY_INFO: u8 = 0x11;
pub const RSP_CELL_STATUS: u8 = 0x12;
pub const RSP_HISTORY: u8 = 0x13;
pub const RSP_BATTERY_ID: u8 = 0x14;
pub const RSP_ENABLE_QI2: u8 = 0x16;
pub const RSP_QI2_STATUS: u8 = 0x17;
pub const RSP_CELL_TEMP_MODEL: u8 = 0x18;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(3_000);

#[derive(Debug, Error)]
pub enum PowerBankError {
    #[error("frame payload is too long: {len} bytes, maximum is {max}")]
    PayloadTooLong { len: usize, max: usize },
    #[error("data length is too short: {0} bytes")]
    DataTooShort(usize),
    #[error("invalid frame head: expected 0xA5, got 0x{0:02X}")]
    InvalidHead(u8),
    #[error("payload is truncated: declared {declared} bytes, available {available}")]
    PayloadTruncated { declared: usize, available: usize },
    #[error("crc mismatch: expected 0x{expected:02X}, got 0x{received:02X}")]
    CrcMismatch { expected: u8, received: u8 },
    #[error("response timeout")]
    Timeout,
    #[error("unexpected response command: expected 0x{expected:02X}, got 0x{actual:02X}")]
    UnexpectedCommand { expected: u8, actual: u8 },
    #[error("{context} payload is too short: {actual}/{expected} bytes")]
    DecodeTooShort {
        context: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("invalid hex input: {0}")]
    InvalidHex(String),
    #[error("transport error: {0}")]
    Transport(String),
}

pub type Result<T> = std::result::Result<T, PowerBankError>;

#[async_trait(?Send)]
pub trait Transport {
    async fn write_frame(&mut self, frame: &[u8; FRAME_SIZE]) -> Result<()>;
    async fn read_frame(&mut self, timeout: Duration) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFrame {
    pub cmd: u8,
    pub payload: Vec<u8>,
    pub payload_len: usize,
    pub crc_ok: bool,
    pub crc_received: u8,
    pub crc_expected: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceModel {
    pub id: u16,
    pub name: &'static str,
    pub code: &'static str,
}

pub const MODEL_DB: &[DeviceModel] = &[
    DeviceModel {
        id: 1,
        name: "Xiaomi Integrated-Cable Power Bank 10000 67W",
        code: "PB1067MI",
    },
    DeviceModel {
        id: 2,
        name: "Xiaomi Integrated-Cable Power Bank 10000 Pocket Edition",
        code: "P15",
    },
    DeviceModel {
        id: 3,
        name: "Xiaomi Integrated-Cable Fast-Charge Power Bank 20000 45W",
        code: "PB2045MI",
    },
    DeviceModel {
        id: 4,
        name: "Xiaomi Integrated-Cable Power Bank 20000 22.5W",
        code: "PB2020",
    },
    DeviceModel {
        id: 5,
        name: "Xiaomi Integrated-Cable Power Bank 20000 67W",
        code: "PB2067MI",
    },
    DeviceModel {
        id: 6,
        name: "Xiaomi Power Bank Pro 25000 250W",
        code: "P25",
    },
    DeviceModel {
        id: 7,
        name: "Xiaomi Retractable-Cable Power Bank 10000 55W",
        code: "NPB1055R",
    },
    DeviceModel {
        id: 8,
        name: "Xiaomi 3-in-1 Power Bank 10000 67W",
        code: "AC1067",
    },
    DeviceModel {
        id: 9,
        name: "Xiaomi Jinshajiang Ultra-Thin Magnetic Power Bank 10000 45W",
        code: "WPB1025S",
    },
    DeviceModel {
        id: 10,
        name: "Xiaomi Jinshajiang Ultra-Thin Magnetic Power Bank 5000 27W",
        code: "WPB0525S",
    },
    DeviceModel {
        id: 11,
        name: "Xiaomi Magnetic Stand Power Bank 10000 7.5W 2026",
        code: "WPB1007ZX",
    },
    DeviceModel {
        id: 12,
        name: "Xiaomi Magnetic Integrated-Cable Power Bank 10000 45W",
        code: "WPB1025",
    },
    DeviceModel {
        id: 13,
        name: "Xiaomi Integrated-Cable Power Bank 10000 Pocket Edition 2026",
        code: "P15",
    },
    DeviceModel {
        id: 14,
        name: "Xiaomi Integrated-Cable Power Bank 20000 22.5W 2026",
        code: "PB2020",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloInfo {
    pub device_name: String,
    pub device_model: String,
    pub model_id: u16,
    pub serial_number: String,
    pub charging_status: ChargingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargingStatus {
    Idle,
    Charging,
    Discharging,
    Unknown(u8),
}

impl fmt::Display for ChargingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => f.write_str("Idle"),
            Self::Charging => f.write_str("Charging"),
            Self::Discharging => f.write_str("Discharging"),
            Self::Unknown(value) => write!(f, "Unknown({value})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryInfo {
    pub success: bool,
    pub status_code: u8,
    pub activated: Option<bool>,
    pub cycle_count: u16,
    pub health: u16,
    pub charge_state: ChargeState,
    pub fault_type: u8,
    pub error_value: u16,
    pub history_errors: u16,
    pub cell_count: u8,
    pub level_pct: u8,
    pub temperature_c: i8,
    pub voltage_mv: u16,
    pub current_ma: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeState {
    Discharging,
    Charging,
    Idle,
    Unknown(u8),
}

impl fmt::Display for ChargeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discharging => f.write_str("Discharging"),
            Self::Charging => f.write_str("Charging"),
            Self::Idle => f.write_str("Idle"),
            Self::Unknown(value) => write!(f, "Unknown({value})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qi2Status {
    pub success: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qi2SetResult {
    pub success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellStatus {
    pub success: bool,
    pub status_code: u8,
    pub cells: Vec<CellReading>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellReading {
    pub index: u8,
    pub temperature_c: Option<i8>,
    pub voltage_mv: u16,
    pub current_ma: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryIdInfo {
    pub success: bool,
    pub status_code: u8,
    pub cell_index: u8,
    pub battery_id: String,
    pub enterprise_code: Option<String>,
    pub production_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellTempModel {
    pub success: bool,
    pub high_temp: i16,
    pub low_temp: i16,
    pub battery_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub hello: HelloInfo,
    pub battery: Option<BatteryInfo>,
    pub qi2: Option<Qi2Status>,
    pub cells: Option<CellStatus>,
    pub battery_ids: Vec<BatteryIdInfo>,
    pub cell_temp_model: Option<CellTempModel>,
}

pub struct PowerBank<T> {
    transport: T,
    debug: bool,
}

impl<T> PowerBank<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            debug: false,
        }
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T: Transport> PowerBank<T> {
    pub async fn send_and_wait(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        expected_cmd: Option<u8>,
        timeout: Duration,
    ) -> Result<ParsedFrame> {
        if self.debug {
            eprintln!("[HID] TX {}", hex_upper(frame));
        }

        self.transport.write_frame(frame).await?;
        let deadline = Instant::now() + timeout;

        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(PowerBankError::Timeout);
            }

            let remaining = deadline.saturating_duration_since(now);
            let raw = match self.transport.read_frame(remaining).await {
                Ok(raw) => raw,
                Err(PowerBankError::Timeout) => return Err(PowerBankError::Timeout),
                Err(err) => return Err(err),
            };

            if self.debug {
                eprintln!("[HID] RX {}", hex_upper(&raw));
            }

            let parsed = match parse_response(&raw) {
                Ok(parsed) => parsed,
                Err(err) => {
                    if self.debug {
                        eprintln!("[HID] parse ignored: {err}");
                    }
                    continue;
                }
            };

            if let Some(expected) = expected_cmd
                && parsed.cmd != expected
            {
                if self.debug {
                    eprintln!(
                        "[HID] ignored cmd 0x{:02X}; expected 0x{expected:02X}",
                        parsed.cmd
                    );
                }
                continue;
            }

            return Ok(parsed);
        }
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        let frame = build_command_frame(CMD_DISCONNECT, &[])?;
        self.transport.write_frame(&frame).await
    }

    pub async fn handshake(&mut self) -> Result<HelloInfo> {
        let frame = build_hello_frame();
        let rsp = self
            .send_and_wait(&frame, Some(RSP_HELLO), DEFAULT_TIMEOUT)
            .await?;
        ensure_crc(&rsp)?;
        decode_hello_response(&rsp.payload)
    }

    pub async fn battery_info(&mut self) -> Result<BatteryInfo> {
        let frame = build_command_frame(CMD_GET_BATTERY_INFO, &[])?;
        let rsp = self
            .send_and_wait(&frame, Some(RSP_BATTERY_INFO), DEFAULT_TIMEOUT)
            .await?;
        ensure_crc(&rsp)?;
        decode_battery_payload(&rsp.payload)
    }

    pub async fn qi2_status(&mut self) -> Result<Qi2Status> {
        let frame = build_command_frame(CMD_GET_QI2_STATUS, &[])?;
        let rsp = self
            .send_and_wait(&frame, Some(RSP_QI2_STATUS), DEFAULT_TIMEOUT)
            .await?;
        ensure_crc(&rsp)?;
        decode_qi2_status(&rsp.payload)
    }

    pub async fn set_qi2(&mut self, enable: bool) -> Result<Qi2SetResult> {
        let payload = [u8::from(enable)];
        let frame = build_command_frame(CMD_ENABLE_QI2, &payload)?;
        let rsp = self
            .send_and_wait(&frame, Some(RSP_ENABLE_QI2), Duration::from_millis(5_000))
            .await?;
        ensure_crc(&rsp)?;
        decode_qi2_enable_response(&rsp.payload)
    }

    pub async fn cell_status(&mut self, cell_count: u8) -> Result<CellStatus> {
        let payload;
        let payload_ref = if cell_count > 0 {
            payload = [1, cell_count];
            payload.as_slice()
        } else {
            &[]
        };
        let frame = build_command_frame(CMD_GET_CELL_STATUS, payload_ref)?;
        let rsp = self
            .send_and_wait(&frame, Some(RSP_CELL_STATUS), DEFAULT_TIMEOUT)
            .await?;
        ensure_crc(&rsp)?;
        decode_cell_status(&rsp.payload)
    }

    pub async fn battery_id(&mut self, cell_index: u8) -> Result<BatteryIdInfo> {
        let payload = [cell_index];
        let frame = build_command_frame(CMD_GET_BATTERY_ID, &payload)?;
        let rsp = self
            .send_and_wait(&frame, Some(RSP_BATTERY_ID), Duration::from_millis(1_000))
            .await?;
        ensure_crc(&rsp)?;
        decode_battery_id_response(&rsp.payload)
    }

    pub async fn cell_temp_model(&mut self) -> Result<CellTempModel> {
        let frame = build_command_frame(CMD_GET_CELL_TEMP_MODEL, &[])?;
        let rsp = self
            .send_and_wait(&frame, Some(RSP_CELL_TEMP_MODEL), DEFAULT_TIMEOUT)
            .await?;
        ensure_crc(&rsp)?;
        decode_cell_temp_model(&rsp.payload)
    }

    pub async fn raw(&mut self, input: &str, timeout: Duration) -> Result<ParsedFrame> {
        let bytes = parse_hex(input)?;
        let mut frame = [0u8; FRAME_SIZE];
        let copy_len = bytes.len().min(FRAME_SIZE);
        frame[..copy_len].copy_from_slice(&bytes[..copy_len]);
        self.send_and_wait(&frame, None, timeout).await
    }

    pub async fn snapshot(&mut self) -> Result<DeviceSnapshot> {
        let hello = self.handshake().await?;
        let battery = self.battery_info().await.ok();
        let qi2 = self.qi2_status().await.ok();
        let cell_count = battery.as_ref().map_or(0, |info| info.cell_count);
        let cells = self.cell_status(cell_count).await.ok();
        let mut battery_ids = Vec::new();
        for index in 1..=cell_count {
            if let Ok(id) = self.battery_id(index).await {
                battery_ids.push(id);
            }
        }
        let cell_temp_model = self.cell_temp_model().await.ok();

        Ok(DeviceSnapshot {
            hello,
            battery,
            qi2,
            cells,
            battery_ids,
            cell_temp_model,
        })
    }
}

pub fn crc8(data: &[u8]) -> u8 {
    let mut t = 0u8;
    for byte in data {
        t ^= *byte;
        for _ in 0..8 {
            t = if t & 0x80 != 0 {
                (t << 1) ^ 0x07
            } else {
                t << 1
            };
        }
    }
    t
}

pub fn build_command_frame(cmd: u8, payload: &[u8]) -> Result<[u8; FRAME_SIZE]> {
    let max_payload = FRAME_SIZE - 4;
    if payload.len() > max_payload {
        return Err(PowerBankError::PayloadTooLong {
            len: payload.len(),
            max: max_payload,
        });
    }

    let mut frame = [0u8; FRAME_SIZE];
    frame[0] = HEAD;
    frame[1] = cmd;
    frame[2] = payload.len() as u8;
    frame[3..3 + payload.len()].copy_from_slice(payload);
    frame[3 + payload.len()] = crc8(&frame[..3 + payload.len()]);
    Ok(frame)
}

pub fn build_hello_frame() -> [u8; FRAME_SIZE] {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_add(8 * 60 * 60) as u32;
    build_hello_frame_at(timestamp)
}

pub fn build_hello_frame_at(timestamp: u32) -> [u8; FRAME_SIZE] {
    let mut frame = [0u8; FRAME_SIZE];
    frame[0] = HEAD;
    frame[1] = CMD_HELLO;
    frame[2] = 13;
    frame[3..7].copy_from_slice(&timestamp.to_le_bytes());
    frame[7..16].copy_from_slice(b"xiaomi-pb");
    frame[16] = crc8(&frame[..16]);
    frame
}

pub fn parse_response(data: &[u8]) -> Result<ParsedFrame> {
    if data.len() < 4 {
        return Err(PowerBankError::DataTooShort(data.len()));
    }

    if data[0] != HEAD {
        return Err(PowerBankError::InvalidHead(data[0]));
    }

    let cmd = data[1];
    let payload_len = data[2] as usize;
    let crc_index = 3 + payload_len;
    if crc_index >= data.len() {
        return Err(PowerBankError::PayloadTruncated {
            declared: payload_len,
            available: data.len().saturating_sub(4),
        });
    }

    let payload = data[3..crc_index].to_vec();
    let crc_received = data[crc_index];
    let crc_expected = crc8(&data[..crc_index]);

    Ok(ParsedFrame {
        cmd,
        payload,
        payload_len,
        crc_ok: crc_received == crc_expected,
        crc_received,
        crc_expected,
    })
}

pub fn decode_hello_response(payload: &[u8]) -> Result<HelloInfo> {
    require_len("hello", payload, 23)?;
    let model_id = u16::from_le_bytes([payload[0], payload[1]]);
    let serial_number = ascii_until_nul(&payload[2..22]);
    let charging_status = match payload[22] {
        0 => ChargingStatus::Idle,
        1 => ChargingStatus::Charging,
        2 => ChargingStatus::Discharging,
        value => ChargingStatus::Unknown(value),
    };
    let model = MODEL_DB.iter().find(|model| model.id == model_id);

    Ok(HelloInfo {
        device_name: model.map_or("--", |model| model.name).to_owned(),
        device_model: model.map_or("--", |model| model.code).to_owned(),
        model_id,
        serial_number,
        charging_status,
    })
}

pub fn decode_battery_payload(payload: &[u8]) -> Result<BatteryInfo> {
    require_len("battery", payload, 18)?;
    let status_code = payload[0];
    let activated = match payload[1] {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    let cycle_count = u16::from_le_bytes([payload[2], payload[3]]);
    let health = u16::from_le_bytes([payload[4], payload[5]]);
    let charge_error = payload[6];
    let charge_state = match (charge_error >> 6) & 3 {
        0 => ChargeState::Discharging,
        1 => ChargeState::Charging,
        2 => ChargeState::Idle,
        value => ChargeState::Unknown(value),
    };
    let fault_type = charge_error & 0x3F;
    let error_value = u16::from_le_bytes([payload[7], payload[8]]);
    let history_errors = u16::from_le_bytes([payload[9], payload[10]]);
    let cell_config = payload[11];
    let series = (cell_config >> 4) & 0x0F;
    let parallel = cell_config & 0x0F;
    let normalized_series = if series == 0 && cell_config != 0 {
        1
    } else {
        series
    };
    let normalized_parallel = if parallel == 0 && cell_config != 0 {
        1
    } else {
        parallel
    };
    let cell_count = normalized_series.saturating_mul(normalized_parallel);
    let level_pct = payload[12];
    let temperature_c = payload[13] as i8;
    let voltage_mv = u16::from_le_bytes([payload[14], payload[15]]);
    let current_ma = i16::from_le_bytes([payload[16], payload[17]]);

    Ok(BatteryInfo {
        success: status_code == 0,
        status_code,
        activated,
        cycle_count,
        health,
        charge_state,
        fault_type,
        error_value,
        history_errors,
        cell_count,
        level_pct,
        temperature_c,
        voltage_mv,
        current_ma,
    })
}

pub fn decode_qi2_status(payload: &[u8]) -> Result<Qi2Status> {
    require_len("qi2 status", payload, 2)?;
    Ok(Qi2Status {
        success: payload[0] == 0,
        enabled: payload[1] == 1,
    })
}

pub fn decode_qi2_enable_response(payload: &[u8]) -> Result<Qi2SetResult> {
    require_len("qi2 set", payload, 1)?;
    Ok(Qi2SetResult {
        success: payload[0] == 0,
    })
}

pub fn decode_cell_status(payload: &[u8]) -> Result<CellStatus> {
    require_len("cell status", payload, 1)?;
    let status_code = payload[0];
    if status_code != 0 {
        return Ok(CellStatus {
            success: false,
            status_code,
            cells: Vec::new(),
        });
    }

    let mut cells = Vec::new();
    for (idx, chunk) in payload[1..].chunks_exact(5).enumerate() {
        let raw_temp = chunk[0] as i8;
        let temperature_c = if raw_temp == -127 {
            None
        } else {
            Some(raw_temp)
        };
        let voltage_mv = u16::from_le_bytes([chunk[1], chunk[2]]);
        let current_ma = i16::from_le_bytes([chunk[3], chunk[4]]);
        cells.push(CellReading {
            index: (idx + 1) as u8,
            temperature_c,
            voltage_mv,
            current_ma,
        });
    }

    Ok(CellStatus {
        success: true,
        status_code,
        cells,
    })
}

pub fn decode_battery_id_response(payload: &[u8]) -> Result<BatteryIdInfo> {
    require_len("battery id", payload, 2)?;
    let status_code = payload[0];
    let cell_index = payload[1];
    let battery_id = if payload.len() > 2 {
        ascii_until_nul(&payload[2..])
    } else {
        String::new()
    };
    let (enterprise_code, production_date) = parse_battery_id(&battery_id);

    Ok(BatteryIdInfo {
        success: status_code == 0,
        status_code,
        cell_index,
        battery_id,
        enterprise_code,
        production_date,
    })
}

pub fn decode_cell_temp_model(payload: &[u8]) -> Result<CellTempModel> {
    require_len("cell temperature model", payload, 5)?;
    let status_code = payload[0];
    let high_temp = i16::from_le_bytes([payload[1], payload[2]]);
    let low_temp = i16::from_le_bytes([payload[3], payload[4]]);
    let battery_model = if payload.len() > 5 {
        ascii_until_nul(&payload[5..])
    } else {
        String::new()
    };

    Ok(CellTempModel {
        success: status_code == 0,
        high_temp,
        low_temp,
        battery_model,
    })
}

pub fn parse_hex(input: &str) -> Result<Vec<u8>> {
    let cleaned = input
        .split_whitespace()
        .map(|part| {
            part.strip_prefix("0x")
                .or_else(|| part.strip_prefix("0X"))
                .unwrap_or(part)
        })
        .collect::<String>();

    if cleaned.len() % 2 != 0 {
        return Err(PowerBankError::InvalidHex(input.to_owned()));
    }

    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    let chars: Vec<char> = cleaned.chars().collect();
    for chunk in chars.chunks(2) {
        let pair = chunk.iter().collect::<String>();
        let byte = u8::from_str_radix(&pair, 16)
            .map_err(|_| PowerBankError::InvalidHex(input.to_owned()))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

pub fn hex_upper(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join("")
}

pub fn ensure_crc(frame: &ParsedFrame) -> Result<()> {
    if frame.crc_ok {
        Ok(())
    } else {
        Err(PowerBankError::CrcMismatch {
            expected: frame.crc_expected,
            received: frame.crc_received,
        })
    }
}

fn require_len(context: &'static str, data: &[u8], expected: usize) -> Result<()> {
    if data.len() < expected {
        Err(PowerBankError::DecodeTooShort {
            context,
            expected,
            actual: data.len(),
        })
    } else {
        Ok(())
    }
}

fn ascii_until_nul(data: &[u8]) -> String {
    let end = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).to_string()
}

fn parse_battery_id(raw_id: &str) -> (Option<String>, Option<String>) {
    let stripped = raw_id.replace(' ', "");
    if stripped.len() < 9 {
        return (None, None);
    }

    let enterprise = stripped[0..4].to_owned();
    let year_code = stripped.as_bytes()[6] as char;
    let month_code = stripped.as_bytes()[7] as char;
    let day_code = stripped.as_bytes()[8] as char;

    let year = match year_code {
        'F' => Some(2025),
        'G' => Some(2026),
        'H' => Some(2027),
        'J' => Some(2028),
        'K' => Some(2029),
        'L' => Some(2030),
        'M' => Some(2031),
        'N' => Some(2032),
        'P' => Some(2033),
        'R' => Some(2034),
        'S' => Some(2035),
        'T' => Some(2036),
        'V' => Some(2037),
        _ => None,
    };
    let month = match month_code {
        '1'..='9' => month_code.to_digit(10),
        'A' => Some(10),
        'B' => Some(11),
        'C' => Some(12),
        _ => None,
    };
    let day = match day_code {
        '1'..='9' => day_code.to_digit(10),
        'A' => Some(10),
        'B' => Some(11),
        'C' => Some(12),
        'D' => Some(13),
        'E' => Some(14),
        'F' => Some(15),
        'G' => Some(16),
        'H' => Some(17),
        'J' => Some(18),
        'K' => Some(19),
        'L' => Some(20),
        'M' => Some(21),
        'N' => Some(22),
        'P' => Some(23),
        'R' => Some(24),
        'S' => Some(25),
        'T' => Some(26),
        'V' => Some(27),
        'W' => Some(28),
        'X' => Some(29),
        'Y' => Some(30),
        '0' => Some(31),
        _ => None,
    };

    let production_date = match (year, month, day) {
        (Some(year), Some(month), Some(day)) => NaiveDate::from_ymd_opt(year, month, day)
            .map(|date| format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())),
        _ => None,
    };

    (Some(enterprise), production_date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::collections::VecDeque;

    #[test]
    fn crc8_matches_known_command() {
        assert_eq!(crc8(&[0xA5, 0x06, 0x01, 0x00]), 0xD9);
        assert_eq!(crc8(&[0xA5, 0x06, 0x01, 0x01]), 0xDE);
    }

    #[test]
    fn builds_command_frame() {
        let frame = build_command_frame(CMD_ENABLE_QI2, &[0]).unwrap();
        assert_eq!(&frame[..5], &[0xA5, 0x06, 0x01, 0x00, 0xD9]);
        assert_eq!(frame.len(), FRAME_SIZE);
        assert!(frame[5..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn builds_hello_frame() {
        let frame = build_hello_frame_at(0x11223344);
        assert_eq!(frame[0], HEAD);
        assert_eq!(frame[1], CMD_HELLO);
        assert_eq!(frame[2], 13);
        assert_eq!(&frame[3..7], &0x11223344u32.to_le_bytes());
        assert_eq!(&frame[7..16], b"xiaomi-pb");
        assert_eq!(frame[16], crc8(&frame[..16]));
    }

    #[test]
    fn parses_response() {
        let mut frame = build_command_frame(RSP_QI2_STATUS, &[0, 1]).unwrap();
        frame[31] = 0xAA;
        let parsed = parse_response(&frame).unwrap();
        assert_eq!(parsed.cmd, RSP_QI2_STATUS);
        assert_eq!(parsed.payload, vec![0, 1]);
        assert!(parsed.crc_ok);
    }

    #[test]
    fn detects_crc_mismatch() {
        let mut frame = build_command_frame(RSP_QI2_STATUS, &[0, 1]).unwrap();
        frame[5] ^= 0xFF;
        let parsed = parse_response(&frame).unwrap();
        assert!(!parsed.crc_ok);
        assert!(matches!(
            ensure_crc(&parsed),
            Err(PowerBankError::CrcMismatch { .. })
        ));
    }

    #[test]
    fn decodes_battery_info() {
        let payload = [
            0, 1, 0x2A, 0x00, 98, 0, 0x40, 0x34, 0x12, 0x78, 0x56, 0x21, 88, 25, 0x10, 0x27, 0x18,
            0xFC,
        ];
        let info = decode_battery_payload(&payload).unwrap();
        assert!(info.success);
        assert_eq!(info.activated, Some(true));
        assert_eq!(info.cycle_count, 42);
        assert_eq!(info.health, 98);
        assert_eq!(info.charge_state, ChargeState::Charging);
        assert_eq!(info.cell_count, 2);
        assert_eq!(info.level_pct, 88);
        assert_eq!(info.temperature_c, 25);
        assert_eq!(info.voltage_mv, 10_000);
        assert_eq!(info.current_ma, -1000);
    }

    #[test]
    fn decodes_cell_status_and_absent_temp() {
        let payload = [0, 25, 0xA0, 0x0F, 0, 0, 0x81, 0xB0, 0x0F, 0xFF, 0xFF];
        let status = decode_cell_status(&payload).unwrap();
        assert_eq!(status.cells.len(), 2);
        assert_eq!(status.cells[0].temperature_c, Some(25));
        assert_eq!(status.cells[1].temperature_c, None);
        assert_eq!(status.cells[1].current_ma, -1);
    }

    #[test]
    fn decodes_battery_id_date() {
        let mut payload = vec![0, 2];
        payload.extend_from_slice(b"ATLNWSG3156789\0");
        let id = decode_battery_id_response(&payload).unwrap();
        assert_eq!(id.enterprise_code.as_deref(), Some("ATLN"));
        assert_eq!(id.production_date.as_deref(), Some("2026-03-01"));
    }

    #[test]
    fn parses_hex_with_prefixes_and_spaces() {
        assert_eq!(
            parse_hex("0xA5 06 01 00 C8").unwrap(),
            vec![0xA5, 6, 1, 0, 0xC8]
        );
        assert!(parse_hex("A50").is_err());
    }

    #[derive(Debug)]
    struct FakeTransport {
        responses: VecDeque<Vec<u8>>,
        writes: Vec<[u8; FRAME_SIZE]>,
    }

    #[async_trait(?Send)]
    impl Transport for FakeTransport {
        async fn write_frame(&mut self, frame: &[u8; FRAME_SIZE]) -> Result<()> {
            self.writes.push(*frame);
            Ok(())
        }

        async fn read_frame(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
            self.responses.pop_front().ok_or(PowerBankError::Timeout)
        }
    }

    #[test]
    fn send_and_wait_ignores_unexpected_commands() {
        let unexpected = build_command_frame(RSP_QI2_STATUS, &[0, 1])
            .unwrap()
            .to_vec();
        let expected = build_command_frame(RSP_ENABLE_QI2, &[0]).unwrap().to_vec();
        let fake = FakeTransport {
            responses: VecDeque::from([unexpected, expected]),
            writes: Vec::new(),
        };
        let mut pb = PowerBank::new(fake);
        let request = build_command_frame(CMD_ENABLE_QI2, &[1]).unwrap();
        let parsed =
            block_on(pb.send_and_wait(&request, Some(RSP_ENABLE_QI2), Duration::from_millis(20)))
                .unwrap();
        assert_eq!(parsed.cmd, RSP_ENABLE_QI2);
        assert_eq!(pb.into_transport().writes.len(), 1);
    }
}
