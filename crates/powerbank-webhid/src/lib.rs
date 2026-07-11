use async_trait::async_trait;
use powerbank_core::{FRAME_SIZE, PowerBankError, Result, Transport};
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use js_sys::{Array, Promise, Uint8Array};
#[cfg(target_arch = "wasm32")]
use powerbank_core::VENDOR_IDS;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::JsFuture;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = r#"
export function webhidSupported() {
  return typeof navigator !== "undefined" && !!navigator.hid;
}

let cachedDevice = null;
let selectedDeviceIdentity = null;
const knownVendorIds = new Set();
const DEVICE_IO_TIMEOUT_MS = 5000;
const MAX_QUEUED_REPORTS = 64;
const deviceStates = new WeakMap();

function withTimeout(operation, timeoutMs, message) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) {
        return;
      }

      settled = true;
      reject(new Error(message));
    }, timeoutMs);

    Promise.resolve(operation).then(
      (value) => {
        if (settled) {
          return;
        }

        settled = true;
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        if (settled) {
          return;
        }

        settled = true;
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function stateForDevice(device) {
  let state = deviceStates.get(device);
  if (state) {
    return state;
  }

  state = {
    reports: [],
    waiters: [],
    onReport: null,
  };
  state.onReport = (event) => {
    const view = event.data;
    const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
    const report = new Uint8Array(bytes);
    const waiter = state.waiters.shift();

    if (waiter) {
      clearTimeout(waiter.timer);
      waiter.resolve(report);
      return;
    }

    if (state.reports.length >= MAX_QUEUED_REPORTS) {
      state.reports.shift();
    }
    state.reports.push(report);
  };

  device.addEventListener("inputreport", state.onReport);
  deviceStates.set(device, state);
  return state;
}

function forgetDevice(device, reason) {
  const state = deviceStates.get(device);
  if (state) {
    device.removeEventListener("inputreport", state.onReport);
    for (const waiter of state.waiters) {
      clearTimeout(waiter.timer);
      waiter.reject(reason);
    }
    deviceStates.delete(device);
  }

  if (cachedDevice === device) {
    cachedDevice = null;
  }
}

function identityForDevice(device) {
  return {
    vendorId: device.vendorId,
    productId: device.productId,
  };
}

function matchesSelectedDevice(device) {
  if (!selectedDeviceIdentity) {
    return true;
  }

  return device.vendorId === selectedDeviceIdentity.vendorId &&
    device.productId === selectedDeviceIdentity.productId;
}

if (typeof navigator !== "undefined" && navigator.hid) {
  navigator.hid.addEventListener("connect", (event) => {
    if (
      !cachedDevice &&
      knownVendorIds.has(event.device.vendorId) &&
      matchesSelectedDevice(event.device)
    ) {
      cachedDevice = event.device;
    }
  });
  navigator.hid.addEventListener("disconnect", (event) => {
    forgetDevice(event.device, new Error("device disconnected"));
  });
}

function matchesVendor(device, vendorIds) {
  return Array.from(vendorIds).some((vendorId) => device.vendorId === Number(vendorId));
}

async function openDevice(device) {
  stateForDevice(device);

  if (!device.opened) {
    await withTimeout(device.open(), DEVICE_IO_TIMEOUT_MS, "device open timeout");
  }

  cachedDevice = device;
  return device;
}

export async function webhidOpenAuthorizedDevice(vendorIds) {
  if (!webhidSupported()) {
    throw new Error("This browser does not support WebHID");
  }

  for (const vendorId of vendorIds) {
    knownVendorIds.add(Number(vendorId));
  }

  let lastError = null;
  if (cachedDevice && matchesVendor(cachedDevice, vendorIds)) {
    const device = cachedDevice;
    try {
      return await openDevice(device);
    } catch (error) {
      lastError = error;
      forgetDevice(device, error);
    }
  }

  const authorized = await withTimeout(
    navigator.hid.getDevices(),
    DEVICE_IO_TIMEOUT_MS,
    "authorized device lookup timeout",
  );
  for (const device of authorized.filter(
    (device) => matchesVendor(device, vendorIds) && matchesSelectedDevice(device),
  )) {
    try {
      return await openDevice(device);
    } catch (error) {
      lastError = error;
      forgetDevice(device, error);
    }
  }

  if (lastError) {
    throw lastError;
  }

  throw new Error("No authorized Xiaomi power bank HID device is available");
}

export async function webhidRequestDevice(vendorIds) {
  return await webhidSelectDevice(vendorIds);
}

export async function webhidSelectDevice(vendorIds) {
  if (!webhidSupported()) {
    throw new Error("This browser does not support WebHID");
  }

  for (const vendorId of vendorIds) {
    knownVendorIds.add(Number(vendorId));
  }

  const filters = Array.from(vendorIds).map((vendorId) => ({ vendorId }));
  const devices = await navigator.hid.requestDevice({ filters });

  if (!devices.length) {
    throw new Error("No HID device was selected");
  }

  const device = await openDevice(devices[0]);
  selectedDeviceIdentity = identityForDevice(device);
  return device;
}

export async function webhidSendReport(device, data) {
  try {
    await openDevice(device);
    stateForDevice(device).reports.length = 0;
    await withTimeout(
      device.sendReport(0, data),
      DEVICE_IO_TIMEOUT_MS,
      "report send timeout",
    );
  } catch (error) {
    forgetDevice(device, error);
    throw error;
  }
}

export function webhidReadReport(device, timeoutMs) {
  const state = stateForDevice(device);
  if (state.reports.length) {
    return Promise.resolve(state.reports.shift());
  }

  return new Promise((resolve, reject) => {
    const waiter = { resolve, reject, timer: null };
    const timer = setTimeout(() => {
      forgetDevice(device, new Error("response timeout"));
    }, timeoutMs);
    waiter.timer = timer;
    state.waiters.push(waiter);
  });
}
"#)]
unsafe extern "C" {
    #[wasm_bindgen(js_name = webhidSupported)]
    fn webhid_supported_js() -> bool;

    #[wasm_bindgen(js_name = webhidRequestDevice)]
    fn webhid_request_device(vendor_ids: &Array) -> Promise;

    #[wasm_bindgen(js_name = webhidOpenAuthorizedDevice)]
    fn webhid_open_authorized_device(vendor_ids: &Array) -> Promise;

    #[wasm_bindgen(js_name = webhidSelectDevice)]
    fn webhid_select_device(vendor_ids: &Array) -> Promise;

    #[wasm_bindgen(js_name = webhidSendReport)]
    fn webhid_send_report(device: &JsValue, data: &Uint8Array) -> Promise;

    #[wasm_bindgen(js_name = webhidReadReport)]
    fn webhid_read_report(device: &JsValue, timeout_ms: u32) -> Promise;
}

#[cfg(target_arch = "wasm32")]
pub struct WebHidTransport {
    device: JsValue,
}

#[cfg(target_arch = "wasm32")]
impl WebHidTransport {
    pub fn supported() -> bool {
        webhid_supported_js()
    }

    pub async fn request_device() -> Result<Self> {
        Self::open_with(webhid_request_device).await
    }

    pub async fn open_authorized() -> Result<Self> {
        Self::open_with(webhid_open_authorized_device).await
    }

    pub async fn select_device() -> Result<Self> {
        Self::open_with(webhid_select_device).await
    }

    async fn open_with(open: fn(&Array) -> Promise) -> Result<Self> {
        let vendor_ids = Array::new();
        for vendor_id in VENDOR_IDS {
            vendor_ids.push(&JsValue::from_f64(f64::from(vendor_id)));
        }

        let device = JsFuture::from(open(&vendor_ids)).await.map_err(js_error)?;

        Ok(Self { device })
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait(?Send)]
impl Transport for WebHidTransport {
    async fn write_frame(&mut self, frame: &[u8; FRAME_SIZE]) -> Result<()> {
        let data = Uint8Array::from(frame.as_slice());
        JsFuture::from(webhid_send_report(&self.device, &data))
            .await
            .map_err(js_error)?;
        Ok(())
    }

    async fn read_frame(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let timeout_ms = timeout.as_millis().clamp(1, u32::MAX as u128) as u32;
        let value = JsFuture::from(webhid_read_report(&self.device, timeout_ms))
            .await
            .map_err(js_error)?;
        let raw = Uint8Array::new(&value).to_vec();

        if raw.first() == Some(&0) && raw.get(1) == Some(&powerbank_core::HEAD) {
            Ok(raw[1..].to_vec())
        } else {
            Ok(raw)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy)]
pub struct WebHidTransport;

#[cfg(not(target_arch = "wasm32"))]
impl WebHidTransport {
    pub fn supported() -> bool {
        false
    }

    pub async fn request_device() -> Result<Self> {
        Err(PowerBankError::Transport(
            "WebHID is only available for wasm32 builds".to_owned(),
        ))
    }

    pub async fn open_authorized() -> Result<Self> {
        Err(PowerBankError::Transport(
            "WebHID is only available for wasm32 builds".to_owned(),
        ))
    }

    pub async fn select_device() -> Result<Self> {
        Err(PowerBankError::Transport(
            "WebHID is only available for wasm32 builds".to_owned(),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait(?Send)]
impl Transport for WebHidTransport {
    async fn write_frame(&mut self, _frame: &[u8; FRAME_SIZE]) -> Result<()> {
        Err(PowerBankError::Transport(
            "WebHID is only available for wasm32 builds".to_owned(),
        ))
    }

    async fn read_frame(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
        Err(PowerBankError::Transport(
            "WebHID is only available for wasm32 builds".to_owned(),
        ))
    }
}

#[cfg(target_arch = "wasm32")]
fn js_error(value: JsValue) -> PowerBankError {
    let message = value
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(&value, &JsValue::from_str("message"))
                .ok()?
                .as_string()
        })
        .unwrap_or_else(|| format!("{value:?}"));
    PowerBankError::Transport(message)
}
