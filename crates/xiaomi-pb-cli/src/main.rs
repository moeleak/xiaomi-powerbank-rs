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
#[command(name = "xiaomi-pb", version, about = "Xiaomi power bank USB HID tool")]
struct Cli {
    #[arg(long, global = true, help = "Show HID traffic logs")]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Show full device information")]
    Info,
    #[command(about = "List matching HID devices")]
    Devices,
    #[command(about = "Query or change Qi2.2 status")]
    Qi2 {
        #[command(subcommand)]
        command: Qi2Command,
    },
    #[command(about = "Send a raw hexadecimal HID frame")]
    Raw {
        #[arg(help = "Hex data, for example A5060100D9")]
        hex: String,
        #[arg(
            long,
            short,
            default_value_t = 3_000,
            help = "Response timeout in milliseconds"
        )]
        timeout: u64,
        #[arg(long, short, help = "Show full parsed response")]
        verbose: bool,
    },
    #[command(about = "Generate shell completion script")]
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum Qi2Command {
    #[command(about = "Query Qi2.2 status")]
    Status,
    #[command(about = "Enable Qi2.2")]
    Enable,
    #[command(about = "Disable Qi2.2")]
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
    let snapshot = block_on(pb.snapshot()).context("failed to read device information")?;
    print_snapshot(&snapshot);
    let _ = block_on(pb.disconnect());
    Ok(())
}

fn run_devices() -> Result<()> {
    let devices = HidTransport::list().context("failed to enumerate HID devices")?;
    if devices.is_empty() {
        println!(
            "{}",
            "No matching Xiaomi power bank HID devices found".yellow()
        );
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
    let status = block_on(pb.qi2_status()).context("failed to query Qi2.2 status")?;
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
            "Qi2.2 is already enabled"
        } else {
            "Qi2.2 is already disabled"
        };
        println!("{}", text.yellow());
        let _ = block_on(pb.disconnect());
        return Ok(());
    }

    let result = block_on(pb.set_qi2(enable)).context("failed to set Qi2.2 status")?;
    if result.success {
        let text = if enable {
            "Qi2.2 enabled"
        } else {
            "Qi2.2 disabled"
        };
        println!("{}", text.green());
    } else {
        let text = if enable {
            "Failed to enable Qi2.2"
        } else {
            "Failed to disable Qi2.2"
        };
        println!("{}", text.red());
    }
    let _ = block_on(pb.disconnect());
    Ok(())
}

fn run_raw(debug: bool, hex: &str, timeout: u64, verbose: bool) -> Result<()> {
    let mut pb = connect_powerbank(debug)?;
    let _ = block_on(pb.handshake());
    let parsed = block_on(pb.raw(hex, Duration::from_millis(timeout)))
        .context("failed to send raw command")?;
    println!("{} 0x{:02X}", "Command:".bold(), parsed.cmd);
    println!(
        "{} {}",
        "Payload:".bold(),
        hex_upper(&parsed.payload).cyan()
    );
    println!(
        "{} {}",
        "CRC:".bold(),
        if parsed.crc_ok {
            "ok".green().to_string()
        } else {
            format!(
                "failed expected=0x{:02X} received=0x{:02X}",
                parsed.crc_expected, parsed.crc_received
            )
            .red()
            .to_string()
        }
    );

    if verbose {
        println!("{} {:?}", "Parsed:".bold(), parsed);
    }

    let _ = block_on(pb.disconnect());
    Ok(())
}

fn connect_powerbank(debug: bool) -> Result<PowerBank<HidTransport>> {
    eprint!("{}", "Looking for a Xiaomi power bank HID device".cyan());
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
    let hello = block_on(pb.handshake()).context("handshake failed")?;
    println!(
        "{} {} ({})",
        "Connected:".green().bold(),
        hello.device_name,
        hello.device_model
    );
    println!("{} {}", "Serial:".bold(), hello.serial_number);
    println!("{} {}", "Power state:".bold(), hello.charging_status);
    println!(
        "Type {} for commands, {} to quit.",
        "help".cyan(),
        "exit".cyan()
    );

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
                    eprintln!("{} {err:#}", "Error:".red().bold());
                }
            }
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("{} {err}", "Failed to read input:".red());
                break;
            }
        }
    }

    let _ = editor.append_history(&history_path);
    let _ = block_on(pb.disconnect());
    println!("{}", "Disconnected.".green());
    Ok(())
}

fn run_interactive_command(
    line: &str,
    debug: bool,
    pb: &mut PowerBank<HidTransport>,
) -> Result<()> {
    let tokens = shlex::split(line).ok_or_else(|| anyhow::anyhow!("failed to parse arguments"))?;
    let args = std::iter::once("xiaomi-pb".to_owned())
        .chain(tokens)
        .collect::<Vec<_>>();
    let cli = Cli::try_parse_from(args)?;

    match cli.command {
        Some(Commands::Info) => {
            let snapshot = block_on(pb.snapshot()).context("failed to read device information")?;
            print_snapshot(&snapshot);
        }
        Some(Commands::Devices) => run_devices()?,
        Some(Commands::Qi2 { command }) => match command {
            Qi2Command::Status => {
                let status = block_on(pb.qi2_status()).context("failed to query Qi2.2 status")?;
                print_qi2_status(&status);
            }
            Qi2Command::Enable => run_interactive_qi2_set(pb, true)?,
            Qi2Command::Disable => run_interactive_qi2_set(pb, false)?,
        },
        Some(Commands::Raw {
            hex,
            timeout,
            verbose,
        }) => {
            let parsed = block_on(pb.raw(&hex, Duration::from_millis(timeout)))
                .context("failed to send raw command")?;
            println!("{} 0x{:02X}", "Command:".bold(), parsed.cmd);
            println!(
                "{} {}",
                "Payload:".bold(),
                hex_upper(&parsed.payload).cyan()
            );
            println!(
                "{} {}",
                "CRC:".bold(),
                if parsed.crc_ok {
                    "ok".green().to_string()
                } else {
                    "failed".red().to_string()
                }
            );
            if verbose {
                println!("{} {:?}", "Parsed:".bold(), parsed);
            }
        }
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "xiaomi-pb", &mut std::io::stdout());
        }
        None => {
            if debug {
                println!("{}", "No command provided.".yellow());
            }
        }
    }

    Ok(())
}

fn run_interactive_qi2_set(pb: &mut PowerBank<HidTransport>, enable: bool) -> Result<()> {
    let result = block_on(pb.set_qi2(enable)).context("failed to set Qi2.2 status")?;
    if result.success {
        let text = if enable {
            "Qi2.2 enabled"
        } else {
            "Qi2.2 disabled"
        };
        println!("{}", text.green());
    } else {
        let text = if enable {
            "Failed to enable Qi2.2"
        } else {
            "Failed to disable Qi2.2"
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
        "Device:".bold(),
        info.device_name,
        info.device_model
    );
    println!("{} {}", "Model ID:".bold(), info.model_id);
    println!("{} {}", "Serial:".bold(), info.serial_number);
    println!("{} {}", "Power state:".bold(), info.charging_status);
    println!();
}

fn print_battery(battery: &BatteryInfo) {
    println!("{}", "Battery:".bold());
    println!(
        "  Status: {}",
        status_text(battery.success, battery.status_code)
    );
    println!(
        "  Activation: {}",
        match battery.activated {
            Some(true) => "activated".green().to_string(),
            Some(false) => "not activated".yellow().to_string(),
            None => "reserved".yellow().to_string(),
        }
    );
    println!("  Level: {}%", battery.level_pct);
    println!("  Cycles: {}", battery.cycle_count);
    println!("  Health: {}", battery.health);
    println!("  Temperature: {}°C", battery.temperature_c);
    println!("  Voltage: {}mV", battery.voltage_mv);
    println!("  Current: {}mA", battery.current_ma);
    println!("  Charge state: {}", battery.charge_state);
    println!();
}

fn print_qi2_status(qi2: &Qi2Status) {
    let text = if qi2.enabled { "enabled" } else { "disabled" };
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
            "Cell status:".bold(),
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
        println!("Temperature points ({}):", valid_temps.len());
        for (index, temp) in valid_temps {
            println!("  #{index}: {temp}°C");
        }
    }

    println!("Cell status ({} cells):", status.cells.len());
    for cell in &status.cells {
        println!(
            "  #{}: {}mV  {}mA",
            cell.index, cell.voltage_mv, cell.current_ma
        );
    }
    println!();
}

fn print_battery_ids(ids: &[BatteryIdInfo]) {
    println!("{}", "Cell ID information:".bold());
    for id in ids {
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
        println!("  #{}: {}", id.cell_index, parts.join("  "));
    }
    println!();
}

fn print_cell_temp_model(temp: &CellTempModel) {
    println!("{} {}", "Cell model:".bold(), temp.battery_model);
    println!(
        "{} {}°C ~ {}°C",
        "Temperature range:".bold(),
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
        println!("  Product: {product}");
    }
    if let Some(manufacturer) = &device.manufacturer_string {
        println!("  Manufacturer: {manufacturer}");
    }
    if let Some(serial) = &device.serial_number {
        println!("  Serial: {serial}");
    }
    println!("  Path: {}", device.path_display());
}

fn status_text(success: bool, code: u8) -> String {
    if success {
        "ok".green().to_string()
    } else {
        format!("failed({code})").red().to_string()
    }
}

fn print_interactive_help() {
    println!("{}", "Available commands:".bold());
    println!("  {:20} Show full device information", "info");
    println!("  {:20} List matching HID devices", "devices");
    println!("  {:20} Query Qi2.2", "qi2 status");
    println!("  {:20} Enable Qi2.2", "qi2 enable");
    println!("  {:20} Disable Qi2.2", "qi2 disable");
    println!("  {:20} Send a raw hexadecimal command", "raw <hex>");
    println!("  {:20} Show this help", "help");
    println!("  {:20} Exit interactive mode", "exit");
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
            "info" | "devices" | "qi2" | "raw" | "completions" | "help" | "exit" | "quit" | "q" => {
                command.green().bold().to_string()
            }
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
    fn raw_frame_size_constant_is_expected() {
        use powerbank_core::FRAME_SIZE;

        assert_eq!(FRAME_SIZE, 32);
    }
}
