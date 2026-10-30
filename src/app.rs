use std::sync::Arc;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};
use tokio::sync::Mutex;
use tracing::info;

use crate::audio::daemon::send_daemon_command;
use crate::audio::{DaemonCommand, PlayerState};
use crate::cache::Database;
use crate::config::AppConfig;
use crate::innertube::client::InnertubeClient;
use crate::innertube::Track;
use crate::theme::Theme;
use crate::ui::playerbar::{get_player_button_at, PlayerButton};
use crate::ui::sidebar::SidebarItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Sidebar,
    Tracklist,
    Search,
}

pub struct App {
    pub config: AppConfig,
    pub theme: Theme,
    pub active_panel: ActivePanel,
    pub sidebar_state: ListState,
    pub tracklist_state: TableState,
    pub active_sidebar_item: SidebarItem,
    pub active_category_title: String,
    pub displayed_tracks: Vec<Track>,
    pub liked_tracks: Vec<Track>,
    pub history_tracks: Vec<Track>,
    pub search_input: String,
    pub search_results: Vec<Track>,
    pub is_searching: bool,
    pub player_state: PlayerState,
    pub show_help: bool,
    pub show_auth_modal: bool,
    pub auth_input: String,
    pub auth_message: Option<(String, bool)>,
    pub should_quit: bool,
    pub playerbar_area: Rect,
    pub client: InnertubeClient,
    pub auth: crate::innertube::auth::AuthConfig,
    pub db: Arc<Mutex<Database>>,
}

impl App {
    pub async fn new(config: AppConfig) -> Result<Self> {
        let theme = Theme::load_omarchy();
        let db_instance = Database::open(&config.db_file)?;
        let db = Arc::new(Mutex::new(db_instance));

        let auth = crate::innertube::auth::AuthConfig::load(&config.auth_file);
        let client = InnertubeClient::new(auth.clone());

        let mut sidebar_state = ListState::default();
        sidebar_state.select(Some(0));

        let mut tracklist_state = TableState::default();
        tracklist_state.select(Some(0));

        let (liked, history) = {
            let db_guard = db.lock().await;
            (
                db_guard.get_liked_tracks().unwrap_or_default(),
                db_guard.get_history(50).unwrap_or_default(),
            )
        };

        let active_sidebar_item = SidebarItem::LikedTracks;
        let displayed_tracks = liked.clone();
        let active_category_title = "Músicas Curtidas".to_string();

        Ok(Self {
            config,
            theme,
            active_panel: ActivePanel::Tracklist,
            sidebar_state,
            tracklist_state,
            active_sidebar_item,
            active_category_title,
            displayed_tracks,
            liked_tracks: liked,
            history_tracks: history,
            search_input: String::new(),
            search_results: Vec::new(),
            is_searching: false,
            player_state: PlayerState::default(),
            show_help: false,
            show_auth_modal: false,
            auth_input: String::new(),
            auth_message: None,
            should_quit: false,
            playerbar_area: Rect::default(),
            client,
            auth,
            db,
        })
    }

    pub async fn refresh_data(&mut self) {
        let db = self.db.lock().await;
        self.liked_tracks = db.get_liked_tracks().unwrap_or_default();
        self.history_tracks = db.get_history(50).unwrap_or_default();
        drop(db);

        match self.active_sidebar_item {
            SidebarItem::LikedTracks => {
                self.displayed_tracks = self.liked_tracks.clone();
                self.active_category_title = "Músicas Curtidas".to_string();
            }
            SidebarItem::History => {
                self.displayed_tracks = self.history_tracks.clone();
                self.active_category_title = "Histórico Recente".to_string();
            }
            SidebarItem::CurrentQueue => {
                self.displayed_tracks = self.player_state.queue.clone();
                self.active_category_title = "Fila / Rádio Atual".to_string();
            }
        }
    }

    pub fn select_sidebar_item(&mut self, item: SidebarItem) {
        self.active_sidebar_item = item;
        match item {
            SidebarItem::LikedTracks => {
                self.displayed_tracks = self.liked_tracks.clone();
                self.active_category_title = "Músicas Curtidas".to_string();
            }
            SidebarItem::History => {
                self.displayed_tracks = self.history_tracks.clone();
                self.active_category_title = "Histórico Recente".to_string();
            }
            SidebarItem::CurrentQueue => {
                self.displayed_tracks = self.player_state.queue.clone();
                self.active_category_title = "Fila / Rádio Atual".to_string();
            }
        }
        self.tracklist_state.select(Some(0));
    }

    pub async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
        if self.show_help {
            if key.code == KeyCode::Esc || key.code == KeyCode::Char('?') || key.code == KeyCode::Char('q') {
                self.show_help = false;
            }
            return Ok(());
        }

        if self.show_auth_modal {
            match key.code {
                KeyCode::Esc => {
                    self.show_auth_modal = false;
                }
                KeyCode::Enter => {
                    if !self.auth_input.trim().is_empty() {
                        let value = self.auth_input.trim().to_string();
                        let sapisid = crate::innertube::auth::extract_sapisid_from_cookies(&value);
                        let mut new_auth = self.auth.clone();
                        new_auth.cookies = Some(value);
                        new_auth.sapisid = sapisid;
                        if let Err(e) = new_auth.save(&self.config.auth_file) {
                            self.auth_message = Some((format!("Erro ao salvar: {}", e), false));
                        } else {
                            self.auth = new_auth.clone();
                            self.client.auth = new_auth;
                            self.auth_message = Some(("✓ Autenticado com sucesso no YouTube Music!".to_string(), true));
                            self.auth_input.clear();
                        }
                    }
                }
                KeyCode::Backspace => {
                    self.auth_input.pop();
                }
                KeyCode::Char('l') | KeyCode::Char('L') if self.auth_input.is_empty() => {
                    if self.auth.is_authenticated() {
                        let _ = std::fs::remove_file(&self.config.auth_file);
                        self.auth = crate::innertube::auth::AuthConfig::default();
                        self.client.auth = self.auth.clone();
                        self.auth_message = Some(("✓ Desconectado. Modo Público ativo.".to_string(), true));
                    }
                }
                KeyCode::Char(c) => {
                    self.auth_input.push(c);
                }
                _ => {}
            }
            return Ok(());
        }

        // Global shortcuts when not in search input mode
        if self.active_panel != ActivePanel::Search {
            match key.code {
                KeyCode::Char('?') => {
                    self.show_help = true;
                    return Ok(());
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    self.show_auth_modal = true;
                    self.auth_input.clear();
                    self.auth_message = None;
                    return Ok(());
                }
                KeyCode::Char('q') => {
                    // Close TUI, keep daemon running
                    self.should_quit = true;
                    return Ok(());
                }
                KeyCode::Char('Q') => {
                    // Stop daemon and quit
                    let _ = send_daemon_command(&self.config, DaemonCommand::StopDaemon).await;
                    self.should_quit = true;
                    return Ok(());
                }
                KeyCode::Char(' ') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::Toggle).await;
                    return Ok(());
                }
                KeyCode::Char('n') | KeyCode::Char('>') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::Next).await;
                    return Ok(());
                }
                KeyCode::Char('p') | KeyCode::Char('<') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::Prev).await;
                    return Ok(());
                }
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::VolumeDelta(5)).await;
                    return Ok(());
                }
                KeyCode::Char('-') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::VolumeDelta(-5)).await;
                    return Ok(());
                }
                KeyCode::Char('f') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::ToggleLike).await;
                    self.refresh_data().await;
                    return Ok(());
                }
                KeyCode::Char('1') => {
                    self.active_panel = ActivePanel::Sidebar;
                    return Ok(());
                }
                KeyCode::Char('2') => {
                    self.active_panel = ActivePanel::Tracklist;
                    return Ok(());
                }
                KeyCode::Char('/') => {
                    self.active_panel = ActivePanel::Search;
                    return Ok(());
                }
                _ => {}
            }
        }

        match self.active_panel {
            ActivePanel::Search => {
                match key.code {
                    KeyCode::Esc => {
                        self.active_panel = ActivePanel::Tracklist;
                    }
                    KeyCode::Enter => {
                        if !self.search_input.trim().is_empty() {
                            self.perform_search().await;
                            self.active_panel = ActivePanel::Tracklist;
                        }
                    }
                    KeyCode::Backspace => {
                        self.search_input.pop();
                    }
                    KeyCode::Char(c) => {
                        self.search_input.push(c);
                    }
                    _ => {}
                }
            }
            ActivePanel::Sidebar => {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        let i = match self.sidebar_state.selected() {
                            Some(i) => {
                                if i == 0 {
                                    SidebarItem::ALL.len() - 1
                                } else {
                                    i - 1
                                }
                            }
                            None => 0,
                        };
                        self.sidebar_state.select(Some(i));
                        self.select_sidebar_item(SidebarItem::ALL[i]);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let i = match self.sidebar_state.selected() {
                            Some(i) => {
                                if i >= SidebarItem::ALL.len() - 1 {
                                    0
                                } else {
                                    i + 1
                                }
                            }
                            None => 0,
                        };
                        self.sidebar_state.select(Some(i));
                        self.select_sidebar_item(SidebarItem::ALL[i]);
                    }
                    KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                        self.active_panel = ActivePanel::Tracklist;
                    }
                    _ => {}
                }
            }
            ActivePanel::Tracklist => {
                let track_count = self.displayed_tracks.len();
                match key.code {
                    KeyCode::Left | KeyCode::Char('h') => {
                        self.active_panel = ActivePanel::Sidebar;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if track_count > 0 {
                            let i = match self.tracklist_state.selected() {
                                Some(i) => {
                                    if i == 0 {
                                        track_count - 1
                                    } else {
                                        i - 1
                                    }
                                }
                                None => 0,
                            };
                            self.tracklist_state.select(Some(i));
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if track_count > 0 {
                            let i = match self.tracklist_state.selected() {
                                Some(i) => {
                                    if i >= track_count - 1 {
                                        0
                                    } else {
                                        i + 1
                                    }
                                }
                                None => 0,
                            };
                            self.tracklist_state.select(Some(i));
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(idx) = self.tracklist_state.selected() {
                            if idx < self.displayed_tracks.len() {
                                let tracks = self.displayed_tracks.clone();
                                let _ = send_daemon_command(
                                    &self.config,
                                    DaemonCommand::QueueTracks(tracks, idx),
                                )
                                .await;
                            }
                        }
                    }
                    KeyCode::Char('a') => {
                        // Enqueue track
                        if let Some(idx) = self.tracklist_state.selected() {
                            if let Some(track) = self.displayed_tracks.get(idx).cloned() {
                                let _ = send_daemon_command(
                                    &self.config,
                                    DaemonCommand::EnqueueTrack(track),
                                )
                                .await;
                            }
                        }
                    }
                    KeyCode::Char('r') => {
                        // Start radio from selected track
                        if let Some(idx) = self.tracklist_state.selected() {
                            if let Some(track) = self.displayed_tracks.get(idx) {
                                let vid = track.video_id.clone();
                                let title = track.title.clone();
                                self.start_radio_for_track(&vid, &title).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    pub async fn handle_mouse_event(&mut self, mouse: MouseEvent) -> Result<()> {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            // Check playerbar button clicks
            if let Some(btn) = get_player_button_at(mouse.column, mouse.row, self.playerbar_area) {
                match btn {
                    PlayerButton::Prev => {
                        let _ = send_daemon_command(&self.config, DaemonCommand::Prev).await;
                    }
                    PlayerButton::PlayPause => {
                        let _ = send_daemon_command(&self.config, DaemonCommand::Toggle).await;
                    }
                    PlayerButton::Next => {
                        let _ = send_daemon_command(&self.config, DaemonCommand::Next).await;
                    }
                    PlayerButton::Like => {
                        let _ = send_daemon_command(&self.config, DaemonCommand::ToggleLike).await;
                        self.refresh_data().await;
                    }
                    PlayerButton::Radio => {
                        if let Some(track) = &self.player_state.current_track {
                            let vid = track.video_id.clone();
                            let title = track.title.clone();
                            self.start_radio_for_track(&vid, &title).await;
                        }
                    }
                    PlayerButton::VolDown => {
                        let _ = send_daemon_command(&self.config, DaemonCommand::VolumeDelta(-5)).await;
                    }
                    PlayerButton::VolUp => {
                        let _ = send_daemon_command(&self.config, DaemonCommand::VolumeDelta(5)).await;
                    }
                    PlayerButton::Account => {
                        self.show_auth_modal = !self.show_auth_modal;
                        self.auth_input.clear();
                        self.auth_message = None;
                    }
                    PlayerButton::Help => {
                        self.show_help = !self.show_help;
                    }
                }
                return Ok(());
            }

            if self.show_auth_modal {
                self.show_auth_modal = false;
                return Ok(());
            }

            // Clicking top header enters search mode
            if mouse.row <= 2 {
                self.active_panel = ActivePanel::Search;
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn handle_paste(&mut self, text: &str) {
        if self.show_auth_modal {
            self.auth_input.push_str(text);
        } else if self.active_panel == ActivePanel::Search {
            self.search_input.push_str(text);
        }
    }

    pub async fn perform_search(&mut self) {
        let q = self.search_input.trim().to_string();
        if q.is_empty() {
            return;
        }

        info!("Searching YouTube Music for query: {}", q);
        if let Ok(tracks) = self.client.search(&q).await {
            if !tracks.is_empty() {
                self.displayed_tracks = tracks.clone();
                self.search_results = tracks;
                self.active_category_title = format!("Busca: \"{}\"", q);
                self.tracklist_state.select(Some(0));
            }
        }
    }

    pub async fn start_radio_for_track(&mut self, video_id: &str, title: &str) {
        info!("Fetching YouTube Music radio station for: {}", video_id);
        if let Ok(tracks) = self.client.radio(video_id).await {
            if !tracks.is_empty() {
                self.displayed_tracks = tracks.clone();
                self.active_category_title = format!("Rádio: \"{}\"", title);
                self.tracklist_state.select(Some(0));

                let _ = send_daemon_command(
                    &self.config,
                    DaemonCommand::QueueTracks(tracks, 0),
                )
                .await;
            }
        }
    }
}
