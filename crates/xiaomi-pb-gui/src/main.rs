#![cfg_attr(windows, windows_subsystem = "windows")]

use iced::time::Instant;
use iced::{Size, Subscription, Task};
use material::widget::{button, navigation, page, progress_bar, text_input};
use material_ui_rs as material;
use powerbank_core::{
    BatteryInfo, CellStatus, CellTempModel, DeviceSnapshot, HelloInfo, PowerBank, Qi2Status,
    hex_upper,
};
use std::time::Duration;

const WINDOW_SIZE: Size = Size::new(1120.0, 840.0);
const MIN_WINDOW_SIZE: Size = Size::new(420.0, 720.0);
const APP_NAME: &str = "Xiaomi Powerbank Manager";

type GuiResult<T> = std::result::Result<T, String>;

pub fn main() -> iced::Result {
    material::application(App::new, update, view)
        .title(APP_NAME)
        .subscription(subscription)
        .window(material::window_with_min_size(WINDOW_SIZE, MIN_WINDOW_SIZE))
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Navigate(Page),
    MenuPressed,
    WindowResized(Size),
    Frame(Instant),
    Refresh,
    SnapshotLoaded(GuiResult<DeviceSnapshot>),
    SetQi2(bool),
    RawChanged(String),
    SendRaw,
    RawLoaded(GuiResult<String>),
    ClearLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Battery,
    Qi2,
    Raw,
    Logs,
}

const DESTINATIONS: [navigation::Destination<Page>; 5] = [
    navigation::Destination::new(Page::Overview, "dashboard", "Overview"),
    navigation::Destination::new(Page::Battery, "battery_full", "Battery"),
    navigation::Destination::new(Page::Qi2, "settings_input_antenna", "Qi2"),
    navigation::Destination::new(Page::Raw, "terminal", "Raw"),
    navigation::Destination::new(Page::Logs, "article", "Logs"),
];

#[derive(Debug)]
struct App {
    navigation: navigation::NavigationState<Page>,
    window_size: Size,
    snapshot: Option<DeviceSnapshot>,
    loading: bool,
    progress_animation: progress_bar::IndeterminateState,
    last_error: Option<String>,
    raw_input: String,
    raw_result: Option<String>,
    logs: Vec<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            navigation: navigation::NavigationState::new(Page::Overview),
            window_size: WINDOW_SIZE,
            snapshot: None,
            loading: false,
            progress_animation: progress_bar::IndeterminateState::new(Instant::now()),
            last_error: None,
            raw_input: "A5060100D9".to_owned(),
            raw_result: None,
            logs: vec![platform_hint()],
        }
    }
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let mut app = Self::default();

        if cfg!(target_arch = "wasm32") {
            (app, Task::none())
        } else {
            let task = app.start_refresh();
            (app, task)
        }
    }

    fn adaptive_navigation_layout(&self) -> navigation::AdaptiveLayout {
        navigation::adaptive_layout(self.window_size.width, self.window_size.height)
    }

    fn begin_operation(&mut self) -> bool {
        if self.loading {
            return false;
        }

        self.loading = true;
        self.progress_animation = progress_bar::IndeterminateState::new(Instant::now());
        true
    }

    fn start_refresh(&mut self) -> Task<Message> {
        if !self.begin_operation() {
            return Task::none();
        }

        self.last_error = None;
        self.log("Reading device information");
        Task::perform(load_snapshot(), Message::SnapshotLoaded)
    }

    fn log(&mut self, entry: impl Into<String>) {
        self.logs.push(entry.into());
        if self.logs.len() > 200 {
            self.logs.remove(0);
        }
    }
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Navigate(page) => {
            app.navigation
                .select(page, Instant::now(), app.adaptive_navigation_layout());
            Task::none()
        }
        Message::MenuPressed => {
            app.navigation.toggle_menu_now();
            Task::none()
        }
        Message::WindowResized(size) => {
            app.window_size = size;
            Task::none()
        }
        Message::Frame(now) => {
            let _ = app.navigation.advance(now);
            if app.loading {
                app.progress_animation.advance(now);
            }
            Task::none()
        }
        Message::Refresh => app.start_refresh(),
        Message::SnapshotLoaded(result) => {
            app.loading = false;
            match result {
                Ok(snapshot) => {
                    app.log("Device information loaded");
                    app.snapshot = Some(snapshot);
                    app.last_error = None;
                }
                Err(err) => {
                    app.log(format!("Failed to read device information: {err}"));
                    app.last_error = Some(err);
                }
            }
            Task::none()
        }
        Message::SetQi2(enable) => {
            if !app.begin_operation() {
                return Task::none();
            }
            app.last_error = None;
            app.log(if enable {
                "Requesting Qi2.2 enable"
            } else {
                "Requesting Qi2.2 disable"
            });
            Task::perform(set_qi2_and_reload(enable), Message::SnapshotLoaded)
        }
        Message::RawChanged(value) => {
            app.raw_input = value;
            Task::none()
        }
        Message::SendRaw => {
            if !app.begin_operation() {
                return Task::none();
            }
            app.raw_result = None;
            app.last_error = None;
            app.log(format!("Sending raw command: {}", app.raw_input));
            Task::perform(send_raw(app.raw_input.clone()), Message::RawLoaded)
        }
        Message::RawLoaded(result) => {
            app.loading = false;
            match result {
                Ok(output) => {
                    app.log("Raw command completed");
                    app.raw_result = Some(output);
                    app.last_error = None;
                }
                Err(err) => {
                    app.log(format!("Raw command failed: {err}"));
                    app.last_error = Some(err);
                }
            }
            Task::none()
        }
        Message::ClearLog => {
            app.logs.clear();
            Task::none()
        }
    }
}

fn subscription(app: &App) -> Subscription<Message> {
    let mut subscriptions =
        vec![iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size))];

    if app.navigation.is_animating() || app.loading {
        subscriptions.push(iced::window::frames().map(Message::Frame));
    }

    Subscription::batch(subscriptions)
}

fn view(app: &App) -> material::Element<'_, Message> {
    navigation::suite(&DESTINATIONS, &app.navigation)
        .layout(app.adaptive_navigation_layout())
        .with_menu("Manager", Message::MenuPressed)
        .view(Message::Navigate, app.navigation.selected().view(app))
}

impl Page {
    fn view(self, app: &App) -> material::Element<'_, Message> {
        match self {
            Self::Overview => overview_page(app),
            Self::Battery => battery_page(app),
            Self::Qi2 => qi2_page(app),
            Self::Raw => raw_page(app),
            Self::Logs => logs_page(app),
        }
    }
}

fn overview_page(app: &App) -> material::Element<'_, Message> {
    let mut sections = vec![status_section(app), actions_section(app)];
    if let Some(snapshot) = &app.snapshot {
        sections.push(device_section(&snapshot.hello));
        if let Some(battery) = &snapshot.battery {
            sections.push(battery_summary_section(battery));
        }
    }

    page::surface(
        page::header(
            "Overview",
            "Read device state, model, battery level, and Qi2.2 status",
        ),
        page::sections(sections),
    )
    .into()
}

fn battery_page(app: &App) -> material::Element<'_, Message> {
    let mut sections = vec![actions_section(app)];
    if let Some(snapshot) = &app.snapshot {
        if let Some(battery) = &snapshot.battery {
            sections.push(battery_detail_section(battery));
        }
        if let Some(cells) = &snapshot.cells {
            sections.push(cells_section(cells));
        }
        if !snapshot.battery_ids.is_empty() {
            sections.push(battery_ids_section(snapshot));
        }
        if let Some(model) = &snapshot.cell_temp_model {
            sections.push(cell_temp_model_section(model));
        }
    } else {
        sections.push(empty_section(
            "No device data yet. The desktop app refreshes automatically; WebHID requires Connect.",
        ));
    }

    page::surface(
        page::header("Battery", "Battery pack, cell readings, and cell IDs"),
        page::sections(sections),
    )
    .into()
}

fn qi2_page(app: &App) -> material::Element<'_, Message> {
    let mut sections = vec![actions_section(app)];
    if let Some(snapshot) = &app.snapshot {
        if let Some(qi2) = &snapshot.qi2 {
            sections.push(qi2_section(qi2, app.loading));
        } else {
            sections.push(empty_section("The device did not return Qi2.2 status."));
        }
    } else {
        sections.push(empty_section(
            "No Qi2.2 data yet. The desktop app refreshes automatically; WebHID requires Connect.",
        ));
    }

    page::surface(
        page::header("Qi2.2", "Query or switch wireless charging support"),
        page::sections(sections),
    )
    .into()
}

fn raw_page(app: &App) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    let input = text_input::outlined("Raw hex", &app.raw_input).on_input(Message::RawChanged);
    let send = if app.loading {
        button::button("Send", ButtonVariant::Filled)
    } else {
        button::button("Send", ButtonVariant::Filled).on_press(Message::SendRaw)
    };
    let mut stack = vec![input.into(), send.into()];

    if let Some(result) = &app.raw_result {
        stack.push(material::text::body_medium(result.clone()).into());
    }
    if let Some(err) = &app.last_error {
        stack.push(material::text::body_medium(format!("Error: {err}")).into());
    }

    page::surface(
        page::header(
            "Raw",
            "Send a 32-byte raw HID frame and inspect the response",
        ),
        page::sections([page::section("Command", page::stack(stack)).into()]),
    )
    .into()
}

fn logs_page(app: &App) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    let clear = button::button("Clear", ButtonVariant::Outlined).on_press(Message::ClearLog);
    let lines = if app.logs.is_empty() {
        vec![material::text::body_medium("No logs").into()]
    } else {
        app.logs
            .iter()
            .rev()
            .map(|line| material::text::body_medium(line.clone()).into())
            .collect::<Vec<_>>()
    };

    page::surface(
        page::header("Logs", "Local operation log and errors"),
        page::sections([
            page::section("Actions", clear).into(),
            page::section("Entries", page::stack(lines)).into(),
        ]),
    )
    .into()
}

fn status_section(app: &App) -> material::Element<'_, Message> {
    let status = if app.loading {
        "Communicating..."
    } else if app.snapshot.is_some() {
        "Loaded"
    } else {
        "Disconnected"
    };

    let mut rows = vec![kv("Status", status)];
    if app.loading {
        rows.push(loading_indicator(app));
    }
    rows.push(kv("Runtime", platform_label()));
    if let Some(error) = &app.last_error {
        rows.push(kv("Last error", error));
    }

    page::section("Status", page::stack(rows)).into()
}

fn loading_indicator(app: &App) -> material::Element<'_, Message> {
    use progress_bar::LoadingIndicatorMode;

    page::row([
        progress_bar::loading(LoadingIndicatorMode::contained_indeterminate(
            app.progress_animation.loading_phase(),
        ))
        .into(),
        material::text::body_medium("Communicating with device").into(),
    ])
    .into()
}

fn actions_section(app: &App) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    if cfg!(target_arch = "wasm32") {
        let label = if app.loading { "Loading" } else { "Connect" };
        let refresh = if app.loading {
            button::button(label, ButtonVariant::Filled)
        } else {
            button::button(label, ButtonVariant::Filled).on_press(Message::Refresh)
        };
        return page::section("Actions", page::row([refresh.into()])).into();
    }

    let content = if app.loading {
        loading_indicator(app)
    } else if app.snapshot.is_some() {
        material::text::body_medium("Automatic refresh completed").into()
    } else if app.last_error.is_some() {
        material::text::body_medium("Automatic refresh failed").into()
    } else {
        material::text::body_medium("Automatic refresh starts when the app opens").into()
    };

    page::section("Actions", content).into()
}

fn device_section(info: &HelloInfo) -> material::Element<'_, Message> {
    page::section(
        "Device",
        page::stack([
            kv(
                "Product",
                format!("{} ({})", info.device_name, info.device_model),
            ),
            kv("Model ID", info.model_id.to_string()),
            kv("Serial", info.serial_number.clone()),
            kv("Power state", info.charging_status.to_string()),
        ]),
    )
    .into()
}

fn battery_summary_section(battery: &BatteryInfo) -> material::Element<'_, Message> {
    page::section(
        "Battery summary",
        page::stack([
            kv("Level", format!("{}%", battery.level_pct)),
            kv("Cycles", battery.cycle_count.to_string()),
            kv("Temperature", format!("{}°C", battery.temperature_c)),
            kv(
                "Voltage / current",
                format!("{}mV / {}mA", battery.voltage_mv, battery.current_ma),
            ),
        ]),
    )
    .into()
}

fn battery_detail_section(battery: &BatteryInfo) -> material::Element<'_, Message> {
    page::section(
        "Battery pack",
        page::stack([
            kv("Status", status_text(battery.success, battery.status_code)),
            kv(
                "Activation",
                match battery.activated {
                    Some(true) => "Activated".to_owned(),
                    Some(false) => "Not activated".to_owned(),
                    None => "Reserved".to_owned(),
                },
            ),
            kv("Health", battery.health.to_string()),
            kv("Charge state", battery.charge_state.to_string()),
            kv("Fault type", battery.fault_type.to_string()),
            kv("Error value", battery.error_value.to_string()),
            kv("History errors", battery.history_errors.to_string()),
            kv("Cell count", battery.cell_count.to_string()),
        ]),
    )
    .into()
}

fn cells_section(status: &CellStatus) -> material::Element<'_, Message> {
    if !status.success {
        return page::section(
            "Cells",
            kv("Status", status_text(false, status.status_code)),
        )
        .into();
    }

    let rows = status
        .cells
        .iter()
        .map(|cell| {
            let temp = cell
                .temperature_c
                .map(|value| format!("{value}°C"))
                .unwrap_or_else(|| "No sensor".to_owned());
            kv(
                format!("#{}", cell.index),
                format!("{temp}  {}mV  {}mA", cell.voltage_mv, cell.current_ma),
            )
        })
        .collect::<Vec<_>>();

    page::section("Cell status", page::stack(rows)).into()
}

fn battery_ids_section(snapshot: &DeviceSnapshot) -> material::Element<'_, Message> {
    let rows = snapshot
        .battery_ids
        .iter()
        .map(|id| {
            let mut parts = Vec::new();
            if !id.battery_id.is_empty() {
                parts.push(format!("id={}", id.battery_id));
            }
            if let Some(date) = &id.production_date {
                parts.push(format!("production_date={date}"));
            }
            if let Some(enterprise) = &id.enterprise_code {
                parts.push(format!("vendor_code={enterprise}"));
            }
            kv(format!("#{}", id.cell_index), parts.join("  "))
        })
        .collect::<Vec<_>>();

    page::section("Cell IDs", page::stack(rows)).into()
}

fn cell_temp_model_section(model: &CellTempModel) -> material::Element<'_, Message> {
    page::section(
        "Temperature range and model",
        page::stack([
            kv("Cell model", model.battery_model.clone()),
            kv(
                "Temperature range",
                format!("{}°C ~ {}°C", model.low_temp, model.high_temp),
            ),
        ]),
    )
    .into()
}

fn qi2_section(status: &Qi2Status, loading: bool) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    let current = if status.enabled {
        "Enabled"
    } else {
        "Disabled"
    };
    let enable = if loading || status.enabled {
        button::button("Enable", ButtonVariant::Filled)
    } else {
        button::button("Enable", ButtonVariant::Filled).on_press(Message::SetQi2(true))
    };
    let disable = if loading || !status.enabled {
        button::button("Disable", ButtonVariant::Outlined)
    } else {
        button::button("Disable", ButtonVariant::Outlined).on_press(Message::SetQi2(false))
    };

    page::section(
        "Qi2.2",
        page::stack([
            kv("Current status", current),
            page::row([enable.into(), disable.into()]).into(),
        ]),
    )
    .into()
}

fn empty_section(message: &str) -> material::Element<'_, Message> {
    page::section("Notice", material::text::body_medium(message)).into()
}

fn kv<'a>(label: impl Into<String>, value: impl Into<String>) -> material::Element<'a, Message> {
    page::row([
        material::text::body_medium(label.into()).into(),
        material::text::body_large(value.into()).into(),
    ])
    .into()
}

fn status_text(success: bool, code: u8) -> String {
    if success {
        "ok".to_owned()
    } else {
        format!("failed({code})")
    }
}

async fn load_snapshot() -> GuiResult<DeviceSnapshot> {
    platform_snapshot().await
}

async fn set_qi2_and_reload(enable: bool) -> GuiResult<DeviceSnapshot> {
    platform_set_qi2(enable).await
}

async fn send_raw(input: String) -> GuiResult<String> {
    platform_raw(input).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn platform_snapshot() -> GuiResult<DeviceSnapshot> {
    std::thread::spawn(|| {
        futures::executor::block_on(async {
            let transport = powerbank_hid::HidTransport::wait_for_first(Duration::from_millis(500))
                .map_err(to_string)?;
            let mut pb = PowerBank::new(transport);
            let snapshot = pb.snapshot().await.map_err(to_string)?;
            let _ = pb.disconnect().await;
            Ok(snapshot)
        })
    })
    .join()
    .map_err(|_| "HID worker thread panicked".to_owned())?
}

#[cfg(target_arch = "wasm32")]
async fn platform_snapshot() -> GuiResult<DeviceSnapshot> {
    let transport = powerbank_webhid::WebHidTransport::request_device()
        .await
        .map_err(to_string)?;
    let mut pb = PowerBank::new(transport);
    pb.snapshot().await.map_err(to_string)
}

#[cfg(not(target_arch = "wasm32"))]
async fn platform_set_qi2(enable: bool) -> GuiResult<DeviceSnapshot> {
    std::thread::spawn(move || {
        futures::executor::block_on(async move {
            let transport = powerbank_hid::HidTransport::open_first().map_err(to_string)?;
            let mut pb = PowerBank::new(transport);
            let _ = pb.handshake().await;
            pb.set_qi2(enable).await.map_err(to_string)?;
            let snapshot = pb.snapshot().await.map_err(to_string)?;
            let _ = pb.disconnect().await;
            Ok(snapshot)
        })
    })
    .join()
    .map_err(|_| "HID worker thread panicked".to_owned())?
}

#[cfg(target_arch = "wasm32")]
async fn platform_set_qi2(enable: bool) -> GuiResult<DeviceSnapshot> {
    let transport = powerbank_webhid::WebHidTransport::request_device()
        .await
        .map_err(to_string)?;
    let mut pb = PowerBank::new(transport);
    let _ = pb.handshake().await;
    pb.set_qi2(enable).await.map_err(to_string)?;
    pb.snapshot().await.map_err(to_string)
}

#[cfg(not(target_arch = "wasm32"))]
async fn platform_raw(input: String) -> GuiResult<String> {
    std::thread::spawn(move || {
        futures::executor::block_on(async move {
            let transport = powerbank_hid::HidTransport::open_first().map_err(to_string)?;
            let mut pb = PowerBank::new(transport);
            let _ = pb.handshake().await;
            let parsed = pb
                .raw(&input, Duration::from_millis(3_000))
                .await
                .map_err(to_string)?;
            let _ = pb.disconnect().await;
            Ok(format!(
                "Command: 0x{:02X}\nPayload: {}\nCRC: {}",
                parsed.cmd,
                hex_upper(&parsed.payload),
                if parsed.crc_ok { "ok" } else { "failed" }
            ))
        })
    })
    .join()
    .map_err(|_| "HID worker thread panicked".to_owned())?
}

#[cfg(target_arch = "wasm32")]
async fn platform_raw(input: String) -> GuiResult<String> {
    let transport = powerbank_webhid::WebHidTransport::request_device()
        .await
        .map_err(to_string)?;
    let mut pb = PowerBank::new(transport);
    let _ = pb.handshake().await;
    let parsed = pb
        .raw(&input, Duration::from_millis(3_000))
        .await
        .map_err(to_string)?;
    Ok(format!(
        "Command: 0x{:02X}\nPayload: {}\nCRC: {}",
        parsed.cmd,
        hex_upper(&parsed.payload),
        if parsed.crc_ok { "ok" } else { "failed" }
    ))
}

fn to_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn platform_label() -> &'static str {
    if cfg!(target_arch = "wasm32") {
        "WASM WebHID"
    } else {
        "Desktop HID"
    }
}

fn platform_hint() -> String {
    if cfg!(target_arch = "wasm32") {
        "The WASM build requires Chrome or Edge, HTTPS or localhost, and WebHID permission when connecting.".to_owned()
    } else {
        "The desktop build enumerates USB HID directly; Linux may need udev rules.".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_marks_failure_code() {
        assert_eq!(status_text(false, 7), "failed(7)");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_app_starts_refreshing() {
        let (app, _) = App::new();

        assert!(app.loading);
        assert!(
            app.logs
                .iter()
                .any(|line| line == "Reading device information")
        );
    }

    #[test]
    fn refresh_is_ignored_while_loading() {
        let mut app = App::default();
        app.loading = true;
        let log_count = app.logs.len();

        let _ = update(&mut app, Message::Refresh);

        assert!(app.loading);
        assert_eq!(app.logs.len(), log_count);
        assert!(app.last_error.is_none());
    }

    #[test]
    fn raw_send_is_ignored_while_loading() {
        let mut app = App::default();
        app.loading = true;
        app.raw_result = Some("previous result".to_owned());

        let _ = update(&mut app, Message::SendRaw);

        assert!(app.loading);
        assert_eq!(app.raw_result.as_deref(), Some("previous result"));
    }
}
