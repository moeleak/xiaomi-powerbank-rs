use anyhow::{Context as _, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use futures::executor::block_on;
use owo_colors::OwoColorize;
use powerbank_core::{
    BatteryIdInfo, BatteryInfo, CellStatus, CellTempModel, DeviceSnapshot, HelloInfo, PowerBank,
    Qi2Status, hex_upper,
};
use powerbank_hid::{HidTransport, NativeDeviceInfo};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, EditMode, Editor, Helper};
use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(name = "xiaomi-pb", version, about = "小米充电宝 USB HID 配置工具")]
struct Cli {
    #[arg(long, global = true, help = "显示 HID 通信日志")]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "显示全部设备信息")]
    Info,
    #[command(about = "列出匹配的 HID 设备")]
    Devices,
    #[command(about = "Qi2.2 状态与开关")]
    Qi2 {
        #[command(subcommand)]
        command: Qi2Command,
    },
    #[command(name = "qi2-enable", hide = true)]
    Qi2Enable,
    #[command(name = "qi2-disable", hide = true)]
    Qi2Disable,
    #[command(about = "发送原始十六进制 HID 帧")]
    Raw {
        #[arg(help = "十六进制数据，例如 A5060100C8")]
        hex: String,
        #[arg(long, short, default_value_t = 3_000, help = "响应超时，单位毫秒")]
        timeout: u64,
        #[arg(long, short, help = "显示完整解析信息")]
        verbose: bool,
    },
    #[command(about = "生成 shell 补全脚本")]
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum Qi2Command {
    #[command(about = "查询 Qi2.2 状态")]
    Status,
    #[command(about = "开启 Qi2.2")]
    Enable,
    #[command(about = "关闭 Qi2.2")]
    Disable,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Info) => run_info(cli.debug),
        Some(Commands::Devices) => run_devices(),
        Some(Commands::Qi2 { command }) => match command {
            Qi2Command::Status => run_qi2_status(cli.debug),
            Qi2Command::Enable => run_qi2_set(cli.debug, true),
            Qi2Command::Disable => run_qi2_set(cli.debug, false),
        },
        Some(Commands::Qi2Enable) => run_qi2_set(cli.debug, true),
        Some(Commands::Qi2Disable) => run_qi2_set(cli.debug, false),
        Some(Commands::Raw {
            hex,
            timeout,
            verbose,
        }) => run_raw(cli.debug, &hex, timeout, verbose),
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "xiaomi-pb", &mut std::io::stdout());
            Ok(())
        }
        None => interactive_mode(cli.debug),
    }
}

fn run_info(debug: bool) -> Result<()> {
    let mut pb = connect_powerbank(debug)?;
    let snapshot = block_on(pb.snapshot()).context("读取设备信息失败")?;
    print_snapshot(&snapshot);
    let _ = block_on(pb.disconnect());
    Ok(())
}

fn run_devices() -> Result<()> {
    let devices = HidTransport::list().context("枚举 HID 设备失败")?;
    if devices.is_empty() {
        println!("{}", "未找到匹配的小米充电宝 HID 设备".yellow());
        return Ok(());
    }

    for (index, device) in devices.iter().enumerate() {
        print_device(index + 1, device);
    }
    Ok(())
}

fn run_qi2_status(debug: bool) -> Result<()> {
    let mut pb = connect_powerbank(debug)?;
    let _ = block_on(pb.handshake());
    let status = block_on(pb.qi2_status()).context("查询 Qi2.2 状态失败")?;
    print_qi2_status(&status);
    let _ = block_on(pb.disconnect());
    Ok(())
}

fn run_qi2_set(debug: bool, enable: bool) -> Result<()> {
    let mut pb = connect_powerbank(debug)?;
    let _ = block_on(pb.handshake());
    let current = block_on(pb.qi2_status()).ok();

    if let Some(status) = current
        && status.enabled == enable
    {
        let text = if enable {
            "Qi2.2 已经开启"
        } else {
            "Qi2.2 未开启，无需关闭"
        };
        println!("{}", text.yellow());
        let _ = block_on(pb.disconnect());
        return Ok(());
    }

    let result = block_on(pb.set_qi2(enable)).context("设置 Qi2.2 失败")?;
    if result.success {
        let text = if enable {
            "Qi2.2 开启成功"
        } else {
            "Qi2.2 关闭成功"
        };
        println!("{}", text.green());
    } else {
        let text = if enable {
            "Qi2.2 开启失败"
        } else {
            "Qi2.2 关闭失败"
        };
        println!("{}", text.red());
    }
    let _ = block_on(pb.disconnect());
    Ok(())
}

fn run_raw(debug: bool, hex: &str, timeout: u64, verbose: bool) -> Result<()> {
    let mut pb = connect_powerbank(debug)?;
    let _ = block_on(pb.handshake());
    let parsed =
        block_on(pb.raw(hex, Duration::from_millis(timeout))).context("发送原始命令失败")?;
    println!("{} 0x{:02X}", "命令:".bold(), parsed.cmd);
    println!("{} {}", "负载:".bold(), hex_upper(&parsed.payload).cyan());
    println!(
        "{} {}",
        "CRC:".bold(),
        if parsed.crc_ok {
            "通过".green().to_string()
        } else {
            format!(
                "失败 expected=0x{:02X} received=0x{:02X}",
                parsed.crc_expected, parsed.crc_received
            )
            .red()
            .to_string()
        }
    );

    if verbose {
        println!("{} {:?}", "解析:".bold(), parsed);
    }

    let _ = block_on(pb.disconnect());
    Ok(())
}

fn connect_powerbank(debug: bool) -> Result<PowerBank<HidTransport>> {
    eprint!("{}", "正在查找小米充电宝 HID 设备".cyan());
    loop {
        match HidTransport::open_first() {
            Ok(transport) => {
                eprintln!();
                return Ok(PowerBank::new(transport).with_debug(debug));
            }
            Err(err) => {
                eprint!(".");
                let _ = std::io::Write::flush(&mut std::io::stderr());
                if debug {
                    eprintln!("\n{err}");
                }
                thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

fn interactive_mode(debug: bool) -> Result<()> {
    let mut pb = connect_powerbank(debug)?;
    let hello = block_on(pb.handshake()).context("握手失败")?;
    println!(
        "{} {} ({})",
        "已连接:".green().bold(),
        hello.device_name,
        hello.device_model
    );
    println!("{} {}", "序列号:".bold(), hello.serial_number);
    println!("{} {}", "充电状态:".bold(), hello.charging_status);
    println!("输入 {} 查看命令，{} 退出。", "help".cyan(), "exit".cyan());

    let mut editor = repl_editor()?;
    let history_path = history_path();
    if let Some(parent) = history_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = editor.load_history(&history_path);

    loop {
        match editor.readline("xiaomi-pb> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);

                if matches!(line, "exit" | "quit" | "q") {
                    break;
                }
                if matches!(line, "help" | "?") {
                    print_interactive_help();
                    continue;
                }

                if let Err(err) = run_interactive_command(line, debug, &mut pb) {
                    eprintln!("{} {err:#}", "错误:".red().bold());
                }
            }
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("{} {err}", "读取输入失败:".red());
                break;
            }
        }
    }

    let _ = editor.append_history(&history_path);
    let _ = block_on(pb.disconnect());
    println!("{}", "已断开连接。".green());
    Ok(())
}

fn run_interactive_command(
    line: &str,
    debug: bool,
    pb: &mut PowerBank<HidTransport>,
) -> Result<()> {
    let tokens = shlex::split(line).ok_or_else(|| anyhow::anyhow!("参数解析失败"))?;
    let args = std::iter::once("xiaomi-pb".to_owned())
        .chain(tokens)
        .collect::<Vec<_>>();
    let cli = Cli::try_parse_from(args)?;

    match cli.command {
        Some(Commands::Info) => {
            let snapshot = block_on(pb.snapshot()).context("读取设备信息失败")?;
            print_snapshot(&snapshot);
        }
        Some(Commands::Devices) => run_devices()?,
        Some(Commands::Qi2 { command }) => match command {
            Qi2Command::Status => {
                let status = block_on(pb.qi2_status()).context("查询 Qi2.2 状态失败")?;
                print_qi2_status(&status);
            }
            Qi2Command::Enable => run_interactive_qi2_set(pb, true)?,
            Qi2Command::Disable => run_interactive_qi2_set(pb, false)?,
        },
        Some(Commands::Qi2Enable) => run_interactive_qi2_set(pb, true)?,
        Some(Commands::Qi2Disable) => run_interactive_qi2_set(pb, false)?,
        Some(Commands::Raw {
            hex,
            timeout,
            verbose,
        }) => {
            let parsed = block_on(pb.raw(&hex, Duration::from_millis(timeout)))
                .context("发送原始命令失败")?;
            println!("{} 0x{:02X}", "命令:".bold(), parsed.cmd);
            println!("{} {}", "负载:".bold(), hex_upper(&parsed.payload).cyan());
            println!(
                "{} {}",
                "CRC:".bold(),
                if parsed.crc_ok {
                    "通过".green().to_string()
                } else {
                    "失败".red().to_string()
                }
            );
            if verbose {
                println!("{} {:?}", "解析:".bold(), parsed);
            }
        }
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "xiaomi-pb", &mut std::io::stdout());
        }
        None => {
            if debug {
                println!("{}", "没有命令。".yellow());
            }
        }
    }

    Ok(())
}

fn run_interactive_qi2_set(pb: &mut PowerBank<HidTransport>, enable: bool) -> Result<()> {
    let result = block_on(pb.set_qi2(enable)).context("设置 Qi2.2 失败")?;
    if result.success {
        let text = if enable {
            "Qi2.2 开启成功"
        } else {
            "Qi2.2 关闭成功"
        };
        println!("{}", text.green());
    } else {
        let text = if enable {
            "Qi2.2 开启失败"
        } else {
            "Qi2.2 关闭失败"
        };
        println!("{}", text.red());
    }
    Ok(())
}

fn print_snapshot(snapshot: &DeviceSnapshot) {
    print_hello(&snapshot.hello);
    if let Some(battery) = &snapshot.battery {
        print_battery(battery);
    }
    if let Some(qi2) = &snapshot.qi2 {
        print_qi2_status(qi2);
    }
    if let Some(cells) = &snapshot.cells {
        print_cells(cells);
    }
    if !snapshot.battery_ids.is_empty() {
        print_battery_ids(&snapshot.battery_ids);
    }
    if let Some(temp) = &snapshot.cell_temp_model {
        print_cell_temp_model(temp);
    }
}

fn print_hello(info: &HelloInfo) {
    println!(
        "{} {} ({})",
        "设备:".bold(),
        info.device_name,
        info.device_model
    );
    println!("{} {}", "型号ID:".bold(), info.model_id);
    println!("{} {}", "序列号:".bold(), info.serial_number);
    println!("{} {}", "充电状态:".bold(), info.charging_status);
    println!();
}

fn print_battery(battery: &BatteryInfo) {
    println!("{}", "电池:".bold());
    println!(
        "  状态: {}",
        status_text(battery.success, battery.status_code)
    );
    println!(
        "  激活: {}",
        match battery.activated {
            Some(true) => "已激活".green().to_string(),
            Some(false) => "未激活".yellow().to_string(),
            None => "保留".yellow().to_string(),
        }
    );
    println!("  电量: {}%", battery.level_pct);
    println!("  循环: {} 次", battery.cycle_count);
    println!("  健康度: {}", battery.health);
    println!("  温度: {}°C", battery.temperature_c);
    println!("  电压: {}mV", battery.voltage_mv);
    println!("  电流: {}mA", battery.current_ma);
    println!("  充放电: {}", battery.charge_state);
    println!();
}

fn print_qi2_status(qi2: &Qi2Status) {
    let text = if qi2.enabled {
        "已开启"
    } else {
        "未开启"
    };
    println!(
        "{} {}",
        "Qi2.2:".bold(),
        if qi2.enabled {
            text.green().to_string()
        } else {
            text.yellow().to_string()
        }
    );
    println!();
}

fn print_cells(status: &CellStatus) {
    if !status.success {
        println!(
            "{} {}",
            "电芯状态:".bold(),
            status_text(false, status.status_code)
        );
        println!();
        return;
    }

    let valid_temps = status
        .cells
        .iter()
        .filter_map(|cell| cell.temperature_c.map(|temp| (cell.index, temp)))
        .collect::<Vec<_>>();

    if !valid_temps.is_empty() {
        println!("温度点 ({} 个):", valid_temps.len());
        for (index, temp) in valid_temps {
            println!("  #{index}: {temp}°C");
        }
    }

    println!("电芯状态 ({} 节):", status.cells.len());
    for cell in &status.cells {
        println!(
            "  #{}: {}mV  {}mA",
            cell.index, cell.voltage_mv, cell.current_ma
        );
    }
    println!();
}

fn print_battery_ids(ids: &[BatteryIdInfo]) {
    println!("{}", "电芯编号信息:".bold());
    for id in ids {
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
        println!("  #{}: {}", id.cell_index, parts.join("  "));
    }
    println!();
}

fn print_cell_temp_model(temp: &CellTempModel) {
    println!("{} {}", "电芯型号:".bold(), temp.battery_model);
    println!(
        "{} {}°C ~ {}°C",
        "温度阈值:".bold(),
        temp.low_temp,
        temp.high_temp
    );
}

fn print_device(index: usize, device: &NativeDeviceInfo) {
    println!(
        "{} VID=0x{:04X} PID=0x{:04X}",
        format!("#{index}").bold(),
        device.vendor_id,
        device.product_id
    );
    if let Some(product) = &device.product_string {
        println!("  产品: {product}");
    }
    if let Some(manufacturer) = &device.manufacturer_string {
        println!("  厂商: {manufacturer}");
    }
    if let Some(serial) = &device.serial_number {
        println!("  序列号: {serial}");
    }
    println!("  路径: {}", device.path_display());
}

fn status_text(success: bool, code: u8) -> String {
    if success {
        "成功".green().to_string()
    } else {
        format!("失败({code})").red().to_string()
    }
}

fn print_interactive_help() {
    println!("{}", "可用命令:".bold());
    println!("  {:20} 显示全部信息", "info");
    println!("  {:20} 列出匹配 HID 设备", "devices");
    println!("  {:20} 查询 Qi2.2", "qi2 status");
    println!("  {:20} 开启 Qi2.2", "qi2 enable");
    println!("  {:20} 关闭 Qi2.2", "qi2 disable");
    println!("  {:20} 发送原始十六进制命令", "raw <hex>");
    println!("  {:20} 显示此帮助", "help");
    println!("  {:20} 退出交互模式", "exit");
}

fn repl_editor() -> Result<Editor<ReplHelper, DefaultHistory>> {
    let config = Config::builder()
        .history_ignore_space(true)
        .completion_type(CompletionType::List)
        .edit_mode(EditMode::Emacs)
        .build();
    let mut editor = Editor::with_config(config)?;
    editor.set_helper(Some(ReplHelper));
    Ok(editor)
}

fn history_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xiaomi-pb")
        .join("history")
}

#[derive(Debug, Clone, Copy)]
struct ReplHelper;

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let prefix_start = line[..pos]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        let prefix = &line[prefix_start..pos];
        let tokens = line[..pos].split_whitespace().collect::<Vec<_>>();
        let candidates: &[&str] = if tokens.first() == Some(&"qi2") && tokens.len() <= 2 {
            &["status", "enable", "disable"]
        } else {
            &[
                "info",
                "devices",
                "qi2",
                "qi2-enable",
                "qi2-disable",
                "raw",
                "completions",
                "help",
                "exit",
            ]
        };

        let matches = candidates
            .iter()
            .filter(|candidate| candidate.starts_with(prefix))
            .map(|candidate| Pair {
                display: (*candidate).to_owned(),
                replacement: (*candidate).to_owned(),
            })
            .collect();

        Ok((prefix_start, matches))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;

    fn hint(&self, line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        match line {
            "qi2 " => Some("status | enable | disable".to_owned()),
            "raw " => Some("A5060100D9".to_owned()),
            _ => None,
        }
    }
}

impl Highlighter for ReplHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Owned(prompt.cyan().bold().to_string())
    }

    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(command) = parts.next() else {
            return Cow::Borrowed(line);
        };
        if command.is_empty() {
            return Cow::Borrowed(line);
        }

        let rest = parts.next().unwrap_or_default();
        let highlighted = match command {
            "info" | "devices" | "qi2" | "qi2-enable" | "qi2-disable" | "raw" | "completions"
            | "help" | "exit" | "quit" | "q" => command.green().bold().to_string(),
            _ => command.red().to_string(),
        };

        if rest.is_empty() {
            Cow::Owned(highlighted)
        } else {
            Cow::Owned(format!("{highlighted} {rest}"))
        }
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(hint.bright_black().to_string())
    }
}

impl Validator for ReplHelper {}
impl Helper for ReplHelper {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qi2_enable() {
        let cli = Cli::try_parse_from(["xiaomi-pb", "qi2", "enable"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Qi2 {
                command: Qi2Command::Enable
            })
        ));
    }

    #[test]
    fn parses_legacy_qi2_enable() {
        let cli = Cli::try_parse_from(["xiaomi-pb", "qi2-enable"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Qi2Enable)));
    }

    #[test]
    fn raw_frame_size_constant_is_expected() {
        use powerbank_core::FRAME_SIZE;

        assert_eq!(FRAME_SIZE, 32);
    }
}
