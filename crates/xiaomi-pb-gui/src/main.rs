#![cfg_attr(windows, windows_subsystem = "windows")]

use iced::time::Instant;
use iced::{
    ContentFit, Length,
    widget::{container, image},
};
use iced::{Size, Subscription, Task};
use material::widget::{button, log_viewer, navigation, page, progress_bar, text_input};
use material_ui_rs as material;
use powerbank_core::{
    BatteryInfo, CellStatus, CellTempModel, DeviceSnapshot, HelloInfo, PowerBank, Qi2Status,
    hex_upper,
};
use std::time::Duration;

const WINDOW_SIZE: Size = Size::new(1120.0, 840.0);
const MIN_WINDOW_SIZE: Size = Size::new(420.0, 720.0);
const APP_NAME: &str = "Xiaomi Powerbank Manager";
#[cfg(target_arch = "wasm32")]
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
#[cfg(not(target_arch = "wasm32"))]
const HID_RECONNECT_TIMEOUT: Duration = Duration::from_secs(30);

type GuiResult<T> = std::result::Result<T, String>;
type Qi2Result<T> = std::result::Result<T, Qi2Failure>;

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
    #[cfg(target_arch = "wasm32")]
    HeartbeatTick,
    #[cfg(target_arch = "wasm32")]
    HeartbeatFinished(GuiResult<()>),
    SetQi2(bool),
    ReconnectQi2(bool),
    Qi2Updated(Qi2Result<Qi2Update>),
    RawChanged(String),
    SendRaw,
    RawLoaded(GuiResult<String>),
    LogViewer(log_viewer::Action<u64>),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HidSessionState {
    NeverConnected,
    Active,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HidAccess {
    Authorized,
    Request,
}

#[derive(Debug, Clone)]
struct Qi2Update {
    status: Option<Qi2Status>,
    notice: Option<String>,
    session: HidSessionState,
}

#[derive(Debug, Clone)]
struct ExpectedDevice {
    model_id: u16,
    serial_number: String,
}

#[derive(Debug, Clone)]
enum Qi2Failure {
    Reconnect(String),
    WrongDevice(String),
    Rejected(String),
}

impl From<String> for Qi2Failure {
    fn from(error: String) -> Self {
        Self::Reconnect(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductImageAsset {
    Pb1067Mi,
    P15,
    Pb20Integrated,
    Pb2067Mi,
    P25,
    Npb1055R,
    Ac1067,
    Wpb1025S,
    Wpb0525S,
    Wpb1007Zx,
    Wpb1025,
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
    product_image: Option<(u16, image::Handle)>,
    hid_session: HidSessionState,
    loading: bool,
    heartbeat_in_flight: bool,
    progress_animation: progress_bar::IndeterminateState,
    last_error: Option<String>,
    pending_qi2: Option<bool>,
    qi2_retry_required: bool,
    qi2_notice: Option<String>,
    raw_input: String,
    raw_result: Option<String>,
    log_viewer: log_viewer::State<u64>,
    logs: Vec<log_viewer::LogEntry<u64>>,
    next_log_id: u64,
}

impl Default for App {
    fn default() -> Self {
        Self {
            navigation: navigation::NavigationState::new(Page::Overview),
            window_size: WINDOW_SIZE,
            snapshot: None,
            product_image: None,
            hid_session: HidSessionState::NeverConnected,
            loading: false,
            heartbeat_in_flight: false,
            progress_animation: progress_bar::IndeterminateState::new(Instant::now()),
            last_error: None,
            pending_qi2: None,
            qi2_retry_required: false,
            qi2_notice: None,
            raw_input: "A5060100D9".to_owned(),
            raw_result: None,
            log_viewer: log_viewer::State::new(),
            logs: vec![log_viewer::LogEntry::new(
                0,
                log_viewer::LogLevel::Info,
                format!(" {}", platform_hint()),
            )],
            next_log_id: 1,
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
        if self.loading || self.heartbeat_in_flight {
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

    fn start_qi2_update(&mut self, enable: bool, access: HidAccess) -> Task<Message> {
        if !self.begin_operation() {
            return Task::none();
        }

        self.pending_qi2 = Some(enable);
        self.qi2_retry_required = false;
        self.qi2_notice = None;
        self.last_error = None;
        self.log(match (enable, access) {
            (true, HidAccess::Authorized) => "Reconnecting to enable Qi2.2",
            (false, HidAccess::Authorized) => "Reconnecting to disable Qi2.2",
            (true, HidAccess::Request) => "Requesting a device to enable Qi2.2",
            (false, HidAccess::Request) => "Requesting a device to disable Qi2.2",
        });
        let expected = self.snapshot.as_ref().map(|snapshot| ExpectedDevice {
            model_id: snapshot.hello.model_id,
            serial_number: snapshot.hello.serial_number.clone(),
        });
        Task::perform(
            set_qi2_with_reconnect(enable, access, expected),
            Message::Qi2Updated,
        )
    }

    fn log(&mut self, entry: impl Into<String>) {
        self.push_log(log_viewer::LogLevel::Info, entry);
    }

    fn log_warning(&mut self, entry: impl Into<String>) {
        self.push_log(log_viewer::LogLevel::Warn, entry);
    }

    fn log_error(&mut self, entry: impl Into<String>) {
        self.push_log(log_viewer::LogLevel::Error, entry);
    }

    fn push_log(&mut self, level: log_viewer::LogLevel, entry: impl Into<String>) {
        let entry =
            log_viewer::LogEntry::new(self.next_log_id, level, format!(" {}", entry.into()));
        self.next_log_id += 1;
        self.logs.push(entry);
        if self.logs.len() > 200 {
            self.logs.remove(0);
            self.log_viewer.retain_entries(&self.logs);
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
            let _ = app.log_viewer.advance(now);
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
                    #[cfg(target_arch = "wasm32")]
                    let first_connection = app.snapshot.is_none();
                    app.log("Device information loaded");
                    if app.product_image.as_ref().map(|(model_id, _)| *model_id)
                        != Some(snapshot.hello.model_id)
                    {
                        app.product_image = product_image_handle(snapshot.hello.model_id)
                            .map(|handle| (snapshot.hello.model_id, handle));
                    }
                    app.snapshot = Some(snapshot);
                    app.hid_session = HidSessionState::Active;
                    app.last_error = None;
                    #[cfg(target_arch = "wasm32")]
                    if first_connection {
                        let layout = app.adaptive_navigation_layout();
                        app.navigation
                            .select(Page::Overview, Instant::now(), layout);
                    }
                }
                Err(err) => {
                    app.log_error(format!("Failed to read device information: {err}"));
                    if app.snapshot.is_some() {
                        app.hid_session = HidSessionState::Stale;
                    }
                    app.last_error = Some(err);
                }
            }
            Task::none()
        }
        #[cfg(target_arch = "wasm32")]
        Message::HeartbeatTick => {
            if app.snapshot.is_none() || app.loading || app.heartbeat_in_flight {
                return Task::none();
            }
            app.heartbeat_in_flight = true;
            Task::perform(platform_heartbeat(), Message::HeartbeatFinished)
        }
        #[cfg(target_arch = "wasm32")]
        Message::HeartbeatFinished(result) => {
            app.heartbeat_in_flight = false;
            match result {
                Ok(()) => {
                    if app.hid_session == HidSessionState::Stale {
                        app.log("HID session recovered");
                    }
                    app.hid_session = HidSessionState::Active;
                }
                Err(err) => {
                    if app.hid_session == HidSessionState::Active {
                        app.log_warning(format!(
                            "HID session became inactive; cached device data was kept: {err}"
                        ));
                    }
                    app.hid_session = HidSessionState::Stale;
                }
            }
            Task::none()
        }
        Message::SetQi2(enable) => app.start_qi2_update(enable, HidAccess::Authorized),
        Message::ReconnectQi2(enable) => app.start_qi2_update(enable, HidAccess::Request),
        Message::Qi2Updated(result) => {
            app.loading = false;
            match result {
                Ok(update) => {
                    let Qi2Update {
                        status,
                        notice,
                        session,
                    } = update;
                    let enabled = status.as_ref().map(|status| status.enabled);
                    let requested = app.pending_qi2.or(enabled).unwrap_or(false);
                    if let Some(status) = status
                        && let Some(snapshot) = app.snapshot.as_mut()
                    {
                        snapshot.qi2 = Some(status);
                    }
                    let message = if notice.is_some() {
                        if requested {
                            "Qi2.2 enable accepted; verification needs attention"
                        } else {
                            "Qi2.2 disable accepted; verification needs attention"
                        }
                    } else if enabled == Some(true) {
                        "Qi2.2 enabled"
                    } else {
                        "Qi2.2 disabled"
                    };
                    if notice.is_some() {
                        app.log_warning(message);
                    } else {
                        app.log(message);
                    }
                    app.hid_session = session;
                    app.pending_qi2 = None;
                    app.qi2_retry_required = false;
                    app.qi2_notice = notice;
                    app.last_error = None;
                }
                Err(Qi2Failure::Reconnect(err)) => {
                    let action = if app.pending_qi2 == Some(true) {
                        "enable"
                    } else {
                        "disable"
                    };
                    let error = format!(
                        "{err} Press the power bank button 8 times to re-enter HID mode, then choose Reconnect and {action}."
                    );
                    app.log_error(format!("Qi2.2 update failed: {error}"));
                    app.hid_session = HidSessionState::Stale;
                    app.qi2_retry_required = true;
                    app.qi2_notice = None;
                    app.last_error = Some(error);
                }
                Err(Qi2Failure::Rejected(error)) => {
                    app.log_error(format!("Qi2.2 update rejected: {error}"));
                    app.hid_session = HidSessionState::Active;
                    app.pending_qi2 = None;
                    app.qi2_retry_required = false;
                    app.qi2_notice = None;
                    app.last_error = Some(error);
                }
                Err(Qi2Failure::WrongDevice(error)) => {
                    app.log_error(format!("Qi2.2 update blocked: {error}"));
                    app.hid_session = HidSessionState::Stale;
                    app.qi2_retry_required = true;
                    app.qi2_notice = None;
                    app.last_error = Some(error);
                }
            }
            Task::none()
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
            let access = if app.hid_session == HidSessionState::Active {
                HidAccess::Authorized
            } else {
                HidAccess::Request
            };
            Task::perform(send_raw(app.raw_input.clone(), access), Message::RawLoaded)
        }
        Message::RawLoaded(result) => {
            app.loading = false;
            match result {
                Ok(output) => {
                    app.log("Raw command completed");
                    app.raw_result = Some(output);
                    app.hid_session = HidSessionState::Active;
                    app.last_error = None;
                }
                Err(err) => {
                    app.log_error(format!("Raw command failed: {err}"));
                    if app.snapshot.is_some() {
                        app.hid_session = HidSessionState::Stale;
                    }
                    app.last_error = Some(err);
                }
            }
            Task::none()
        }
        Message::LogViewer(action) => app.log_viewer.update(action, &app.logs),
        Message::ClearLog => {
            app.logs.clear();
            app.log_viewer.clear_selection();
            Task::none()
        }
    }
}

fn subscription(app: &App) -> Subscription<Message> {
    let mut subscriptions =
        vec![iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size))];

    if app.navigation.is_animating() || app.log_viewer.is_animating() || app.loading {
        subscriptions.push(iced::window::frames().map(Message::Frame));
    }

    #[cfg(target_arch = "wasm32")]
    if app.snapshot.is_some() {
        subscriptions.push(iced::time::every(HEARTBEAT_INTERVAL).map(|_| Message::HeartbeatTick));
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
    let mut sections = Vec::new();
    if let Some((_, handle)) = &app.product_image {
        sections.push(product_image_section(handle));
    }
    sections.extend([status_section(app), actions_section(app)]);
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

fn product_image_section(handle: &image::Handle) -> material::Element<'_, Message> {
    let product = image(handle.clone())
        .width(Length::Fill)
        .height(Length::Fixed(280.0))
        .content_fit(ContentFit::Contain);

    page::section(
        "Product",
        container(product)
            .width(Length::Fill)
            .center_x(Length::Fill),
    )
    .into()
}

fn product_image_asset(model_id: u16) -> Option<ProductImageAsset> {
    match model_id {
        1 => Some(ProductImageAsset::Pb1067Mi),
        2 | 13 => Some(ProductImageAsset::P15),
        3 | 4 | 14 => Some(ProductImageAsset::Pb20Integrated),
        5 => Some(ProductImageAsset::Pb2067Mi),
        6 => Some(ProductImageAsset::P25),
        7 => Some(ProductImageAsset::Npb1055R),
        8 => Some(ProductImageAsset::Ac1067),
        9 => Some(ProductImageAsset::Wpb1025S),
        10 => Some(ProductImageAsset::Wpb0525S),
        11 => Some(ProductImageAsset::Wpb1007Zx),
        12 => Some(ProductImageAsset::Wpb1025),
        _ => None,
    }
}

fn product_image_handle(model_id: u16) -> Option<image::Handle> {
    let bytes: &'static [u8] = match product_image_asset(model_id)? {
        ProductImageAsset::Pb1067Mi => include_bytes!("../assets/products/pb1067mi.png"),
        ProductImageAsset::P15 => include_bytes!("../assets/products/p15.png"),
        ProductImageAsset::Pb20Integrated => {
            include_bytes!("../assets/products/pb20-integrated.png")
        }
        ProductImageAsset::Pb2067Mi => include_bytes!("../assets/products/pb2067mi.png"),
        ProductImageAsset::P25 => include_bytes!("../assets/products/p25.png"),
        ProductImageAsset::Npb1055R => include_bytes!("../assets/products/npb1055r.png"),
        ProductImageAsset::Ac1067 => include_bytes!("../assets/products/ac1067.png"),
        ProductImageAsset::Wpb1025S => include_bytes!("../assets/products/wpb1025s.png"),
        ProductImageAsset::Wpb0525S => include_bytes!("../assets/products/wpb0525s.png"),
        ProductImageAsset::Wpb1007Zx => include_bytes!("../assets/products/wpb1007zx.png"),
        ProductImageAsset::Wpb1025 => include_bytes!("../assets/products/wpb1025.png"),
    };

    Some(image::Handle::from_bytes(bytes))
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
            sections.push(qi2_section(qi2, app.loading || app.heartbeat_in_flight));
        } else {
            sections.push(empty_section("The device did not return Qi2.2 status."));
        }
    } else {
        sections.push(empty_section(
            "No Qi2.2 data yet. The desktop app refreshes automatically; WebHID requires Connect.",
        ));
    }

    if let Some(enable) = app.pending_qi2
        && app.qi2_retry_required
    {
        sections.push(qi2_reconnect_section(
            enable,
            app.hid_session,
            app.loading || app.heartbeat_in_flight,
        ));
    }
    if let Some(notice) = &app.qi2_notice {
        sections.push(message_section("Notice", notice));
    }
    if let Some(error) = &app.last_error {
        sections.push(message_section("Error", error));
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
    let send = if app.loading || app.heartbeat_in_flight {
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

    let clear = if app.logs.is_empty() {
        button::button("Clear", ButtonVariant::Outlined)
    } else {
        button::button("Clear", ButtonVariant::Outlined).on_press(Message::ClearLog)
    };
    let viewer = container(
        log_viewer::view(&app.logs, &app.log_viewer, Message::LogViewer)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fixed(620.0));

    page::surface(
        page::header("Logs", "Local operation log and errors"),
        page::sections([
            page::section("Actions", clear).into(),
            page::section("Entries", viewer).into(),
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

    let show_loading = actions_show_loading(app);
    if cfg!(target_arch = "wasm32") {
        let content = if app.snapshot.is_some() {
            material::text::body_medium(match app.hid_session {
                HidSessionState::Active => "Connected",
                HidSessionState::Stale => {
                    "Device data loaded; HID will reconnect when an action needs it"
                }
                HidSessionState::NeverConnected => "Device data loaded",
            })
            .into()
        } else if show_loading {
            loading_indicator(app)
        } else {
            button::button("Connect", ButtonVariant::Filled)
                .on_press(Message::Refresh)
                .into()
        };
        return page::section("Actions", content).into();
    }

    let content = if app.snapshot.is_some() {
        material::text::body_medium("Automatic refresh completed").into()
    } else if show_loading {
        loading_indicator(app)
    } else if app.last_error.is_some() {
        button::button("Retry", ButtonVariant::Filled)
            .on_press(Message::Refresh)
            .into()
    } else {
        material::text::body_medium("Automatic refresh starts when the app opens").into()
    };

    page::section("Actions", content).into()
}

fn actions_show_loading(app: &App) -> bool {
    app.loading && app.snapshot.is_none()
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
            material::text::body_medium(
                "Changes silently reopen an authorized HID device. If the power bank has left HID mode, press its button 8 times first.",
            )
            .into(),
        ]),
    )
    .into()
}

fn qi2_reconnect_section(
    enable: bool,
    session: HidSessionState,
    loading: bool,
) -> material::Element<'static, Message> {
    use material::widget::button::ButtonVariant;

    let recovered = session == HidSessionState::Active;
    let label = if recovered && enable {
        "Retry enable"
    } else if recovered {
        "Retry disable"
    } else if enable {
        "Reconnect and enable"
    } else {
        "Reconnect and disable"
    };
    let reconnect = if loading {
        button::button(label, ButtonVariant::Filled)
    } else {
        button::button(label, ButtonVariant::Filled).on_press(if recovered {
            Message::SetQi2(enable)
        } else {
            Message::ReconnectQi2(enable)
        })
    };

    page::section(
        if recovered {
            "Retry change"
        } else {
            "Reconnect required"
        },
        page::stack([
            material::text::body_medium(if recovered {
                "The HID session recovered after the previous attempt. Retry the intended change."
            } else {
                "The last HID session is no longer available. Re-enter data transfer mode, then retry the intended change."
            })
            .into(),
            reconnect.into(),
        ]),
    )
    .into()
}

fn message_section<'a>(title: &'a str, message: &'a str) -> material::Element<'a, Message> {
    page::section(title, material::text::body_medium(message)).into()
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

async fn set_qi2_with_reconnect(
    enable: bool,
    access: HidAccess,
    expected: Option<ExpectedDevice>,
) -> Qi2Result<Qi2Update> {
    platform_set_qi2(enable, access, expected).await
}

async fn send_raw(input: String, access: HidAccess) -> GuiResult<String> {
    platform_raw(input, access).await
}

#[cfg(not(target_arch = "wasm32"))]
type NativeHidJob = Box<dyn FnOnce() + Send + 'static>;

#[cfg(not(target_arch = "wasm32"))]
fn native_hid_worker() -> GuiResult<std::sync::mpsc::Sender<NativeHidJob>> {
    use std::sync::{OnceLock, mpsc};

    // On macOS, hidapi binds its global IOHIDManager to the first caller's
    // CFRunLoop. Keep that thread alive and route every native HID call to it.
    static WORKER: OnceLock<GuiResult<mpsc::Sender<NativeHidJob>>> = OnceLock::new();

    WORKER
        .get_or_init(|| {
            let (jobs, pending_jobs) = mpsc::channel::<NativeHidJob>();
            std::thread::Builder::new()
                .name("xiaomi-pb-hid".to_owned())
                .spawn(move || {
                    while let Ok(job) = pending_jobs.recv() {
                        job();
                    }
                })
                .map(|_| jobs)
                .map_err(|err| format!("Failed to start HID worker thread: {err}"))
        })
        .clone()
}

#[cfg(not(target_arch = "wasm32"))]
async fn run_on_native_hid_worker<T, E>(
    operation: impl FnOnce() -> std::result::Result<T, E> + Send + 'static,
) -> std::result::Result<T, E>
where
    T: Send + 'static,
    E: From<String> + Send + 'static,
{
    use futures::channel::oneshot;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let worker = native_hid_worker().map_err(E::from)?;
    let (result_sender, result_receiver) = oneshot::channel();
    worker
        .send(Box::new(move || {
            let result = catch_unwind(AssertUnwindSafe(operation))
                .unwrap_or_else(|_| Err(E::from("HID operation panicked".to_owned())));
            let _ = result_sender.send(result);
        }))
        .map_err(|_| E::from("HID worker thread stopped unexpectedly".to_owned()))?;

    result_receiver
        .await
        .map_err(|_| E::from("HID worker thread stopped before returning a result".to_owned()))?
}

#[cfg(not(target_arch = "wasm32"))]
async fn platform_snapshot() -> GuiResult<DeviceSnapshot> {
    run_on_native_hid_worker(|| {
        futures::executor::block_on(async {
            let transport = powerbank_hid::HidTransport::wait_for_first(Duration::from_millis(500))
                .map_err(to_string)?;
            let mut pb = PowerBank::new(transport);
            let snapshot = pb.snapshot().await.map_err(to_string)?;
            // Do not send CMD_DISCONNECT while the GUI remains open. The
            // firmware may otherwise leave HID mode before a settings change.
            Ok(snapshot)
        })
    })
    .await
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
async fn platform_set_qi2(
    enable: bool,
    _access: HidAccess,
    expected: Option<ExpectedDevice>,
) -> Qi2Result<Qi2Update> {
    run_on_native_hid_worker(move || {
        futures::executor::block_on(async move {
            let transport = powerbank_hid::HidTransport::wait_for_first_timeout(
                Duration::from_millis(250),
                HID_RECONNECT_TIMEOUT,
            )
            .map_err(|err| Qi2Failure::Reconnect(to_string(err)))?;
            let mut pb = PowerBank::new(transport);
            apply_qi2(&mut pb, enable, expected.as_ref()).await
        })
    })
    .await
}

#[cfg(target_arch = "wasm32")]
async fn platform_set_qi2(
    enable: bool,
    access: HidAccess,
    expected: Option<ExpectedDevice>,
) -> Qi2Result<Qi2Update> {
    let transport = match access {
        HidAccess::Authorized => powerbank_webhid::WebHidTransport::open_authorized().await,
        HidAccess::Request => powerbank_webhid::WebHidTransport::select_device().await,
    }
    .map_err(|err| Qi2Failure::Reconnect(to_string(err)))?;
    let mut pb = PowerBank::new(transport);
    apply_qi2(&mut pb, enable, expected.as_ref()).await
}

async fn apply_qi2<T: powerbank_core::Transport>(
    pb: &mut PowerBank<T>,
    enable: bool,
    expected: Option<&ExpectedDevice>,
) -> Qi2Result<Qi2Update> {
    let hello = pb.handshake().await.map_err(|err| {
        Qi2Failure::Reconnect(format!("Failed to establish a HID session: {err}"))
    })?;
    if let Some(expected) = expected
        && !matches_expected_device(&hello, expected)
    {
        return Err(Qi2Failure::WrongDevice(
            "The selected HID device is not the power bank whose data is currently displayed. Select the original device and retry."
                .to_owned(),
        ));
    }
    let result = pb
        .set_qi2(enable)
        .await
        .map_err(|err| Qi2Failure::Reconnect(to_string(err)))?;
    if !result.success {
        return Err(Qi2Failure::Rejected(
            "The power bank rejected the Qi2.2 change.".to_owned(),
        ));
    }

    let expected = if enable { "enabled" } else { "disabled" };
    match pb.qi2_status().await {
        Ok(status) if status.success && status.enabled == enable => Ok(Qi2Update {
            status: Some(status),
            notice: None,
            session: HidSessionState::Active,
        }),
        Ok(status) if !status.success => Ok(Qi2Update {
            status: None,
            notice: Some(format!(
                "The power bank accepted the change to {expected}, but status verification returned a failure."
            )),
            session: HidSessionState::Active,
        }),
        Ok(status) => {
            let reported = if status.enabled {
                "enabled"
            } else {
                "disabled"
            };
            Ok(Qi2Update {
                status: Some(status),
                notice: Some(format!(
                    "The power bank accepted the change to {expected}, but read-back still reports {reported}."
                )),
                session: HidSessionState::Active,
            })
        }
        Err(err) => Ok(Qi2Update {
            status: None,
            notice: Some(format!(
                "The power bank accepted the change to {expected}, but disconnected before verification: {err}"
            )),
            session: HidSessionState::Stale,
        }),
    }
}

fn matches_expected_device(actual: &HelloInfo, expected: &ExpectedDevice) -> bool {
    actual.model_id == expected.model_id
        && (expected.serial_number.is_empty() || actual.serial_number == expected.serial_number)
}

#[cfg(target_arch = "wasm32")]
async fn platform_heartbeat() -> GuiResult<()> {
    let transport = powerbank_webhid::WebHidTransport::open_authorized()
        .await
        .map_err(to_string)?;
    let mut pb = PowerBank::new(transport);
    pb.heartbeat().await.map_err(to_string)
}

#[cfg(not(target_arch = "wasm32"))]
async fn platform_raw(input: String, _access: HidAccess) -> GuiResult<String> {
    run_on_native_hid_worker(move || {
        futures::executor::block_on(async move {
            let transport = powerbank_hid::HidTransport::wait_for_first_timeout(
                Duration::from_millis(250),
                HID_RECONNECT_TIMEOUT,
            )
            .map_err(to_string)?;
            let mut pb = PowerBank::new(transport);
            pb.handshake().await.map_err(to_string)?;
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
        })
    })
    .await
}

#[cfg(target_arch = "wasm32")]
async fn platform_raw(input: String, access: HidAccess) -> GuiResult<String> {
    let transport = match access {
        HidAccess::Authorized => powerbank_webhid::WebHidTransport::open_authorized().await,
        HidAccess::Request => powerbank_webhid::WebHidTransport::select_device().await,
    }
    .map_err(to_string)?;
    let mut pb = PowerBank::new(transport);
    pb.handshake().await.map_err(to_string)?;
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

    fn sample_snapshot(qi2: Option<Qi2Status>) -> DeviceSnapshot {
        DeviceSnapshot {
            hello: HelloInfo {
                device_name: "Xiaomi Jinshajiang Ultra-Thin Magnetic Power Bank 10000 45W"
                    .to_owned(),
                device_model: "WPB1025S".to_owned(),
                model_id: 9,
                serial_number: "test".to_owned(),
                charging_status: powerbank_core::ChargingStatus::Idle,
            },
            battery: None,
            qi2,
            cells: None,
            battery_ids: Vec::new(),
            cell_temp_model: None,
        }
    }

    #[test]
    fn status_text_marks_failure_code() {
        assert_eq!(status_text(false, 7), "failed(7)");
    }

    #[test]
    fn qi2_device_guard_matches_model_and_protocol_serial() {
        let actual = sample_snapshot(None).hello;
        let expected = ExpectedDevice {
            model_id: actual.model_id,
            serial_number: actual.serial_number.clone(),
        };
        assert!(matches_expected_device(&actual, &expected));

        let wrong_model = ExpectedDevice {
            model_id: actual.model_id + 1,
            serial_number: actual.serial_number.clone(),
        };
        assert!(!matches_expected_device(&actual, &wrong_model));

        let wrong_serial = ExpectedDevice {
            model_id: actual.model_id,
            serial_number: "another-device".to_owned(),
        };
        assert!(!matches_expected_device(&actual, &wrong_serial));
    }

    #[test]
    fn every_known_model_has_product_art() {
        for model in powerbank_core::MODEL_DB {
            assert!(
                product_image_asset(model.id).is_some(),
                "missing product art for model {} ({})",
                model.id,
                model.code
            );
        }

        assert!(product_image_asset(u16::MAX).is_none());
    }

    #[test]
    fn loaded_snapshot_caches_product_art() {
        let mut app = App {
            loading: true,
            ..App::default()
        };
        let snapshot = sample_snapshot(None);

        let _ = update(&mut app, Message::SnapshotLoaded(Ok(snapshot)));

        assert_eq!(
            app.product_image.as_ref().map(|(model_id, _)| *model_id),
            Some(9)
        );
        assert_eq!(
            app.snapshot
                .as_ref()
                .map(|snapshot| snapshot.hello.model_id),
            Some(9)
        );
        assert!(!app.loading);
        assert_eq!(app.hid_session, HidSessionState::Active);
    }

    #[test]
    fn loaded_device_actions_do_not_switch_to_loading_indicator() {
        let mut app = App {
            loading: true,
            ..App::default()
        };
        assert!(actions_show_loading(&app));

        app.snapshot = Some(sample_snapshot(None));

        assert!(!actions_show_loading(&app));
    }

    #[test]
    fn qi2_failure_keeps_cached_data_and_exposes_contextual_reconnect() {
        let mut app = App::default();
        let _ = update(
            &mut app,
            Message::SnapshotLoaded(Ok(sample_snapshot(Some(Qi2Status {
                success: true,
                enabled: false,
            })))),
        );
        app.loading = true;
        app.pending_qi2 = Some(true);

        let _ = update(
            &mut app,
            Message::Qi2Updated(Err(Qi2Failure::Reconnect("response timeout".to_owned()))),
        );

        assert!(app.snapshot.is_some());
        assert!(app.product_image.is_some());
        assert_eq!(app.pending_qi2, Some(true));
        assert!(app.qi2_retry_required);
        assert_eq!(app.hid_session, HidSessionState::Stale);
        assert!(
            app.last_error
                .as_deref()
                .is_some_and(|error| error.contains("Reconnect and enable"))
        );
        assert!(!app.loading);
    }

    #[test]
    fn qi2_success_updates_only_cached_qi2_status() {
        let mut app = App::default();
        let _ = update(
            &mut app,
            Message::SnapshotLoaded(Ok(sample_snapshot(Some(Qi2Status {
                success: true,
                enabled: true,
            })))),
        );
        app.loading = true;
        app.pending_qi2 = Some(false);

        let _ = update(
            &mut app,
            Message::Qi2Updated(Ok(Qi2Update {
                status: Some(Qi2Status {
                    success: true,
                    enabled: false,
                }),
                notice: Some("verification notice".to_owned()),
                session: HidSessionState::Active,
            })),
        );

        assert_eq!(
            app.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.qi2.as_ref())
                .map(|status| status.enabled),
            Some(false)
        );
        assert_eq!(app.pending_qi2, None);
        assert!(!app.qi2_retry_required);
        assert_eq!(app.hid_session, HidSessionState::Active);
        assert_eq!(app.qi2_notice.as_deref(), Some("verification notice"));
        assert!(app.last_error.is_none());
        assert!(!app.loading);
    }

    #[test]
    fn qi2_rejection_does_not_offer_a_reconnect_retry() {
        let mut app = App::default();
        let _ = update(
            &mut app,
            Message::SnapshotLoaded(Ok(sample_snapshot(Some(Qi2Status {
                success: true,
                enabled: false,
            })))),
        );
        app.loading = true;
        app.pending_qi2 = Some(true);

        let _ = update(
            &mut app,
            Message::Qi2Updated(Err(Qi2Failure::Rejected("rejected".to_owned()))),
        );

        assert_eq!(app.pending_qi2, None);
        assert!(!app.qi2_retry_required);
        assert_eq!(app.hid_session, HidSessionState::Active);
        assert_eq!(app.last_error.as_deref(), Some("rejected"));
        assert!(!app.loading);
    }

    #[test]
    fn wrong_qi2_device_keeps_the_intent_for_another_selection() {
        let mut app = App::default();
        let _ = update(
            &mut app,
            Message::SnapshotLoaded(Ok(sample_snapshot(Some(Qi2Status {
                success: true,
                enabled: false,
            })))),
        );
        app.loading = true;
        app.pending_qi2 = Some(true);

        let _ = update(
            &mut app,
            Message::Qi2Updated(Err(Qi2Failure::WrongDevice("wrong device".to_owned()))),
        );

        assert_eq!(app.pending_qi2, Some(true));
        assert!(app.qi2_retry_required);
        assert_eq!(app.hid_session, HidSessionState::Stale);
        assert_eq!(app.last_error.as_deref(), Some("wrong device"));
        assert!(!app.loading);
    }

    #[test]
    fn unverified_qi2_ack_preserves_known_status_and_marks_session_stale() {
        let mut app = App::default();
        let _ = update(
            &mut app,
            Message::SnapshotLoaded(Ok(sample_snapshot(Some(Qi2Status {
                success: true,
                enabled: false,
            })))),
        );
        app.loading = true;
        app.pending_qi2 = Some(true);

        let _ = update(
            &mut app,
            Message::Qi2Updated(Ok(Qi2Update {
                status: None,
                notice: Some("accepted but unverified".to_owned()),
                session: HidSessionState::Stale,
            })),
        );

        assert_eq!(
            app.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.qi2.as_ref())
                .map(|status| status.enabled),
            Some(false)
        );
        assert_eq!(app.pending_qi2, None);
        assert!(!app.qi2_retry_required);
        assert_eq!(app.hid_session, HidSessionState::Stale);
        assert_eq!(app.qi2_notice.as_deref(), Some("accepted but unverified"));
        assert!(!app.loading);
    }

    #[test]
    fn foreground_operation_is_gated_during_background_heartbeat() {
        let mut app = App {
            heartbeat_in_flight: true,
            ..App::default()
        };

        let _ = update(&mut app, Message::SetQi2(true));

        assert!(!app.loading);
        assert_eq!(app.pending_qi2, None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_app_starts_refreshing() {
        let (app, _) = App::new();

        assert!(app.loading);
        assert!(
            app.logs
                .iter()
                .any(|entry| entry.message() == " Reading device information")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_hid_operations_reuse_the_same_thread() {
        let test_thread = std::thread::current().id();
        let first = futures::executor::block_on(run_on_native_hid_worker(|| {
            Ok::<_, String>(std::thread::current().id())
        }))
        .unwrap();
        let second = futures::executor::block_on(run_on_native_hid_worker(|| {
            Ok::<_, String>(std::thread::current().id())
        }))
        .unwrap();

        assert_ne!(first, test_thread);
        assert_eq!(first, second);
    }

    #[test]
    fn refresh_is_ignored_while_loading() {
        let mut app = App {
            loading: true,
            ..App::default()
        };
        let log_count = app.logs.len();

        let _ = update(&mut app, Message::Refresh);

        assert!(app.loading);
        assert_eq!(app.logs.len(), log_count);
        assert!(app.last_error.is_none());
    }

    #[test]
    fn raw_send_is_ignored_while_loading() {
        let mut app = App {
            loading: true,
            raw_result: Some("previous result".to_owned()),
            ..App::default()
        };

        let _ = update(&mut app, Message::SendRaw);

        assert!(app.loading);
        assert_eq!(app.raw_result.as_deref(), Some("previous result"));
    }
}
