use std::sync::Arc;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tracing::info;

use crate::audio::daemon::send_daemon_command;
use crate::audio::{DaemonCommand, PlayerState};
use crate::cache::Database;
use crate::config::AppConfig;
use crate::innertube::client::InnertubeClient;
use crate::innertube::{AlbumSummary, ArtistSummary, SearchFilter, Track};
use crate::theme::Theme;
use crate::ui::playerbar::{get_player_button_at, PlayerButton};
use crate::ui::sidebar::SidebarItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Sidebar,
    Tracklist,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentView {
    Tracks,
    Albums,
    Artists,
}

pub struct App {
    pub config: AppConfig,
    pub theme: Theme,
    pub active_panel: ActivePanel,
    pub content_view: ContentView,
    pub search_filter: SearchFilter,
    pub sidebar_state: ListState,
    pub tracklist_state: TableState,
    pub album_table_state: TableState,
    pub artist_table_state: TableState,
    pub active_sidebar_item: SidebarItem,
    pub active_category_title: String,
    pub displayed_tracks: Vec<Track>,
    pub liked_tracks: Vec<Track>,
    pub history_tracks: Vec<Track>,
    pub search_input: String,
    pub search_results: Vec<Track>,
    pub search_results_albums: Vec<AlbumSummary>,
    pub search_results_artists: Vec<ArtistSummary>,
    pub previous_view: Option<(ContentView, String)>,
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
    pub picker: Picker,
    pub cover_protocol: Option<Box<dyn StatefulProtocol>>,
    pub cover_video_id: Option<String>,
    pub cover_tx: UnboundedSender<(String, image::DynamicImage)>,
    pub cover_rx: UnboundedReceiver<(String, image::DynamicImage)>,
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

        let mut album_table_state = TableState::default();
        album_table_state.select(Some(0));

        let mut artist_table_state = TableState::default();
        artist_table_state.select(Some(0));

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

        let mut picker = match Picker::from_termios() {
            Ok(p) => p,
            Err(_) => Picker::new((8, 16)),
        };
        picker.guess_protocol();

        let (cover_tx, cover_rx) = unbounded_channel();

        Ok(Self {
            config,
            theme,
            active_panel: ActivePanel::Tracklist,
            content_view: ContentView::Tracks,
            search_filter: SearchFilter::Tracks,
            sidebar_state,
            tracklist_state,
            album_table_state,
            artist_table_state,
            active_sidebar_item,
            active_category_title,
            displayed_tracks,
            liked_tracks: liked,
            history_tracks: history,
            search_input: String::new(),
            search_results: Vec::new(),
            search_results_albums: Vec::new(),
            search_results_artists: Vec::new(),
            previous_view: None,
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
            picker,
            cover_protocol: None,
            cover_video_id: None,
            cover_tx,
            cover_rx,
        })
    }

    pub fn update_cover_if_needed(&mut self) {
        while let Ok((vid, img)) = self.cover_rx.try_recv() {
            if self.player_state.current_track.as_ref().map(|t| &t.video_id) == Some(&vid) {
                self.cover_protocol = Some(self.picker.new_resize_protocol(img));
            }
        }

        let current_track = self.player_state.current_track.as_ref();
        let current_vid = current_track.map(|t| t.video_id.as_str());

        if current_vid != self.cover_video_id.as_deref() {
            self.cover_video_id = current_vid.map(|s| s.to_string());
            self.cover_protocol = None;

            if let Some(track) = current_track {
                if !track.thumbnail_url.is_empty() {
                    let vid = track.video_id.clone();
                    let url = track.thumbnail_url.clone();
                    let tx = self.cover_tx.clone();

                    tokio::spawn(async move {
                        let fetch_url = if url.contains("=w") && url.contains("-h") {
                            let prefix = url.split("=w").next().unwrap_or(&url);
                            format!("{}=w544-h544-l90-rj", prefix)
                        } else {
                            url.clone()
                        };

                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(8))
                            .build()
                            .unwrap_or_default();

                        let resp = match client.get(&fetch_url).send().await {
                            Ok(r) if r.status().is_success() => Ok(r),
                            _ => client.get(&url).send().await,
                        };

                        if let Ok(res) = resp {
                            if let Ok(bytes) = res.bytes().await {
                                if let Ok(img) = image::load_from_memory(&bytes) {
                                    let _ = tx.send((vid, img));
                                }
                            }
                        }
                    });
                }
            }
        }
    }

    pub async fn refresh_data(&mut self) {
        let db = self.db.lock().await;
        self.liked_tracks = db.get_liked_tracks().unwrap_or_default();
        self.history_tracks = db.get_history(50).unwrap_or_default();
        drop(db);

        if self.content_view == ContentView::Tracks && self.previous_view.is_none() {
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
    }

    pub fn select_sidebar_item(&mut self, item: SidebarItem) {
        self.active_sidebar_item = item;
        self.content_view = ContentView::Tracks;
        self.previous_view = None;
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
                KeyCode::Char('<') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::Prev).await;
                    return Ok(());
                }
                KeyCode::Char('p') if self.active_panel != ActivePanel::Tracklist || self.content_view == ContentView::Tracks => {
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
                    KeyCode::Tab => {
                        self.search_filter = self.search_filter.next();
                    }
                    KeyCode::BackTab => {
                        self.search_filter = self.search_filter.prev();
                    }
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
                match self.content_view {
                    ContentView::Tracks => {
                        let track_count = self.displayed_tracks.len();
                        match key.code {
                            KeyCode::Left | KeyCode::Char('h') => {
                                self.active_panel = ActivePanel::Sidebar;
                            }
                            KeyCode::Backspace | KeyCode::Esc => {
                                if let Some((prev_view, prev_title)) = self.previous_view.take() {
                                    self.content_view = prev_view;
                                    self.active_category_title = prev_title;
                                }
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
                    ContentView::Albums => {
                        let album_count = self.search_results_albums.len();
                        match key.code {
                            KeyCode::Left | KeyCode::Char('h') => {
                                self.active_panel = ActivePanel::Sidebar;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                if album_count > 0 {
                                    let i = match self.album_table_state.selected() {
                                        Some(i) => {
                                            if i == 0 {
                                                album_count - 1
                                            } else {
                                                i - 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    self.album_table_state.select(Some(i));
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if album_count > 0 {
                                    let i = match self.album_table_state.selected() {
                                        Some(i) => {
                                            if i >= album_count - 1 {
                                                0
                                            } else {
                                                i + 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    self.album_table_state.select(Some(i));
                                }
                            }
                            KeyCode::Enter => {
                                self.open_selected_album(false).await;
                            }
                            KeyCode::Char('p') | KeyCode::Char('P') => {
                                self.open_selected_album(true).await;
                            }
                            KeyCode::Char('a') => {
                                if let Some(idx) = self.album_table_state.selected() {
                                    if let Some(album) = self.search_results_albums.get(idx) {
                                        let browse_id = album.browse_id.clone();
                                        if let Ok((_, tracks)) = self.client.get_album_tracks(&browse_id).await {
                                            for t in tracks {
                                                let _ = send_daemon_command(&self.config, DaemonCommand::EnqueueTrack(t)).await;
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    ContentView::Artists => {
                        let artist_count = self.search_results_artists.len();
                        match key.code {
                            KeyCode::Left | KeyCode::Char('h') => {
                                self.active_panel = ActivePanel::Sidebar;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                if artist_count > 0 {
                                    let i = match self.artist_table_state.selected() {
                                        Some(i) => {
                                            if i == 0 {
                                                artist_count - 1
                                            } else {
                                                i - 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    self.artist_table_state.select(Some(i));
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if artist_count > 0 {
                                    let i = match self.artist_table_state.selected() {
                                        Some(i) => {
                                            if i >= artist_count - 1 {
                                                0
                                            } else {
                                                i + 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    self.artist_table_state.select(Some(i));
                                }
                            }
                            KeyCode::Enter => {
                                self.open_selected_artist(false).await;
                            }
                            KeyCode::Char('p') | KeyCode::Char('P') => {
                                self.open_selected_artist(true).await;
                            }
                            KeyCode::Char('a') => {
                                if let Some(idx) = self.artist_table_state.selected() {
                                    if let Some(artist) = self.search_results_artists.get(idx) {
                                        let browse_id = artist.browse_id.clone();
                                        if let Ok((_, tracks)) = self.client.get_artist_tracks(&browse_id).await {
                                            for t in tracks {
                                                let _ = send_daemon_command(&self.config, DaemonCommand::EnqueueTrack(t)).await;
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
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

            if self.show_help {
                self.show_help = false;
                return Ok(());
            }

            // Clicking top header: left side enters search, right side opens auth
            if mouse.row <= 2 {
                let search_width = (self.playerbar_area.width * 55) / 100;
                if mouse.column >= search_width {
                    self.show_auth_modal = true;
                    self.auth_input.clear();
                    self.auth_message = None;
                } else {
                    self.active_panel = ActivePanel::Search;
                }
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

        info!("Searching YouTube Music ({:?}) for query: {}", self.search_filter, q);
        match self.search_filter {
            SearchFilter::Tracks => {
                if let Ok(tracks) = self.client.search(&q).await {
                    if !tracks.is_empty() {
                        self.displayed_tracks = tracks.clone();
                        self.search_results = tracks;
                        self.content_view = ContentView::Tracks;
                        self.previous_view = None;
                        self.active_category_title = format!("Músicas: \"{}\"", q);
                        self.tracklist_state.select(Some(0));
                    }
                }
            }
            SearchFilter::Albums => {
                if let Ok(albums) = self.client.search_albums(&q).await {
                    if !albums.is_empty() {
                        self.search_results_albums = albums;
                        self.content_view = ContentView::Albums;
                        self.previous_view = None;
                        self.active_category_title = format!("Álbuns: \"{}\"", q);
                        self.album_table_state.select(Some(0));
                    }
                }
            }
            SearchFilter::Artists => {
                if let Ok(artists) = self.client.search_artists(&q).await {
                    if !artists.is_empty() {
                        self.search_results_artists = artists;
                        self.content_view = ContentView::Artists;
                        self.previous_view = None;
                        self.active_category_title = format!("Artistas: \"{}\"", q);
                        self.artist_table_state.select(Some(0));
                    }
                }
            }
        }
    }

    pub async fn open_selected_album(&mut self, play_immediately: bool) {
        if let Some(idx) = self.album_table_state.selected() {
            if let Some(album) = self.search_results_albums.get(idx) {
                let browse_id = album.browse_id.clone();
                let album_title = album.title.clone();
                info!("Opening album: {} ({})", album_title, browse_id);
                if let Ok((title, tracks)) = self.client.get_album_tracks(&browse_id).await {
                    if !tracks.is_empty() {
                        self.previous_view = Some((self.content_view, self.active_category_title.clone()));
                        self.displayed_tracks = tracks.clone();
                        self.active_category_title = format!("Álbum: \"{}\"", title);
                        self.content_view = ContentView::Tracks;
                        self.tracklist_state.select(Some(0));

                        if play_immediately {
                            let _ = send_daemon_command(
                                &self.config,
                                DaemonCommand::QueueTracks(tracks, 0),
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    pub async fn open_selected_artist(&mut self, play_immediately: bool) {
        if let Some(idx) = self.artist_table_state.selected() {
            if let Some(artist) = self.search_results_artists.get(idx) {
                let browse_id = artist.browse_id.clone();
                let artist_name = artist.name.clone();
                info!("Opening artist: {} ({})", artist_name, browse_id);
                if let Ok((name, tracks)) = self.client.get_artist_tracks(&browse_id).await {
                    if !tracks.is_empty() {
                        self.previous_view = Some((self.content_view, self.active_category_title.clone()));
                        self.displayed_tracks = tracks.clone();
                        self.active_category_title = format!("Artista: \"{}\"", name);
                        self.content_view = ContentView::Tracks;
                        self.tracklist_state.select(Some(0));

                        if play_immediately {
                            let _ = send_daemon_command(
                                &self.config,
                                DaemonCommand::QueueTracks(tracks, 0),
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    pub async fn start_radio_for_track(&mut self, video_id: &str, title: &str) {
        info!("Fetching YouTube Music radio station for: {}", video_id);
        if let Ok(tracks) = self.client.radio(video_id).await {
            if !tracks.is_empty() {
                self.displayed_tracks = tracks.clone();
                self.content_view = ContentView::Tracks;
                self.previous_view = None;
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
