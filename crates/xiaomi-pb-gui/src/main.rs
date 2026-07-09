#![cfg_attr(windows, windows_subsystem = "windows")]

use iced::time::Instant;
use iced::{Size, Subscription, Task};
use material::widget::{button, navigation, page, text_input};
use material_ui_rs as material;
use powerbank_core::{
    BatteryInfo, CellStatus, CellTempModel, DeviceSnapshot, HelloInfo, PowerBank, Qi2Status,
    hex_upper,
};
use std::time::Duration;

const WINDOW_SIZE: Size = Size::new(1120.0, 840.0);
const MIN_WINDOW_SIZE: Size = Size::new(420.0, 720.0);

type GuiResult<T> = std::result::Result<T, String>;

pub fn main() -> iced::Result {
    material::application(App::default, update, view)
        .title("Xiaomi Powerbank")
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
    navigation::Destination::new(Page::Overview, "dashboard", "概览"),
    navigation::Destination::new(Page::Battery, "battery_full", "电池"),
    navigation::Destination::new(Page::Qi2, "settings_input_antenna", "Qi2"),
    navigation::Destination::new(Page::Raw, "terminal", "Raw"),
    navigation::Destination::new(Page::Logs, "article", "日志"),
];

#[derive(Debug)]
struct App {
    navigation: navigation::NavigationState<Page>,
    window_size: Size,
    snapshot: Option<DeviceSnapshot>,
    loading: bool,
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
            last_error: None,
            raw_input: "A5060100D9".to_owned(),
            raw_result: None,
            logs: vec![platform_hint()],
        }
    }
}

impl App {
    fn adaptive_navigation_layout(&self) -> navigation::AdaptiveLayout {
        navigation::adaptive_layout(self.window_size.width, self.window_size.height)
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
            Task::none()
        }
        Message::Refresh => {
            app.loading = true;
            app.last_error = None;
            app.log("开始读取设备信息");
            Task::perform(load_snapshot(), Message::SnapshotLoaded)
        }
        Message::SnapshotLoaded(result) => {
            app.loading = false;
            match result {
                Ok(snapshot) => {
                    app.log("设备信息读取完成");
                    app.snapshot = Some(snapshot);
                    app.last_error = None;
                }
                Err(err) => {
                    app.log(format!("设备信息读取失败: {err}"));
                    app.last_error = Some(err);
                }
            }
            Task::none()
        }
        Message::SetQi2(enable) => {
            app.loading = true;
            app.last_error = None;
            app.log(if enable {
                "请求开启 Qi2.2"
            } else {
                "请求关闭 Qi2.2"
            });
            Task::perform(set_qi2_and_reload(enable), Message::SnapshotLoaded)
        }
        Message::RawChanged(value) => {
            app.raw_input = value;
            Task::none()
        }
        Message::SendRaw => {
            app.loading = true;
            app.raw_result = None;
            app.last_error = None;
            app.log(format!("发送 Raw 命令: {}", app.raw_input));
            Task::perform(send_raw(app.raw_input.clone()), Message::RawLoaded)
        }
        Message::RawLoaded(result) => {
            app.loading = false;
            match result {
                Ok(output) => {
                    app.log("Raw 命令完成");
                    app.raw_result = Some(output);
                    app.last_error = None;
                }
                Err(err) => {
                    app.log(format!("Raw 命令失败: {err}"));
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

    if app.navigation.is_animating() {
        subscriptions.push(iced::window::frames().map(Message::Frame));
    }

    Subscription::batch(subscriptions)
}

fn view(app: &App) -> material::Element<'_, Message> {
    navigation::suite(&DESTINATIONS, &app.navigation)
        .layout(app.adaptive_navigation_layout())
        .with_menu("Xiaomi Powerbank", Message::MenuPressed)
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
        page::header("概览", "读取设备状态、充电宝型号、电量和 Qi2.2 状态"),
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
        sections.push(empty_section("还没有设备数据，先刷新读取一次。"));
    }

    page::surface(
        page::header("电池", "电池组、电芯和编号信息"),
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
            sections.push(empty_section("设备没有返回 Qi2.2 状态。"));
        }
    } else {
        sections.push(empty_section("刷新后才能显示 Qi2.2 状态。"));
    }

    page::surface(
        page::header("Qi2.2", "查询或切换无线充电能力"),
        page::sections(sections),
    )
    .into()
}

fn raw_page(app: &App) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    let input = text_input::outlined("Raw hex", &app.raw_input).on_input(Message::RawChanged);
    let send = button::button("发送", ButtonVariant::Filled).on_press(Message::SendRaw);
    let mut stack = vec![input.into(), send.into()];

    if let Some(result) = &app.raw_result {
        stack.push(material::text::body_medium(result.clone()).into());
    }
    if let Some(err) = &app.last_error {
        stack.push(material::text::body_medium(format!("错误: {err}")).into());
    }

    page::surface(
        page::header("Raw", "发送 32 字节 HID 原始帧并显示响应"),
        page::sections([page::section("命令", page::stack(stack)).into()]),
    )
    .into()
}

fn logs_page(app: &App) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    let clear = button::button("清空", ButtonVariant::Outlined).on_press(Message::ClearLog);
    let lines = if app.logs.is_empty() {
        vec![material::text::body_medium("暂无日志").into()]
    } else {
        app.logs
            .iter()
            .rev()
            .map(|line| material::text::body_medium(line.clone()).into())
            .collect::<Vec<_>>()
    };

    page::surface(
        page::header("日志", "本地操作日志和错误信息"),
        page::sections([
            page::section("操作", clear).into(),
            page::section("记录", page::stack(lines)).into(),
        ]),
    )
    .into()
}

fn status_section(app: &App) -> material::Element<'_, Message> {
    let status = if app.loading {
        "正在通信..."
    } else if app.snapshot.is_some() {
        "已读取"
    } else {
        "未连接"
    };

    let mut rows = vec![kv("状态", status)];
    rows.push(kv("运行形态", platform_label()));
    if let Some(error) = &app.last_error {
        rows.push(kv("最近错误", error));
    }

    page::section("状态", page::stack(rows)).into()
}

fn actions_section(app: &App) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    let label = if app.loading { "读取中" } else { "刷新" };
    let refresh = if app.loading {
        button::button(label, ButtonVariant::Filled)
    } else {
        button::button(label, ButtonVariant::Filled).on_press(Message::Refresh)
    };

    page::section("操作", page::row([refresh.into()])).into()
}

fn device_section(info: &HelloInfo) -> material::Element<'_, Message> {
    page::section(
        "设备",
        page::stack([
            kv(
                "产品",
                format!("{} ({})", info.device_name, info.device_model),
            ),
            kv("型号 ID", info.model_id.to_string()),
            kv("序列号", info.serial_number.clone()),
            kv("充电状态", info.charging_status.to_string()),
        ]),
    )
    .into()
}

fn battery_summary_section(battery: &BatteryInfo) -> material::Element<'_, Message> {
    page::section(
        "电池摘要",
        page::stack([
            kv("电量", format!("{}%", battery.level_pct)),
            kv("循环", format!("{} 次", battery.cycle_count)),
            kv("温度", format!("{}°C", battery.temperature_c)),
            kv(
                "电压/电流",
                format!("{}mV / {}mA", battery.voltage_mv, battery.current_ma),
            ),
        ]),
    )
    .into()
}

fn battery_detail_section(battery: &BatteryInfo) -> material::Element<'_, Message> {
    page::section(
        "电池组",
        page::stack([
            kv("状态", status_text(battery.success, battery.status_code)),
            kv(
                "激活",
                match battery.activated {
                    Some(true) => "已激活".to_owned(),
                    Some(false) => "未激活".to_owned(),
                    None => "保留".to_owned(),
                },
            ),
            kv("健康度", battery.health.to_string()),
            kv("充放电", battery.charge_state.to_string()),
            kv("故障类型", battery.fault_type.to_string()),
            kv("错误值", battery.error_value.to_string()),
            kv("历史错误", battery.history_errors.to_string()),
            kv("电芯数量", battery.cell_count.to_string()),
        ]),
    )
    .into()
}

fn cells_section(status: &CellStatus) -> material::Element<'_, Message> {
    if !status.success {
        return page::section("电芯", kv("状态", status_text(false, status.status_code))).into();
    }

    let rows = status
        .cells
        .iter()
        .map(|cell| {
            let temp = cell
                .temperature_c
                .map(|value| format!("{value}°C"))
                .unwrap_or_else(|| "无传感器".to_owned());
            kv(
                format!("#{}", cell.index),
                format!("{temp}  {}mV  {}mA", cell.voltage_mv, cell.current_ma),
            )
        })
        .collect::<Vec<_>>();

    page::section("电芯状态", page::stack(rows)).into()
}

fn battery_ids_section(snapshot: &DeviceSnapshot) -> material::Element<'_, Message> {
    let rows = snapshot
        .battery_ids
        .iter()
        .map(|id| {
            let mut parts = Vec::new();
            if !id.battery_id.is_empty() {
                parts.push(format!("编码={}", id.battery_id));
            }
            if let Some(date) = &id.production_date {
                parts.push(format!("生产日期={date}"));
            }
            if let Some(enterprise) = &id.enterprise_code {
                parts.push(format!("厂商代码={enterprise}"));
            }
            kv(format!("#{}", id.cell_index), parts.join("  "))
        })
        .collect::<Vec<_>>();

    page::section("电芯编号", page::stack(rows)).into()
}

fn cell_temp_model_section(model: &CellTempModel) -> material::Element<'_, Message> {
    page::section(
        "温度阈值与型号",
        page::stack([
            kv("电芯型号", model.battery_model.clone()),
            kv(
                "温度阈值",
                format!("{}°C ~ {}°C", model.low_temp, model.high_temp),
            ),
        ]),
    )
    .into()
}

fn qi2_section(status: &Qi2Status, loading: bool) -> material::Element<'_, Message> {
    use material::widget::button::ButtonVariant;

    let current = if status.enabled {
        "已开启"
    } else {
        "未开启"
    };
    let enable = if loading || status.enabled {
        button::button("开启", ButtonVariant::Filled)
    } else {
        button::button("开启", ButtonVariant::Filled).on_press(Message::SetQi2(true))
    };
    let disable = if loading || !status.enabled {
        button::button("关闭", ButtonVariant::Outlined)
    } else {
        button::button("关闭", ButtonVariant::Outlined).on_press(Message::SetQi2(false))
    };

    page::section(
        "Qi2.2",
        page::stack([
            kv("当前状态", current),
            page::row([enable.into(), disable.into()]).into(),
        ]),
    )
    .into()
}

fn empty_section(message: &str) -> material::Element<'_, Message> {
    page::section("提示", material::text::body_medium(message)).into()
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
        "成功".to_owned()
    } else {
        format!("失败({code})")
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
            let transport = powerbank_hid::HidTransport::open_first().map_err(to_string)?;
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
                "命令: 0x{:02X}\n负载: {}\nCRC: {}",
                parsed.cmd,
                hex_upper(&parsed.payload),
                if parsed.crc_ok { "通过" } else { "失败" }
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
        "命令: 0x{:02X}\n负载: {}\nCRC: {}",
        parsed.cmd,
        hex_upper(&parsed.payload),
        if parsed.crc_ok { "通过" } else { "失败" }
    ))
}

fn to_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn platform_label() -> &'static str {
    if cfg!(target_arch = "wasm32") {
        "WASM WebHID"
    } else {
        "桌面 HID"
    }
}

fn platform_hint() -> String {
    if cfg!(target_arch = "wasm32") {
        "WASM 版需要 Chrome/Edge、HTTPS 或 localhost，并在连接时授权 WebHID。".to_owned()
    } else {
        "桌面版会直接枚举 USB HID；Linux 可能需要 udev 规则。".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_marks_failure_code() {
        assert_eq!(status_text(false, 7), "失败(7)");
    }
}
