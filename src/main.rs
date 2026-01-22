use std::io;
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
    
    // For Action Menu
    action_items: Vec<&'static str>,
    action_state: ListState,
    
    // Target for connection
    target_ssid: String,
    target_bssid: String,
    target_security: String,
}

impl App {
    fn new() -> Self {
        Self {
            mode: AppMode::Scanning,
            networks: Vec::new(),
            list_state: ListState::default(),
            input_buffer: String::new(),
            action_items: vec!["Disconnect", "Forget", "Cancel"],
            action_state: ListState::default(),
            target_ssid: String::new(),
            target_bssid: String::new(),
            target_security: String::new(),
        }
    }

    fn next_network(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.networks.len() - 1 { 0 } else { i + 1 }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous_network(&mut self) {
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

fn get_networks() -> Vec<Network> {
    // Format: IN-USE:SSID:BSSID:SECURITY:SIGNAL
    let output = match run_command("nmcli", &["-t", "-f", "IN-USE,SSID,BSSID,SECURITY,SIGNAL", "dev", "wifi", "list"]) {
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

fn connect_network(ssid: &str, bssid: &str, password: &str, security: &str) -> Result<String, String> {
    // Strategy: Use 'dev wifi connect' with BSSID for precision, but name the profile with SSID.
    
    // 1. Delete existing profile to avoid conflicts (e.g. stale key-mgmt settings)
    let _ = run_command("nmcli", &["connection", "delete", ssid]);

    let mut args = vec!["dev", "wifi", "connect"];
    
    if !bssid.is_empty() {
        args.push(bssid);
        args.push("name");
        args.push(ssid);
    } else {
        args.push(ssid);
    }
    
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

fn disconnect_interface() -> Result<String, String> {
    run_command("nmcli", &["dev", "disconnect", "wlan0"]) // Assuming wlan0, could detect
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
        .block(Block::default().borders(Borders::ALL).title(" Wi-Fi Networks "))
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
        AppMode::PasswordInput => " Enter Password | Esc to Cancel ".to_string(),
        AppMode::ActionMenu => " Select Action ".to_string(),
        AppMode::Browsing => {
            if let Some(idx) = app.list_state.selected() {
                let selected_net = &app.networks[idx];
                if selected_net.ssid.len() > MAX_SSID_DISPLAY_LEN {
                    format!(" Full SSID: {} | r: Rescan | Enter: Connect | q: Quit ", selected_net.ssid)
                } else {
                    " r: Rescan | Enter: Connect | q: Quit ".to_string()
                }
            } else {
                " r: Rescan | Enter: Connect | q: Quit ".to_string()
            }
        }
        _ => " r: Rescan | Enter: Connect | q: Quit ".to_string(),
    };
    let status_bar = Paragraph::new(status_text).style(status_style);
    f.render_widget(status_bar, chunks[1]);

    // Popups
    if app.mode == AppMode::PasswordInput {
        let area = centered_rect(60, 20, f.area());
        f.render_widget(Clear, area); // Clear background
        
        let input = Paragraph::new(format!("Password: {}\n\n(Press Enter to Connect)", "*".repeat(app.input_buffer.len())))
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
    // Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    
    // Initial Scan
    app.mode = AppMode::Processing("Scanning...".to_string());
    terminal.draw(|f| ui(f, &app))?;
    app.networks = get_networks();
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
                                let _ = run_command("nmcli", &["dev", "wifi", "rescan"]);
                                app.networks = get_networks();
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
                                        let res = connect_network(&net.ssid, &net.bssid, "", &net.security);
                                        match res {
                                            Ok(_) => {
                                                app.mode = AppMode::Message(format!("Connected to {}", net.ssid));
                                                app.networks = get_networks(); // Refresh status
                                            },
                                            Err(_) => {
                                                // Failed. Need password?
                                                if !net.security.is_empty() {
                                                    app.mode = AppMode::PasswordInput;
                                                    app.input_buffer.clear();
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
                            KeyCode::Enter => {
                                app.mode = AppMode::Processing("Verifying Password...".to_string());
                                terminal.draw(|f| ui(f, &app))?;
                                
                                let res = connect_network(&app.target_ssid, &app.target_bssid, &app.input_buffer, &app.target_security);
                                match res {
                                    Ok(_) => {
                                        app.mode = AppMode::Message("Success!".to_string());
                                        app.networks = get_networks();
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
                                            let _ = disconnect_interface();
                                            app.mode = AppMode::Message("Disconnected".to_string());
                                            app.networks = get_networks();
                                        },
                                        "Forget" => {
                                            let _ = delete_connection(&app.target_ssid);
                                            app.mode = AppMode::Message("Network Forgotten".to_string());
                                            app.networks = get_networks();
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
