use async_trait::async_trait;
use hidapi::{HidApi, HidDevice};
use powerbank_core::{FRAME_SIZE, PowerBankError, Result, Transport, VENDOR_IDS};
use std::ffi::CString;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct NativeDeviceInfo {
    path: CString,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_string: Option<String>,
    pub manufacturer_string: Option<String>,
    pub serial_number: Option<String>,
}

impl NativeDeviceInfo {
    pub fn path_display(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

pub struct HidTransport {
    api: HidApi,
    device: HidDevice,
}

impl HidTransport {
    pub fn open_first() -> Result<Self> {
        let api = HidApi::new().map_err(to_transport_error)?;
        let info = find_device(&api).ok_or_else(|| {
            PowerBankError::Transport(
                "No Xiaomi power bank HID device found. Press the power bank button 8 times to enter data transfer mode, then connect it over USB.".to_owned(),
            )
        })?;
        Self::open_info(api, &info)
    }

    pub fn wait_for_first(poll_interval: Duration) -> Result<Self> {
        loop {
            match Self::open_first() {
                Ok(transport) => return Ok(transport),
                Err(_) => thread::sleep(poll_interval),
            }
        }
    }

    pub fn open_path(path: &str) -> Result<Self> {
        let api = HidApi::new().map_err(to_transport_error)?;
        let c_path = CString::new(path)
            .map_err(|err| PowerBankError::Transport(format!("invalid HID path: {err}")))?;
        let device = api.open_path(&c_path).map_err(to_transport_error)?;
        Ok(Self { api, device })
    }

    pub fn list() -> Result<Vec<NativeDeviceInfo>> {
        let api = HidApi::new().map_err(to_transport_error)?;
        Ok(list_devices(&api))
    }

    fn open_info(api: HidApi, info: &NativeDeviceInfo) -> Result<Self> {
        let device = api.open_path(&info.path).map_err(|err| {
            PowerBankError::Transport(format!(
                "Failed to open device {}: {err}. Linux may need udev rules; macOS may need Input Monitoring permission.",
                info.path_display()
            ))
        })?;
        Ok(Self { api, device })
    }

    pub fn refresh_devices(&mut self) -> Result<Vec<NativeDeviceInfo>> {
        self.api.refresh_devices().map_err(to_transport_error)?;
        Ok(list_devices(&self.api))
    }
}

#[async_trait(?Send)]
impl Transport for HidTransport {
    async fn write_frame(&mut self, frame: &[u8; FRAME_SIZE]) -> Result<()> {
        let mut report = [0u8; FRAME_SIZE + 1];
        report[1..].copy_from_slice(frame);
        let written = self.device.write(&report).map_err(to_transport_error)?;
        if written == 0 {
            return Err(PowerBankError::Transport(
                "HID write returned zero bytes".to_owned(),
            ));
        }
        Ok(())
    }

    async fn read_frame(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let mut buf = [0u8; 64];
        let timeout_ms = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
        let read = self
            .device
            .read_timeout(&mut buf, timeout_ms)
            .map_err(to_transport_error)?;
        if read == 0 {
            return Err(PowerBankError::Timeout);
        }

        let raw = &buf[..read];
        if raw.first() == Some(&0) && raw.get(1) == Some(&powerbank_core::HEAD) {
            Ok(raw[1..].to_vec())
        } else {
            Ok(raw.to_vec())
        }
    }
}

pub fn list_devices(api: &HidApi) -> Vec<NativeDeviceInfo> {
    api.device_list()
        .filter(|device| VENDOR_IDS.contains(&device.vendor_id()))
        .map(|device| NativeDeviceInfo {
            path: device.path().to_owned(),
            vendor_id: device.vendor_id(),
            product_id: device.product_id(),
            product_string: device.product_string().map(ToOwned::to_owned),
            manufacturer_string: device.manufacturer_string().map(ToOwned::to_owned),
            serial_number: device.serial_number().map(ToOwned::to_owned),
        })
        .collect()
}

pub fn find_device(api: &HidApi) -> Option<NativeDeviceInfo> {
    list_devices(api).into_iter().next()
}

fn to_transport_error(error: hidapi::HidError) -> PowerBankError {
    PowerBankError::Transport(error.to_string())
}
