use std::io::stdout;
use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyEventKind, EnableMouseCapture, DisableMouseCapture, EnableBracketedPaste, DisableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::info;

use ytm_rs::app::App;
use ytm_rs::audio::daemon::{ensure_daemon_running, send_daemon_command, AudioDaemon};
use ytm_rs::audio::{DaemonCommand, DaemonResponse};
use ytm_rs::config::AppConfig;
use ytm_rs::innertube::auth::{extract_sapisid_from_cookies, AuthConfig};
use ytm_rs::theme::{Theme, ThemeWatcher};
use ytm_rs::topbar::waybar::read_waybar_status;
use ytm_rs::ui;

#[derive(Parser)]
#[command(name = "ytm-rs")]
#[command(about = "YouTube Music TUI and background daemon for Omarchy", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Inicia o daemon de áudio em segundo plano
    Daemon,
    /// Envia comandos rápidos de controle ao daemon em execução (< 2ms)
    Ctl {
        #[command(subcommand)]
        action: CtlAction,
    },
    /// Foca na janela existente do YTM-RS ou abre uma nova no terminal apropriado
    Launch,
    /// Gerencia autenticação no YouTube Music (cookies e conta)
    Auth {
        #[command(subcommand)]
        action: Option<AuthAction>,
    },
}

#[derive(Subcommand)]
enum CtlAction {
    /// Foca na janela existente do YTM-RS ou abre uma nova se não existir
    Focus,
    /// Alterna reprodução / pausa
    Toggle,
    /// Inicia reprodução
    Play,
    /// Pausa reprodução
    Pause,
    /// Para a reprodução
    Stop,
    /// Avança para a próxima faixa
    Next,
    /// Volta para a faixa anterior
    Prev,
    /// Aumenta o volume (+5%)
    VolUp,
    /// Diminui o volume (-5%)
    VolDown,
    /// Define o volume exato (0-100)
    Volume { level: u8 },
    /// Favorita / Desfavorita a faixa atual (Like)
    Like,
    /// Exibe o estado atual em JSON
    Status,
    /// Emite JSON formatado para a Waybar / Top Bar do Omarchy
    Waybar,
}

#[derive(Subcommand)]
enum AuthAction {
    /// Exibe o status da autenticação atual
    Status,
    /// Salva cookies do YouTube Music
    Cookies { value: String },
    /// Remove credenciais de autenticação
    Logout,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::load()?;
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon) => run_daemon(config).await,
        Some(Commands::Ctl { action }) => run_ctl(config, action).await,
        Some(Commands::Launch) => {
            focus_or_launch_window();
            Ok(())
        }
        Some(Commands::Auth { action }) => run_auth(config, action).await,
        None => run_tui(config).await,
    }
}

async fn run_daemon(config: AppConfig) -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("Starting ytm-rs audio daemon...");
    let (daemon, mpv_events) = AudioDaemon::new(config).await?;
    let daemon = Arc::new(daemon);
    daemon.run(mpv_events).await
}

fn which(cmd: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = candidate.metadata() {
                        if meta.permissions().mode() & 0o111 != 0 {
                            return true;
                        }
                    }
                }
                #[cfg(not(unix))]
                return true;
            }
        }
    }
    false
}

struct TerminalInvocation {
    cmd: String,
    args: Vec<String>,
}

fn detect_terminal_launcher() -> TerminalInvocation {
    // 1. Prioritize Foot (standard terminal in Omarchy).
    // Uses --app-id=ytm-rs so Hyprland applies floating window rules (^ytm-rs$)
    // and Sixel graphics for album art.
    if which("foot") {
        return TerminalInvocation {
            cmd: "foot".to_string(),
            args: vec![
                "--app-id=ytm-rs".to_string(),
                "--title=YTM-RS".to_string(),
                "-e".to_string(),
                "ytm-rs".to_string(),
            ],
        };
    }

    // 2. If Foot is not installed, check if user explicitly requested a terminal via YTM_TERMINAL or TERMINAL
    let user_terminal = std::env::var("YTM_TERMINAL")
        .ok()
        .or_else(|| {
            std::env::var("TERMINAL")
                .ok()
                .filter(|t| t.trim() != "xdg-terminal-exec" && !t.trim().is_empty())
        });

    if let Some(term) = user_terminal {
        let term_trimmed = term.trim();
        if which(term_trimmed) {
            let lower = term_trimmed.to_lowercase();
            if lower.contains("foot") {
                return TerminalInvocation {
                    cmd: term_trimmed.to_string(),
                    args: vec![
                        "--app-id=ytm-rs".to_string(),
                        "--title=YTM-RS".to_string(),
                        "-e".to_string(),
                        "ytm-rs".to_string(),
                    ],
                };
            } else if lower.contains("kitty") {
                return TerminalInvocation {
                    cmd: term_trimmed.to_string(),
                    args: vec![
                        "--class=ytm-rs".to_string(),
                        "--title=YTM-RS".to_string(),
                        "-e".to_string(),
                        "ytm-rs".to_string(),
                    ],
                };
            } else if lower.contains("ghostty") {
                return TerminalInvocation {
                    cmd: term_trimmed.to_string(),
                    args: vec![
                        "--class=ytm-rs".to_string(),
                        "--title=YTM-RS".to_string(),
                        "-e".to_string(),
                        "ytm-rs".to_string(),
                    ],
                };
            } else if lower.contains("alacritty") {
                return TerminalInvocation {
                    cmd: term_trimmed.to_string(),
                    args: vec![
                        "--class".to_string(),
                        "ytm-rs,ytm-rs".to_string(),
                        "--title".to_string(),
                        "YTM-RS".to_string(),
                        "-e".to_string(),
                        "ytm-rs".to_string(),
                    ],
                };
            } else if lower.contains("wezterm") {
                return TerminalInvocation {
                    cmd: term_trimmed.to_string(),
                    args: vec![
                        "start".to_string(),
                        "--class".to_string(),
                        "ytm-rs".to_string(),
                        "ytm-rs".to_string(),
                    ],
                };
            } else {
                return TerminalInvocation {
                    cmd: term_trimmed.to_string(),
                    args: vec!["-e".to_string(), "ytm-rs".to_string()],
                };
            }
        }
    }

    // 3. Fallbacks for other popular terminals when outside Omarchy
    if which("kitty") {
        return TerminalInvocation {
            cmd: "kitty".to_string(),
            args: vec![
                "--class=ytm-rs".to_string(),
                "--title=YTM-RS".to_string(),
                "-e".to_string(),
                "ytm-rs".to_string(),
            ],
        };
    }

    if which("ghostty") {
        return TerminalInvocation {
            cmd: "ghostty".to_string(),
            args: vec![
                "--class=ytm-rs".to_string(),
                "--title=YTM-RS".to_string(),
                "-e".to_string(),
                "ytm-rs".to_string(),
            ],
        };
    }

    if which("alacritty") {
        return TerminalInvocation {
            cmd: "alacritty".to_string(),
            args: vec![
                "--class".to_string(),
                "ytm-rs,ytm-rs".to_string(),
                "--title".to_string(),
                "YTM-RS".to_string(),
                "-e".to_string(),
                "ytm-rs".to_string(),
            ],
        };
    }

    if which("wezterm") {
        return TerminalInvocation {
            cmd: "wezterm".to_string(),
            args: vec![
                "start".to_string(),
                "--class".to_string(),
                "ytm-rs".to_string(),
                "ytm-rs".to_string(),
            ],
        };
    }

    if which("xdg-terminal-exec") {
        return TerminalInvocation {
            cmd: "xdg-terminal-exec".to_string(),
            args: vec!["-e".to_string(), "ytm-rs".to_string()],
        };
    }

    if which("x-terminal-emulator") {
        return TerminalInvocation {
            cmd: "x-terminal-emulator".to_string(),
            args: vec!["-e".to_string(), "ytm-rs".to_string()],
        };
    }

    // 4. Ultimate fallback
    TerminalInvocation {
        cmd: "xterm".to_string(),
        args: vec![
            "-title".to_string(),
            "YTM-RS".to_string(),
            "-e".to_string(),
            "ytm-rs".to_string(),
        ],
    }
}

fn spawn_terminal_window() {
    let term = detect_terminal_launcher();

    let use_uwsm = which("uwsm-app");
    let use_setsid = which("setsid");

    if use_setsid {
        let mut cmd = std::process::Command::new("setsid");
        cmd.arg("-f");
        if use_uwsm {
            cmd.args(["uwsm-app", "--"]);
        }
        cmd.arg(&term.cmd);
        cmd.args(&term.args);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let _ = cmd.status();
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let mut cmd = if use_uwsm {
                let mut c = std::process::Command::new("uwsm-app");
                c.arg("--").arg(&term.cmd);
                c
            } else {
                std::process::Command::new(&term.cmd)
            };
            cmd.args(&term.args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .process_group(0);
            let _ = cmd.spawn();
        }
        #[cfg(not(unix))]
        {
            let mut cmd = std::process::Command::new(&term.cmd);
            cmd.args(&term.args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let _ = cmd.spawn();
        }
    }
}

fn focus_or_launch_window() {
    // Check if a window with class ytm-rs already exists in Hyprland
    let output = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output();
    let mut window_address = None;
    let mut window_monitor = None;
    if let Ok(out) = output {
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            if let Some(arr) = val.as_array() {
                for client in arr {
                    let class = client.get("class").and_then(|c| c.as_str()).unwrap_or("");
                    let initial_class = client.get("initialClass").and_then(|c| c.as_str()).unwrap_or("");
                    let title = client.get("title").and_then(|c| c.as_str()).unwrap_or("");
                    let initial_title = client.get("initialTitle").and_then(|c| c.as_str()).unwrap_or("");
                    if class == "ytm-rs" || initial_class == "ytm-rs" || title == "YTM-RS" || initial_title == "YTM-RS" {
                        if let Some(addr) = client.get("address").and_then(|a| a.as_str()) {
                            window_address = Some(addr.to_string());
                        }
                        if let Some(mon) = client.get("monitor").and_then(|m| m.as_i64()) {
                            window_monitor = Some(mon);
                        }
                        break;
                    }
                }
            }
        }
    }

    if let Some(addr) = window_address {
        let lua_cmd = if let Some(mon) = window_monitor {
            format!(
                "hl.dispatch(hl.dsp.focus({{ monitor = '{}' }})); hl.dispatch(hl.dsp.focus({{ window = 'address:{}' }}))",
                mon, addr
            )
        } else {
            format!("hl.dispatch(hl.dsp.focus({{ window = 'address:{}' }}))", addr)
        };
        let _ = std::process::Command::new("hyprctl")
            .args(["eval", &lua_cmd])
            .output();
    } else {
        spawn_terminal_window();
    }
}

async fn run_ctl(config: AppConfig, action: CtlAction) -> Result<()> {
    match action {
        CtlAction::Focus => {
            focus_or_launch_window();
        }
        CtlAction::Toggle => {
            let _ = send_daemon_command(&config, DaemonCommand::Toggle).await?;
        }
        CtlAction::Play => {
            let _ = send_daemon_command(&config, DaemonCommand::Play).await?;
        }
        CtlAction::Pause => {
            let _ = send_daemon_command(&config, DaemonCommand::Pause).await?;
        }
        CtlAction::Stop => {
            let _ = send_daemon_command(&config, DaemonCommand::Stop).await?;
        }
        CtlAction::Next => {
            let _ = send_daemon_command(&config, DaemonCommand::Next).await?;
        }
        CtlAction::Prev => {
            let _ = send_daemon_command(&config, DaemonCommand::Prev).await?;
        }
        CtlAction::VolUp => {
            let _ = send_daemon_command(&config, DaemonCommand::VolumeDelta(5)).await?;
        }
        CtlAction::VolDown => {
            let _ = send_daemon_command(&config, DaemonCommand::VolumeDelta(-5)).await?;
        }
        CtlAction::Volume { level } => {
            let _ = send_daemon_command(&config, DaemonCommand::SetVolume(level)).await?;
        }
        CtlAction::Like => {
            let _ = send_daemon_command(&config, DaemonCommand::ToggleLike).await?;
        }
        CtlAction::Status => {
            let resp = send_daemon_command(&config, DaemonCommand::GetState).await?;
            if let DaemonResponse::State(state) = resp {
                println!("{}", serde_json::to_string_pretty(&state)?);
            }
        }
        CtlAction::Waybar => {
            let output = read_waybar_status(&config.status_file)?;
            println!("{}", output);
        }
    }
    Ok(())
}

async fn run_auth(config: AppConfig, action: Option<AuthAction>) -> Result<()> {
    match action {
        Some(AuthAction::Status) => {
            let auth = AuthConfig::load(&config.auth_file);
            if auth.is_authenticated() {
                println!("✓ Autenticado no YouTube Music.");
                if auth.sapisid.is_some() {
                    println!("  Método: Cookies / SAPISID ativo.");
                }
            } else {
                println!("✗ Não autenticado. Execute 'ytm-rs auth' para autenticar.");
            }
        }
        Some(AuthAction::Cookies { value }) => {
            let mut auth = AuthConfig::load(&config.auth_file);
            auth.sapisid = extract_sapisid_from_cookies(&value);
            auth.cookies = Some(value);
            auth.save(&config.auth_file)?;
            println!("✓ Cookies salvos com sucesso em {:?}", config.auth_file);
        }
        Some(AuthAction::Logout) => {
            if config.auth_file.exists() {
                std::fs::remove_file(&config.auth_file)?;
            }
            println!("✓ Credenciais removidas.");
        }
        None => {
            println!("=== Autenticação do YouTube Music (YTM-RS) ===");
            println!("1. Abra https://music.youtube.com no seu navegador");
            println!("2. Abra o DevTools (F12) -> aba Rede (Network)");
            println!("3. Faça uma busca ou toque uma música");
            println!("4. Copie o valor do cabeçalho 'cookie' da requisição");
            println!("5. Execute: ytm-rs auth cookies \"<conteúdo do cookie>\"\n");
            let auth = AuthConfig::load(&config.auth_file);
            if auth.is_authenticated() {
                println!("Status atual: ✓ Autenticado");
            } else {
                println!("Status atual: ✗ Não autenticado (Modo público ativo)");
            }
        }
    }
    Ok(())
}

async fn run_tui(config: AppConfig) -> Result<()> {
    // 1. Ensure daemon is up
    ensure_daemon_running(&config).await?;

    // 2. Terminal setup
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config.clone()).await?;

    // 3. Connect to daemon socket to subscribe to live updates
    let state_sock_path = config.state_sock.clone();
    let app_state_shared = Arc::new(Mutex::new(app.player_state.clone()));
    let shared_for_sock = Arc::clone(&app_state_shared);

    tokio::spawn(async move {
        if let Ok(stream) = UnixStream::connect(&state_sock_path).await {
            let (reader, mut writer) = stream.into_split();
            let mut line = serde_json::to_string(&DaemonCommand::Subscribe).unwrap_or_default();
            line.push('\n');
            let _ = writer.write_all(line.as_bytes()).await;

            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(msg)) = lines.next_line().await {
                if let Ok(DaemonResponse::State(new_state)) = serde_json::from_str::<DaemonResponse>(&msg) {
                    let mut s = shared_for_sock.lock().await;
                    *s = *new_state;
                }
            }
        }
    });

    // 4. Watch Omarchy theme live
    let theme_watcher = ThemeWatcher::new();

    // 5. Main UI loop
    let mut last_tick = std::time::Instant::now();
    let tick_rate = Duration::from_millis(50);

    loop {
        // Sync player state from subscription
        {
            let s = app_state_shared.lock().await;
            app.player_state = s.clone();
        }
        app.sync_queue_if_active();

        // Check if Omarchy theme changed
        if let Some(watcher) = &theme_watcher {
            if watcher.rx.try_recv().is_ok() {
                app.theme = Theme::load_omarchy();
            }
        }

        // Update cover art, thumbnail, avatar and account sync if needed
        app.poll_account_sync().await;
        app.update_cover_if_needed();
        app.update_thumbnails_if_needed();
        app.update_avatar_if_needed();

        // Draw UI
        terminal.draw(|f| ui::render_ui(f, &mut app))?;

        // Handle input events
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_millis(0));

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        app.handle_key_event(key).await?;
                    }
                }
                Event::Mouse(mouse) => {
                    app.handle_mouse_event(mouse).await?;
                }
                Event::Paste(text) => {
                    app.handle_paste(&text);
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = std::time::Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste)?;
    terminal.show_cursor()?;

    Ok(())
}
