use std::{env, io};
use std::process::Command;
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Clear},
};

const MAX_SSID_DISPLAY_LEN: usize = 25;

// --- Data Structures ---

#[derive(Clone, Debug)]
struct Network {
    ssid: String,
    bssid: String,
    security: String,
    signal: u8,
    in_use: bool,
}

#[derive(PartialEq)]
enum AppMode {
    Scanning,
    Browsing,
    PasswordInput,
    ActionMenu, // For connected/saved networks (Disconnect, Forget)
    Processing(String),
    Message(String), // Press any key to dismiss
}

struct App {
    mode: AppMode,
    networks: Vec<Network>,
    list_state: ListState,
    input_buffer: String,
    show_password: bool,
    wifi_interfaces: Vec<String>,
    current_interface: String,
    
    // For Action Menu
    action_items: Vec<&'static str>,
    action_state: ListState,
    
    // Target for connection
    target_ssid: String,
    target_bssid: String,
    target_security: String,
}

impl App {
    fn new(wifi_interfaces: Vec<String>, current_interface: String) -> Self {
        Self {
            mode: AppMode::Scanning,
            networks: Vec::new(),
            list_state: ListState::default(),
            input_buffer: String::new(),
            show_password: false,
            wifi_interfaces,
            current_interface,
            action_items: vec!["Disconnect", "Forget", "Cancel"],
            action_state: ListState::default(),
            target_ssid: String::new(),
            target_bssid: String::new(),
            target_security: String::new(),
        }
    }

    fn next_network(&mut self) {
        if self.networks.is_empty() {
            self.list_state.select(None);
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.networks.len() - 1 { 0 } else { i + 1 }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous_network(&mut self) {
        if self.networks.is_empty() {
            self.list_state.select(None);
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 { self.networks.len() - 1 } else { i - 1 }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }
    
    fn next_action(&mut self) {
        let i = match self.action_state.selected() {
            Some(i) => if i >= self.action_items.len() - 1 { 0 } else { i + 1 },
            None => 0,
        };
        self.action_state.select(Some(i));
    }
    
    fn previous_action(&mut self) {
        let i = match self.action_state.selected() {
            Some(i) => if i == 0 { self.action_items.len() - 1 } else { i - 1 },
            None => 0,
        };
        self.action_state.select(Some(i));
    }

    fn cycle_interface(&mut self) {
        if self.wifi_interfaces.is_empty() {
            return;
        }

        let current_idx = self
            .wifi_interfaces
            .iter()
            .position(|iface| iface == &self.current_interface)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % self.wifi_interfaces.len();
        self.current_interface = self.wifi_interfaces[next_idx].clone();
        self.list_state.select(None);
    }
}

// --- Helper Functions ---

fn run_command(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(if !stderr.is_empty() { stderr } else { stdout })
    }
}

#[derive(Default)]
struct CliOptions {
    rescan: bool,
    disconnect: bool,
    status: bool,
    interface: Option<String>,
}

fn print_usage() {
    println!("wifi_menu - TUI Wi-Fi manager");
    println!();
    println!("Usage:");
    println!("  wifi_menu [--interface <ifname>]");
    println!("  wifi_menu --rescan [--interface <ifname>]");
    println!("  wifi_menu --disconnect [--interface <ifname>]");
    println!("  wifi_menu --status [--interface <ifname>]");
    println!("  wifi_menu --help");
}

fn parse_cli_options() -> Result<CliOptions, String> {
    let mut opts = CliOptions::default();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rescan" => opts.rescan = true,
            "--disconnect" => opts.disconnect = true,
            "--status" => opts.status = true,
            "--interface" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--interface requires a value".to_string())?;
                opts.interface = Some(value);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => return Err(format!("Unknown argument: {}", arg)),
        }
    }

    let action_count = [opts.rescan, opts.disconnect, opts.status]
        .iter()
        .filter(|&&flag| flag)
        .count();
    if action_count > 1 {
        return Err("Use only one non-interactive action at a time".to_string());
    }

    Ok(opts)
}

fn get_wifi_interfaces() -> Vec<String> {
    let output = match run_command("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE", "device", "status"]) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let mut interfaces = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 2 {
            continue;
        }
        if parts[1] == "wifi" && !parts[0].is_empty() {
            interfaces.push(parts[0].to_string());
        }
    }
    interfaces
}

fn pick_default_interface(interfaces: &[String]) -> Option<String> {
    if interfaces.is_empty() {
        return None;
    }

    let output = run_command("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE", "device", "status"]).ok()?;
    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        if parts[1] == "wifi"
            && (parts[2].eq_ignore_ascii_case("connected")
                || parts[2].eq_ignore_ascii_case("connecting"))
        {
            return Some(parts[0].to_string());
        }
    }

    Some(interfaces[0].clone())
}

fn run_status(interface: Option<&str>) -> Result<String, String> {
    let output = run_command("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"])?;
    let mut rows = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 4 || parts[1] != "wifi" {
            continue;
        }
        if let Some(iface) = interface {
            if parts[0] != iface {
                continue;
            }
        }
        rows.push(format!(
            "interface={} state={} connection={}",
            parts[0], parts[2], parts[3]
        ));
    }

    if rows.is_empty() {
        if let Some(iface) = interface {
            return Ok(format!("interface={} state=unavailable connection=--", iface));
        }
        return Ok("interface=none state=unavailable connection=--".to_string());
    }

    Ok(rows.join("\n"))
}

fn get_networks(interface: &str) -> Vec<Network> {
    // Format: IN-USE:SSID:BSSID:SECURITY:SIGNAL
    let output = match run_command(
        "nmcli",
        &[
            "-t",
            "-f",
            "IN-USE,SSID,BSSID,SECURITY,SIGNAL",
            "dev",
            "wifi",
            "list",
            "ifname",
            interface,
        ],
    ) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let mut networks = Vec::new();
    let mut seen_ssids = Vec::new();

    for line in output.lines() {
        let safe_line = line.replace("\\:", "\u{0000}");
        let parts: Vec<&str> = safe_line.split(':').collect();
        if parts.len() < 5 { continue; }

        let in_use = parts[0] == "*";
        // Do not trim SSID; significant whitespace might exist
        let ssid = parts[1].replace("\u{0000}", ":").to_string();
        let bssid = parts[2].replace("\u{0000}", ":");
        let security = parts[3].replace("\u{0000}", ":");
        let signal: u8 = parts[4].parse().unwrap_or(0);

        if ssid.is_empty() { continue; }
        
        // Deduplicate by SSID, preferring the connected one or stronger signal
        if seen_ssids.contains(&ssid) && !in_use { 
            continue; 
        }
        seen_ssids.push(ssid.clone());

        networks.push(Network {
            ssid,
            bssid,
            security,
            signal,
            in_use,
        });
    }
    // Sort: Connected first, then Signal strength
    networks.sort_by(|a, b| {
        if a.in_use != b.in_use {
            b.in_use.cmp(&a.in_use)
        } else {
            b.signal.cmp(&a.signal)
        }
    });
    networks
}

fn connect_network(
    ssid: &str,
    bssid: &str,
    password: &str,
    security: &str,
    interface: &str,
) -> Result<String, String> {
    // Strategy: Use 'dev wifi connect' with BSSID for precision, but name the profile with SSID.
    
    // 1. Delete existing profile to avoid conflicts (e.g. stale key-mgmt settings)
    let _ = run_command("nmcli", &["connection", "delete", ssid]);

    let mut args = vec!["dev", "wifi", "connect", ssid, "ifname", interface];
    if !bssid.is_empty() {
        args.push("bssid");
        args.push(bssid);
    }
    args.push("name");
    args.push(ssid);
    
    // Only add password argument if the network is secured
    if security.contains("WPA") || security.contains("RSN") || security.contains("WEP") {
        args.push("password");
        args.push(password);
    }

    run_command("nmcli", &args)
}

fn delete_connection(ssid: &str) -> Result<String, String> {
    // Try to find the connection name. Usually same as SSID or "SSID 1"
    // Simple approach: delete by SSID, nmcli usually handles it.
    // Better approach: Find active connection on interface.
    // For this TUI, let's just try deleting the SSID profile.
    run_command("nmcli", &["connection", "delete", ssid])
}

fn disconnect_interface(interface: &str) -> Result<String, String> {
    run_command("nmcli", &["dev", "disconnect", interface])
}

fn rescan_interface(interface: &str) -> Result<String, String> {
    run_command("nmcli", &["dev", "wifi", "rescan", "ifname", interface])
}


// --- UI Rendering ---

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    // Network List
    let items: Vec<ListItem> = app.networks.iter().map(|n| {
        let lock = if n.security.is_empty() { " " } else { "" };
        let signal_icon = match n.signal {
            0..=20 => "󰤯",
            21..=40 => "󰤟",
            41..=60 => "󰤢",
            61..=80 => "󰤥",
            _ => "󰤨",
        };
        let active = if n.in_use { " " } else { "  " };
        
        let display_ssid = if n.ssid.len() > MAX_SSID_DISPLAY_LEN && MAX_SSID_DISPLAY_LEN > 3 {
            format!("{:.width$}...", &n.ssid[..MAX_SSID_DISPLAY_LEN - 3], width = MAX_SSID_DISPLAY_LEN - 3)
        } else {
            // Pad to MAX_SSID_DISPLAY_LEN if not truncated to maintain column width
            let mut s = n.ssid.clone();
            s.truncate(MAX_SSID_DISPLAY_LEN); // Ensure it doesn't exceed if it was just slightly longer than display_len
            s
        };
        // Use `MAX_SSID_DISPLAY_LEN` for formatting width
        let content = format!("{} {} {:<width$} {:>3}% {}", active, signal_icon, display_ssid, n.signal, lock, width = MAX_SSID_DISPLAY_LEN);
        let style = if n.in_use { 
            Style::default().fg(Color::Green)
        } else { 
            Style::default() 
        };
        ListItem::new(content).style(style)
    }).collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Wi-Fi Networks ({}) ", app.current_interface)),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));

    f.render_stateful_widget(list, chunks[0], &mut app.list_state.clone());

    // Status Bar
    let status_style = match app.mode {
        AppMode::Processing(_) => Style::default().bg(Color::Yellow).fg(Color::Black),
        AppMode::Message(_) => Style::default().bg(Color::Blue),
        _ => Style::default().bg(Color::White).fg(Color::Black),
    };
    let status_text = match &app.mode {
        AppMode::Processing(msg) => format!(" 󰑐 {} ", msg),
        AppMode::Message(msg) => format!(" 󰋗 {} (Press Any Key)", msg),
        AppMode::PasswordInput => " Enter Password | Tab: Show/Hide | Esc: Cancel ".to_string(),
        AppMode::ActionMenu => " Select Action ".to_string(),
        AppMode::Browsing => {
            if let Some(idx) = app.list_state.selected() {
                let selected_net = &app.networks[idx];
                if selected_net.ssid.len() > MAX_SSID_DISPLAY_LEN {
                    format!(
                        " IF:{} | Full SSID: {} | i: Switch IF | r: Rescan | Enter: Connect | q: Quit ",
                        app.current_interface, selected_net.ssid
                    )
                } else {
                    format!(
                        " IF:{} | i: Switch IF | r: Rescan | Enter: Connect | q: Quit ",
                        app.current_interface
                    )
                }
            } else {
                format!(
                    " IF:{} | i: Switch IF | r: Rescan | Enter: Connect | q: Quit ",
                    app.current_interface
                )
            }
        }
        _ => format!(
            " IF:{} | i: Switch IF | r: Rescan | Enter: Connect | q: Quit ",
            app.current_interface
        ),
    };
    let status_bar = Paragraph::new(status_text).style(status_style);
    f.render_widget(status_bar, chunks[1]);

    // Popups
    if app.mode == AppMode::PasswordInput {
        let area = centered_rect(60, 20, f.area());
        f.render_widget(Clear, area); // Clear background

        let password_display = if app.show_password {
            app.input_buffer.clone()
        } else {
            "*".repeat(app.input_buffer.len())
        };

        let input = Paragraph::new(format!("Password: {}\n\n(Tab to Show/Hide, Enter to Connect)", password_display))
            .block(Block::default().borders(Borders::ALL).title(format!(" Connect to {} ", app.target_ssid)))
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(input, area);
    }
    
    if app.mode == AppMode::ActionMenu {
        let area = centered_rect(40, 25, f.area());
        f.render_widget(Clear, area);
        
        let items: Vec<ListItem> = app.action_items.iter().map(|i| ListItem::new(*i)).collect();
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(format!(" {} ", app.target_ssid)))
            .highlight_style(Style::default().bg(Color::Red).fg(Color::White));
        
        f.render_stateful_widget(list, area, &mut app.action_state.clone());
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}


// --- Main ---

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = match parse_cli_options() {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{}", e);
            print_usage();
            std::process::exit(2);
        }
    };

    let interfaces = get_wifi_interfaces();

    if cli.status {
        match run_status(cli.interface.as_deref()) {
            Ok(out) => {
                println!("{}", out);
                return Ok(());
            }
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    }

    let selected_interface = if let Some(iface) = cli.interface.clone() {
        if interfaces.iter().any(|candidate| candidate == &iface) {
            iface
        } else {
            eprintln!("Interface '{}' not found among Wi-Fi devices.", iface);
            std::process::exit(1);
        }
    } else {
        match pick_default_interface(&interfaces) {
            Some(iface) => iface,
            None => {
                eprintln!("No Wi-Fi interface found.");
                std::process::exit(1);
            }
        }
    };

    if cli.rescan {
        match rescan_interface(&selected_interface) {
            Ok(out) => {
                if !out.is_empty() {
                    println!("{}", out);
                } else {
                    println!("rescan=ok interface={}", selected_interface);
                }
                return Ok(());
            }
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    }

    if cli.disconnect {
        match disconnect_interface(&selected_interface) {
            Ok(out) => {
                if !out.is_empty() {
                    println!("{}", out);
                } else {
                    println!("disconnect=ok interface={}", selected_interface);
                }
                return Ok(());
            }
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    }

    // Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(interfaces, selected_interface);
    
    // Initial Scan
    app.mode = AppMode::Processing("Scanning...".to_string());
    terminal.draw(|f| ui(f, &app))?;
    app.networks = get_networks(&app.current_interface);
    if !app.networks.is_empty() { app.list_state.select(Some(0)); }
    app.mode = AppMode::Browsing;

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Special Handling for Processing State (non-event driven updates if needed, mostly blocking for now)
        if let AppMode::Processing(_msg) = &app.mode {
            // In a real async app, we'd wait for channel messages.
            // Here we handle blocking actions triggered by previous state transitions immediately.
            // But since we are in the loop, we need to detect *what* to do.
            // Actually, simpler: Set processing, Draw, then Do work, then Set Browsing.
            // We can't do that easily inside the loop without a re-draw.
            // Current approach: we set Processing, loop continues, Draw happens.
            // We need a "tick" to actually do the work? 
            // Let's just do blocking actions in the Key handler for now, manually calling draw.
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press { continue; }

                match app.mode {
                    AppMode::Browsing => {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Down | KeyCode::Char('j') => app.next_network(),
                            KeyCode::Up | KeyCode::Char('k') => app.previous_network(),
                            KeyCode::Char('r') => {
                                app.mode = AppMode::Processing("Scanning...".to_string());
                                terminal.draw(|f| ui(f, &app))?;
                                let _ = rescan_interface(&app.current_interface);
                                app.networks = get_networks(&app.current_interface);
                                if !app.networks.is_empty() {
                                    app.list_state.select(Some(0));
                                }
                                app.mode = AppMode::Browsing;
                            }
                            KeyCode::Char('i') => {
                                app.cycle_interface();
                                app.mode = AppMode::Processing(format!(
                                    "Switching to {}...",
                                    app.current_interface
                                ));
                                terminal.draw(|f| ui(f, &app))?;
                                app.networks = get_networks(&app.current_interface);
                                if !app.networks.is_empty() {
                                    app.list_state.select(Some(0));
                                }
                                app.mode = AppMode::Browsing;
                            }
                            KeyCode::Enter => {
                                if let Some(idx) = app.list_state.selected() {
                                    let net = app.networks[idx].clone();
                                    app.target_ssid = net.ssid.clone();
                                    app.target_bssid = net.bssid.clone();
                                    app.target_security = net.security.clone();

                                    if net.in_use {
                                        app.mode = AppMode::ActionMenu;
                                        app.action_state.select(Some(0));
                                    } else {
                                        // Try connecting
                                        app.mode = AppMode::Processing(format!("Connecting to {}...", net.ssid));
                                        terminal.draw(|f| ui(f, &app))?;
                                        
                                        // Try passwordless/saved first
                                        let res = connect_network(
                                            &net.ssid,
                                            &net.bssid,
                                            "",
                                            &net.security,
                                            &app.current_interface,
                                        );
                                        match res {
                                            Ok(_) => {
                                                app.mode = AppMode::Message(format!("Connected to {}", net.ssid));
                                                app.networks = get_networks(&app.current_interface); // Refresh status
                                            },
                                            Err(_) => {
                                                // Failed. Need password?
                                                if !net.security.is_empty() {
                                                    app.mode = AppMode::PasswordInput;
                                                    app.input_buffer.clear();
                                                    app.show_password = false;
                                                } else {
                                                    app.mode = AppMode::Message(format!("Failed to connect to {}", net.ssid));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {} 
                        }
                    }
                    AppMode::PasswordInput => {
                        match key.code {
                            KeyCode::Esc => app.mode = AppMode::Browsing,
                            KeyCode::Tab => app.show_password = !app.show_password,
                            KeyCode::Enter => {
                                app.mode = AppMode::Processing("Verifying Password...".to_string());
                                terminal.draw(|f| ui(f, &app))?;
                                
                                let res = connect_network(
                                    &app.target_ssid,
                                    &app.target_bssid,
                                    &app.input_buffer,
                                    &app.target_security,
                                    &app.current_interface,
                                );
                                match res {
                                    Ok(_) => {
                                        app.mode = AppMode::Message("Success!".to_string());
                                        app.networks = get_networks(&app.current_interface);
                                    },
                                    Err(e) => app.mode = AppMode::Message(format!("Error: {}", e)),
                                }
                            }
                            KeyCode::Backspace => { app.input_buffer.pop(); }
                            KeyCode::Char(c) => { app.input_buffer.push(c); }
                            _ => {} 
                        }
                    }
                    AppMode::ActionMenu => {
                         match key.code {
                            KeyCode::Esc => app.mode = AppMode::Browsing,
                            KeyCode::Up | KeyCode::Char('k') => app.previous_action(),
                            KeyCode::Down | KeyCode::Char('j') => app.next_action(),
                            KeyCode::Enter => {
                                if let Some(idx) = app.action_state.selected() {
                                    let action = app.action_items[idx];
                                    match action {
                                        "Disconnect" => {
                                            let _ = disconnect_interface(&app.current_interface);
                                            app.mode = AppMode::Message("Disconnected".to_string());
                                            app.networks = get_networks(&app.current_interface);
                                        },
                                        "Forget" => {
                                            let _ = delete_connection(&app.target_ssid);
                                            app.mode = AppMode::Message("Network Forgotten".to_string());
                                            app.networks = get_networks(&app.current_interface);
                                        },
                                        _ => app.mode = AppMode::Browsing,
                                    }
                                }
                            }
                             _ => {} 
                         }
                    }
                    AppMode::Message(_) => {
                        // Any key returns to browsing
                        app.mode = AppMode::Browsing;
                    }
                    _ => {} 
                }
            }
        }
    }

    // Restore Terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
