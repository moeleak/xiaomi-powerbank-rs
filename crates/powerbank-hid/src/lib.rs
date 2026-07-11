use async_trait::async_trait;
use hidapi::{HidApi, HidDevice};
use powerbank_core::{FRAME_SIZE, PowerBankError, Result, Transport, VENDOR_IDS};
use std::ffi::CString;
use std::thread;
use std::time::{Duration, Instant};

const DEVICE_OPEN_RETRY_ATTEMPTS: usize = 5;

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
        Self::wait_for_first_inner(poll_interval, None)
    }

    pub fn wait_for_first_timeout(poll_interval: Duration, timeout: Duration) -> Result<Self> {
        Self::wait_for_first_inner(poll_interval, Some(timeout))
    }

    fn wait_for_first_inner(poll_interval: Duration, timeout: Option<Duration>) -> Result<Self> {
        let mut api = HidApi::new().map_err(to_transport_error)?;
        let mut open_failures = 0;
        let bounded = timeout.is_some();
        let mut last_open_error = None;
        let mut poll = || {
            api.refresh_devices().map_err(to_transport_error)?;
            let Some(info) = find_device(&api) else {
                open_failures = 0;
                last_open_error = None;
                return Ok(None);
            };
            match open_device(&api, &info) {
                Ok(device) => Ok(Some(device)),
                Err(err) => {
                    if bounded {
                        last_open_error = Some(err);
                        return Ok(None);
                    }
                    open_failures += 1;
                    if open_failures >= DEVICE_OPEN_RETRY_ATTEMPTS {
                        Err(err)
                    } else {
                        Ok(None)
                    }
                }
            }
        };
        let device = if let Some(timeout) = timeout {
            let device =
                poll_until_some_with_timeout(poll_interval, timeout, &mut poll, thread::sleep)?;
            device.ok_or_else(|| {
                last_open_error.unwrap_or_else(|| {
                    PowerBankError::Transport(format!(
                        "No Xiaomi power bank HID device appeared within {:.0} seconds. Press the power bank button 8 times to re-enter data transfer mode, then retry.",
                        timeout.as_secs_f32()
                    ))
                })
            })?
        } else {
            poll_until_some(poll_interval, &mut poll, thread::sleep)?
        };
        Ok(Self { api, device })
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
        let device = open_device(&api, info)?;
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

fn open_device(api: &HidApi, info: &NativeDeviceInfo) -> Result<HidDevice> {
    api.open_path(&info.path).map_err(|err| {
        PowerBankError::Transport(format!(
            "Failed to open device {}: {err}. Linux may need udev rules; macOS may need Input Monitoring permission.",
            info.path_display()
        ))
    })
}

fn poll_until_some<T>(
    poll_interval: Duration,
    mut poll: impl FnMut() -> Result<Option<T>>,
    mut sleep: impl FnMut(Duration),
) -> Result<T> {
    loop {
        if let Some(value) = poll()? {
            return Ok(value);
        }
        sleep(poll_interval);
    }
}

fn poll_until_some_with_timeout<T>(
    poll_interval: Duration,
    timeout: Duration,
    mut poll: impl FnMut() -> Result<Option<T>>,
    mut sleep: impl FnMut(Duration),
) -> Result<Option<T>> {
    let started = Instant::now();
    loop {
        if let Some(value) = poll()? {
            return Ok(Some(value));
        }

        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Ok(None);
        }
        sleep(poll_interval.min(remaining));
    }
}

fn to_transport_error(error: hidapi::HidError) -> PowerBankError {
    PowerBankError::Transport(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_retries_only_while_device_is_absent() {
        let interval = Duration::from_millis(25);
        let mut poll_count = 0;
        let mut sleeps = Vec::new();

        let value = poll_until_some(
            interval,
            || {
                poll_count += 1;
                Ok((poll_count == 3).then_some(42))
            },
            |duration| sleeps.push(duration),
        )
        .unwrap();

        assert_eq!(value, 42);
        assert_eq!(poll_count, 3);
        assert_eq!(sleeps, vec![interval, interval]);
    }

    #[test]
    fn polling_propagates_refresh_errors() {
        let interval = Duration::from_millis(25);
        let mut poll_count = 0;
        let mut sleep_count = 0;

        let err = poll_until_some::<()>(
            interval,
            || {
                poll_count += 1;
                if poll_count == 1 {
                    Ok(None)
                } else {
                    Err(PowerBankError::Transport("refresh failed".to_owned()))
                }
            },
            |_| sleep_count += 1,
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "transport error: refresh failed");
        assert_eq!(poll_count, 2);
        assert_eq!(sleep_count, 1);
    }

    #[test]
    fn bounded_poll_times_out_without_an_extra_sleep() {
        let mut polls = 0;
        let mut sleeps = 0;

        let value = poll_until_some_with_timeout::<()>(
            Duration::from_millis(25),
            Duration::ZERO,
            || {
                polls += 1;
                Ok(None)
            },
            |_| sleeps += 1,
        )
        .unwrap();

        assert!(value.is_none());
        assert_eq!(polls, 1);
        assert_eq!(sleeps, 0);
    }

    #[test]
    fn bounded_poll_returns_a_device_that_appears_before_deadline() {
        let interval = Duration::from_millis(25);
        let mut polls = 0;
        let mut sleeps = Vec::new();

        let value = poll_until_some_with_timeout(
            interval,
            Duration::from_secs(1),
            || {
                polls += 1;
                Ok((polls == 3).then_some(42))
            },
            |duration| sleeps.push(duration),
        )
        .unwrap();

        assert_eq!(value, Some(42));
        assert_eq!(polls, 3);
        assert_eq!(sleeps, vec![interval, interval]);
    }
}
