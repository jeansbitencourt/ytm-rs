use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::fs::OpenOptions;
use std::io::stdout;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::info;

use ytm_rs::app::App;
use ytm_rs::audio::daemon::{ensure_daemon_running, send_daemon_command, AudioDaemon};
use ytm_rs::audio::{DaemonCommand, DaemonResponse, PlayerState};
use ytm_rs::config::{AppConfig, PlaybackAuthSettings, PlaybackAuthSource};
use ytm_rs::innertube::auth::{extract_sapisid_from_cookies, AuthConfig};
use ytm_rs::theme::{Theme, ThemeWatcher};
use ytm_rs::topbar::waybar::read_waybar_status;
use ytm_rs::ui;

#[derive(Parser)]
#[command(name = "ytm-rs")]
#[command(
    about = "YouTube Music TUI and background daemon for Omarchy",
    version = "0.1.0"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the audio daemon in the background
    Daemon,
    /// Send quick control commands to the running daemon (< 2 ms)
    Ctl {
        #[command(subcommand)]
        action: CtlAction,
    },
    /// Focus the existing YTM-RS window or open a new one in the appropriate terminal
    Launch,
    /// Manage YouTube Music authentication (cookies and account)
    Auth {
        #[command(subcommand)]
        action: Option<AuthAction>,
    },
}

#[derive(Subcommand)]
enum CtlAction {
    /// Focus the existing YTM-RS window or open a new one if it does not exist
    Focus,
    /// Toggle playback / pause
    Toggle,
    /// Start playback
    Play,
    /// Pause playback
    Pause,
    /// Stop playback
    Stop,
    /// Skip to the next track
    Next,
    /// Go back to the previous track
    Prev,
    /// Increase volume (+5%)
    VolUp,
    /// Decrease volume (-5%)
    VolDown,
    /// Set the exact volume (0-100)
    Volume { level: u8 },
    /// Like / unlike the current track
    Like,
    /// Toggle shuffle mode (Shuffle ON/OFF)
    Shuffle,
    /// Display the current state as JSON
    Status,
    /// Emit formatted JSON for Waybar / the Omarchy Top Bar
    Waybar,
}

#[derive(Subcommand)]
enum AuthAction {
    /// Display the current authentication status
    Status,
    /// Start interactive Google login using a TV code (google.com/device)
    Login,
    /// Read a Cookie header for library access from standard input
    Cookies,
    /// Remove authentication credentials
    Logout,
    /// Configure authentication used only for stream extraction
    Playback {
        #[command(subcommand)]
        action: PlaybackAuthAction,
    },
}

#[derive(Subcommand)]
enum PlaybackAuthAction {
    /// Show the stream authentication source without contacting YouTube
    Status,
    /// Query only the available audio formats for one video ID; do not download media
    Inspect { video_id: String },
    /// Read cookies from a supported browser profile using yt-dlp
    Browser {
        #[arg(value_enum)]
        browser: PlaybackBrowser,
        #[arg(long)]
        profile: Option<String>,
    },
    /// Import an existing Netscape-format cookie file into private app storage
    Cookies { file: PathBuf },
    /// Disable playback credentials and remove the imported cookie copy
    Disable,
    /// Explicitly enable or disable background prefetch for authenticated playback
    Prefetch { setting: PrefetchSetting },
}

#[derive(Clone, Copy, ValueEnum)]
enum PlaybackBrowser {
    Brave,
    Chrome,
    Chromium,
    Edge,
    Firefox,
    Opera,
    Safari,
    Vivaldi,
    Whale,
}

impl PlaybackBrowser {
    fn as_str(self) -> &'static str {
        match self {
            Self::Brave => "brave",
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
            Self::Firefox => "firefox",
            Self::Opera => "opera",
            Self::Safari => "safari",
            Self::Vivaldi => "vivaldi",
            Self::Whale => "whale",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PrefetchSetting {
    On,
    Off,
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
    let log_path = config.runtime_dir.join("daemon.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open daemon log at {:?}", log_path))?;
    let log_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .with_writer(log_file)
        .try_init()
        .map_err(|e| anyhow::anyhow!("Failed to initialize daemon logging: {e}"))?;
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
    let user_terminal = std::env::var("YTM_TERMINAL").ok().or_else(|| {
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
                    let initial_class = client
                        .get("initialClass")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let title = client.get("title").and_then(|c| c.as_str()).unwrap_or("");
                    let initial_title = client
                        .get("initialTitle")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    if class == "ytm-rs"
                        || initial_class == "ytm-rs"
                        || title == "YTM-RS"
                        || initial_title == "YTM-RS"
                    {
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
            format!(
                "hl.dispatch(hl.dsp.focus({{ window = 'address:{}' }}))",
                addr
            )
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
        CtlAction::Shuffle => {
            let _ = send_daemon_command(&config, DaemonCommand::ToggleShuffle).await?;
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
                println!("Biblioteca: credenciais configuradas.");
                if auth.has_oauth() {
                    println!("  Método: Google OAuth 2.0 (Device Code - Token permanente).");
                }
                if auth.sapisid.is_some() {
                    println!("  Método: Cookies / SAPISID ativo.");
                }
            } else {
                println!("Biblioteca: desconectada (busca pública disponível).");
            }
            print_playback_auth_status(&config)?;
        }
        Some(AuthAction::Login) => {
            println!("Iniciando autenticação Google Device Flow (RFC 8628)...");
            let device_res = ytm_rs::innertube::oauth::request_device_code(
                ytm_rs::innertube::oauth::DEFAULT_CLIENT_ID,
            )
            .await?;
            println!("\n╔══════════════════════════════════════════════════════════════════╗");
            println!(
                "║  1. Acesse no navegador: {:<39} ║",
                device_res.verification_url
            );
            println!("║  2. Digite o código:     {:<39} ║", device_res.user_code);
            println!("║  3. Conecte sua conta Google e clique em Permitir               ║");
            println!("╚══════════════════════════════════════════════════════════════════╝\n");
            println!(
                "⏳ Aguardando autorização no navegador (código expira em {} min)...",
                device_res.expires_in / 60
            );

            let poll_interval = Duration::from_secs(device_res.interval.max(5));
            let start = std::time::Instant::now();
            let timeout = Duration::from_secs(device_res.expires_in);

            while start.elapsed() < timeout {
                tokio::time::sleep(poll_interval).await;
                match ytm_rs::innertube::oauth::poll_token_once(
                    ytm_rs::innertube::oauth::DEFAULT_CLIENT_ID,
                    ytm_rs::innertube::oauth::DEFAULT_CLIENT_SECRET,
                    &device_res.device_code,
                )
                .await?
                {
                    ytm_rs::innertube::oauth::PollResult::Success(tok) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();

                        let mut auth = AuthConfig::load(&config.auth_file);
                        auth.oauth_token = Some(tok.access_token.clone());
                        if let Some(ref_tok) = tok.refresh_token {
                            auth.refresh_token = Some(ref_tok);
                        }
                        auth.token_expiry = Some(now + tok.expires_in);
                        auth.save(&config.auth_file)?;

                        println!("\n✓ Autorização concluída com sucesso!");
                        let client = ytm_rs::innertube::client::InnertubeClient::new(auth);
                        if let Ok(Some(profile)) = client.get_user_profile().await {
                            println!("✓ Conectado como: {} ({})", profile.name, profile.handle);
                        }
                        println!("✓ Credenciais salvas em: {:?}", config.auth_file);
                        println!(
                            "O token permanente foi registrado e será renovado automaticamente!"
                        );
                        return Ok(());
                    }
                    ytm_rs::innertube::oauth::PollResult::Pending => {
                        print!(".");
                        std::io::Write::flush(&mut std::io::stdout())?;
                    }
                    ytm_rs::innertube::oauth::PollResult::SlowDown => {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    ytm_rs::innertube::oauth::PollResult::Expired => {
                        println!("\n✗ O código expirou. Execute 'ytm-rs auth login' novamente.");
                        return Ok(());
                    }
                    ytm_rs::innertube::oauth::PollResult::Error(err) => {
                        println!("\n✗ Erro ao autorizar: {}", err);
                        return Ok(());
                    }
                }
            }
            println!("\n✗ Tempo limite de autorização esgotado.");
        }
        Some(AuthAction::Cookies) => {
            use std::io::{IsTerminal, Read};
            let mut stdin = std::io::stdin();
            if stdin.is_terminal() {
                anyhow::bail!("Pipe the Cookie header on standard input; do not pass it as a command-line argument");
            }
            let mut value = String::new();
            stdin.read_to_string(&mut value)?;
            let value = value.trim().to_string();
            if value.is_empty() {
                anyhow::bail!("No Cookie header was provided on standard input");
            }
            let mut auth = AuthConfig::load(&config.auth_file);
            auth.sapisid = extract_sapisid_from_cookies(&value);
            auth.cookies = Some(value);
            auth.save(&config.auth_file)?;
            println!("✓ Cookies salvos com sucesso em {:?}", config.auth_file);
        }
        Some(AuthAction::Logout) => {
            config.clear_playback_auth()?;
            if config.auth_file.exists() {
                std::fs::remove_file(&config.auth_file)?;
            }
            notify_daemon_playback_auth_changed(&config).await;
            println!("✓ Credenciais da biblioteca e da reprodução removidas.");
        }
        Some(AuthAction::Playback { action }) => run_playback_auth(config, action).await?,
        None => {
            println!("=== Autenticação do YouTube Music (YTM-RS) ===");
            println!("Opção 1 (Recomendada): Login Google permanente via código:");
            println!("  execute: ytm-rs auth login\n");
            println!("Opção 2: Ler um cabeçalho Cookie pela entrada padrão:");
            println!("  execute: cat cookie-header.txt | ytm-rs auth cookies\n");
            let auth = AuthConfig::load(&config.auth_file);
            if auth.is_authenticated() {
                println!("Status atual: ✓ Credenciais da biblioteca configuradas");
            } else {
                println!("Status atual: ✗ Não autenticado (Modo público ativo)");
            }
            println!("A autenticação da biblioteca não habilita credenciais de reprodução.");
            println!("Use 'ytm-rs auth playback --help' para configurar uma fonte separada.");
        }
    }
    Ok(())
}

fn print_playback_auth_status(config: &AppConfig) -> Result<()> {
    let settings = config.load_playback_auth()?;
    if settings.source.is_enabled() {
        println!(
            "Reprodução: credenciais configuradas via {} (a sessão ainda não foi validada).",
            settings.source.description()
        );
        println!(
            "  Prefetch autenticado: {}.",
            if settings.prefetch_enabled {
                "ativo"
            } else {
                "desativado"
            }
        );
        println!("  Opção experimental: uso da conta pode causar bloqueio; nenhum benefício Premium concreto foi demonstrado.");
    } else {
        println!("Reprodução: anônima (padrão).");
    }
    Ok(())
}

async fn run_playback_auth(config: AppConfig, action: PlaybackAuthAction) -> Result<()> {
    match action {
        PlaybackAuthAction::Status => print_playback_auth_status(&config)?,
        PlaybackAuthAction::Inspect { video_id } => {
            let formats =
                ytm_rs::audio::prefetch::inspect_stream_formats(&video_id, &config).await?;
            println!("Formatos de áudio disponíveis para {video_id} (nenhuma mídia baixada):");
            for format in formats {
                let bitrate = format
                    .bitrate_kbps
                    .map(|value| format!("{value:.0} kb/s"))
                    .unwrap_or_else(|| "ABR desconhecido".to_string());
                let sample_rate = format
                    .sample_rate_hz
                    .map(|value| format!("{value} Hz"))
                    .unwrap_or_else(|| "taxa desconhecida".to_string());
                println!(
                    "  {:<8} {:<8} {:<12} {}",
                    format.id,
                    format.extension,
                    format.codec,
                    format!("{bitrate}, {sample_rate}")
                );
            }
            println!("A consulta foi limitada a uma extração sem retry. Aguarde pelo menos uma hora antes de qualquer reprodução autenticada.");
        }
        PlaybackAuthAction::Browser { browser, profile } => {
            let settings = PlaybackAuthSettings {
                source: PlaybackAuthSource::Browser {
                    browser: browser.as_str().to_string(),
                    profile,
                },
                // Authenticated prefetch is opt-in to avoid hidden extraction requests.
                prefetch_enabled: false,
            };
            config.save_playback_auth(&settings)?;
            notify_daemon_playback_auth_changed(&config).await;
            println!(
                "✓ Fonte de reprodução configurada: {}.",
                settings.source.description()
            );
            println!("  Opção experimental: uso da conta pode causar bloqueio; nenhum benefício Premium concreto foi demonstrado.");
        }
        PlaybackAuthAction::Cookies { file } => {
            config.import_playback_cookies(&file)?;
            notify_daemon_playback_auth_changed(&config).await;
            println!("✓ Arquivo Netscape importado para armazenamento privado do aplicativo.");
            println!("  A cópia importada é apagada por 'ytm-rs auth playback disable' ou 'ytm-rs auth logout'.");
        }
        PlaybackAuthAction::Disable => {
            config.clear_playback_auth()?;
            notify_daemon_playback_auth_changed(&config).await;
            println!("✓ Reprodução anônima restaurada; a cópia importada foi removida.");
        }
        PlaybackAuthAction::Prefetch { setting } => {
            let mut settings = config.load_playback_auth()?;
            if !settings.source.is_enabled() {
                anyhow::bail!(
                    "Configure uma fonte de reprodução antes de alterar o prefetch autenticado"
                );
            }
            settings.prefetch_enabled = matches!(setting, PrefetchSetting::On);
            config.save_playback_auth(&settings)?;
            println!(
                "✓ Prefetch autenticado {}.",
                if settings.prefetch_enabled {
                    "ativado"
                } else {
                    "desativado"
                }
            );
        }
    }
    Ok(())
}

async fn notify_daemon_playback_auth_changed(config: &AppConfig) {
    let Ok(stream) = UnixStream::connect(&config.state_sock).await else {
        return;
    };
    let (reader, mut writer) = stream.into_split();
    let Ok(mut message) = serde_json::to_string(&DaemonCommand::PlaybackAuthChanged) else {
        return;
    };
    message.push('\n');
    let update = async {
        writer.write_all(message.as_bytes()).await?;
        let mut lines = BufReader::new(reader).lines();
        let _ = lines.next_line().await?;
        Ok::<(), anyhow::Error>(())
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), update).await;
}

async fn run_tui(config: AppConfig) -> Result<()> {
    // 1. Ensure daemon is up
    ensure_daemon_running(&config).await?;

    // 2. Terminal setup
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config.clone()).await?;

    // 3. Connect to daemon socket to subscribe to live updates
    let state_sock_path = config.state_sock.clone();
    let daemon_config = config.clone();
    let app_state_shared = Arc::new(Mutex::new(app.player_state.clone()));
    let shared_for_sock = Arc::clone(&app_state_shared);

    let state_sync_task = tokio::spawn(async move {
        loop {
            if let Ok(stream) = UnixStream::connect(&state_sock_path).await {
                let (reader, mut writer) = stream.into_split();
                let mut line = serde_json::to_string(&DaemonCommand::Subscribe).unwrap_or_default();
                line.push('\n');
                if writer.write_all(line.as_bytes()).await.is_ok() {
                    let mut lines = BufReader::new(reader).lines();
                    while let Ok(Some(msg)) = lines.next_line().await {
                        if let Ok(DaemonResponse::State(new_state)) =
                            serde_json::from_str::<DaemonResponse>(&msg)
                        {
                            let mut state = shared_for_sock.lock().await;
                            *state = *new_state;
                        }
                    }
                }
            }

            *shared_for_sock.lock().await = PlayerState::default();
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = ensure_daemon_running(&daemon_config).await;
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

        // In automatic mode, follow a language changed from Omarchy Setup while
        // this TUI is already open. The audio daemon remains independent.
        if app.i18n.preference == ytm_rs::i18n::LanguagePreference::Auto {
            let refreshed = ytm_rs::i18n::I18n::new(app.i18n.preference);
            if refreshed.language != app.i18n.language {
                app.i18n = refreshed;
            }
        }

        // Update cover art, thumbnail, avatar and account sync if needed
        app.poll_account_sync().await;
        app.poll_playlist_load();
        app.poll_oauth_progress().await;
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
    state_sync_task.abort();

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    Ok(())
}
