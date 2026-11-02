use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

use crate::audio::daemon::send_daemon_command;
use crate::audio::{DaemonCommand, DaemonResponse, PlayerState};
use crate::cache::Database;
use crate::config::AppConfig;
use crate::innertube::client::InnertubeClient;
use crate::innertube::{AlbumSummary, ArtistSummary, PlaylistSummary, SearchFilter, Track, UserProfile};
use crate::theme::Theme;
use crate::ui::sidebar::{SidebarEntry, SidebarItem};

pub mod actions;
pub mod events;
pub mod images;
pub mod mouse;

pub use images::detect_terminal_font_size;

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

pub struct AccountSyncResult {
    pub profile: Option<UserProfile>,
    pub liked_tracks: Vec<Track>,
    pub playlists: Vec<PlaylistSummary>,
    pub updated_auth: Option<crate::innertube::auth::AuthConfig>,
}

pub struct PlaylistLoadResult {
    pub playlist_id: String,
    pub title: String,
    pub tracks: Vec<Track>,
    pub updated_auth: Option<crate::innertube::auth::AuthConfig>,
}

#[derive(Debug, Clone)]
pub enum OAuthProgress {
    CodeReady { user_code: String, verification_url: String },
    TokenReady(crate::innertube::oauth::TokenResponse),
    Failed(String),
}

pub struct App {
    pub config: AppConfig,
    pub theme: Theme,
    pub active_panel: ActivePanel,
    pub content_view: ContentView,
    pub search_filter: SearchFilter,
    pub sidebar_state: ListState,
    pub tracklist_state: ListState,
    pub album_table_state: ListState,
    pub artist_table_state: ListState,
    pub active_sidebar_entry: SidebarEntry,
    pub active_category_title: String,
    pub displayed_tracks: Vec<Track>,
    pub liked_tracks: Vec<Track>,
    pub history_tracks: Vec<Track>,
    pub user_playlists: Vec<PlaylistSummary>,
    pub user_profile: Option<UserProfile>,
    pub is_syncing_account: bool,
    pub account_sync_error: Option<String>,
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
    pub oauth_user_code: Option<String>,
    pub oauth_verification_url: Option<String>,
    pub is_oauth_polling: bool,
    pub oauth_tx: UnboundedSender<OAuthProgress>,
    pub oauth_rx: UnboundedReceiver<OAuthProgress>,
    pub should_quit: bool,
    pub playerbar_area: Rect,
    pub header_search_area: Rect,
    pub tracklist_area: Rect,
    pub sidebar_area: Rect,
    pub is_loading_tracks: bool,
    pub current_loading_playlist_id: Option<String>,
    pub playlist_load_tx: UnboundedSender<PlaylistLoadResult>,
    pub playlist_load_rx: UnboundedReceiver<PlaylistLoadResult>,
    pub client: InnertubeClient,
    pub auth: crate::innertube::auth::AuthConfig,
    pub db: Arc<Mutex<Database>>,
    pub picker: Picker,
    pub cover_protocol: Option<Box<dyn StatefulProtocol>>,
    pub cover_video_id: Option<String>,
    pub current_cover_image: Option<(String, image::DynamicImage)>,
    pub cover_tx: UnboundedSender<(String, image::DynamicImage)>,
    pub cover_rx: UnboundedReceiver<(String, image::DynamicImage)>,
    pub thumb_protocols: HashMap<String, Box<dyn StatefulProtocol>>,
    pub thumb_loading: HashSet<String>,
    pub thumb_tx: UnboundedSender<(String, image::DynamicImage)>,
    pub thumb_rx: UnboundedReceiver<(String, image::DynamicImage)>,
    pub avatar_protocol: Option<Box<dyn StatefulProtocol>>,
    pub current_avatar_image: Option<image::DynamicImage>,
    pub avatar_tx: UnboundedSender<image::DynamicImage>,
    pub avatar_rx: UnboundedReceiver<image::DynamicImage>,
    pub account_sync_tx: UnboundedSender<AccountSyncResult>,
    pub account_sync_rx: UnboundedReceiver<AccountSyncResult>,
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

        let mut tracklist_state = ListState::default();
        tracklist_state.select(Some(0));

        let mut album_table_state = ListState::default();
        album_table_state.select(Some(0));

        let mut artist_table_state = ListState::default();
        artist_table_state.select(Some(0));

        let (liked, history, user_playlists, user_profile, last_sync_ts) = {
            let db_guard = db.lock().await;
            (
                db_guard.get_liked_tracks().unwrap_or_default(),
                db_guard.get_history(50).unwrap_or_default(),
                db_guard.get_playlists().unwrap_or_default(),
                db_guard.get_user_profile().unwrap_or_default(),
                db_guard.get_last_sync_ts().unwrap_or_default(),
            )
        };

        let cached_avatar = {
            let avatar_path = config.cache_dir.join("avatar.png");
            if avatar_path.exists() {
                image::open(&avatar_path).ok()
            } else {
                None
            }
        };

        let player_state = match send_daemon_command(&config, DaemonCommand::GetState).await {
            Ok(DaemonResponse::State(s)) => *s,
            _ => PlayerState::default(),
        };

        let has_active_playback = player_state.current_track.is_some()
            || player_state.status == crate::audio::PlaybackStatus::Playing
            || player_state.status == crate::audio::PlaybackStatus::Paused;

        let (active_sidebar_entry, displayed_tracks, active_category_title, sidebar_idx, track_idx) =
            if has_active_playback && !player_state.queue.is_empty() {
                let q_idx = player_state.queue_index.min(player_state.queue.len().saturating_sub(1));
                (
                    SidebarEntry::Fixed(SidebarItem::CurrentQueue),
                    player_state.queue.clone(),
                    "Fila / Rádio Atual".to_string(),
                    2,
                    q_idx,
                )
            } else {
                (
                    SidebarEntry::Fixed(SidebarItem::LikedTracks),
                    liked.clone(),
                    "Músicas Curtidas".to_string(),
                    0,
                    0,
                )
            };

        sidebar_state.select(Some(sidebar_idx));
        tracklist_state.select(Some(track_idx));

        let font_size = detect_terminal_font_size();
        let mut picker = match Picker::from_termios() {
            Ok(mut p) => {
                p.font_size = font_size;
                p
            }
            Err(_) => Picker::new(font_size),
        };
        picker.font_size = font_size;
        picker.guess_protocol();

        let avatar_protocol = cached_avatar
            .as_ref()
            .map(|img| picker.new_resize_protocol(img.clone()));

        let (cover_tx, cover_rx) = unbounded_channel();
        let (thumb_tx, thumb_rx) = unbounded_channel();
        let (avatar_tx, avatar_rx) = unbounded_channel();
        let (account_sync_tx, account_sync_rx) = unbounded_channel();
        let (oauth_tx, oauth_rx) = unbounded_channel();
        let (playlist_load_tx, playlist_load_rx) = unbounded_channel();

        let mut app = Self {
            config,
            theme,
            active_panel: ActivePanel::Tracklist,
            content_view: ContentView::Tracks,
            search_filter: SearchFilter::Tracks,
            sidebar_state,
            tracklist_state,
            album_table_state,
            artist_table_state,
            active_sidebar_entry,
            active_category_title,
            displayed_tracks,
            liked_tracks: liked,
            history_tracks: history,
            user_playlists,
            user_profile,
            is_syncing_account: false,
            account_sync_error: None,
            search_input: String::new(),
            search_results: Vec::new(),
            search_results_albums: Vec::new(),
            search_results_artists: Vec::new(),
            previous_view: None,
            is_searching: false,
            player_state,
            show_help: false,
            show_auth_modal: false,
            auth_input: String::new(),
            auth_message: None,
            oauth_user_code: None,
            oauth_verification_url: None,
            is_oauth_polling: false,
            oauth_tx,
            oauth_rx,
            should_quit: false,
            playerbar_area: Rect::default(),
            header_search_area: Rect::default(),
            tracklist_area: Rect::default(),
            sidebar_area: Rect::default(),
            is_loading_tracks: false,
            current_loading_playlist_id: None,
            playlist_load_tx,
            playlist_load_rx,
            client,
            auth,
            db,
            picker,
            cover_protocol: None,
            cover_video_id: None,
            current_cover_image: None,
            cover_tx,
            cover_rx,
            thumb_protocols: HashMap::new(),
            thumb_loading: HashSet::new(),
            thumb_tx,
            thumb_rx,
            avatar_protocol,
            current_avatar_image: cached_avatar,
            avatar_tx,
            avatar_rx,
            account_sync_tx,
            account_sync_rx,
        };

        // Dispara a sincronização de conta em background se autenticado e se TTL de 24h expirou
        // ou se ainda não temos playlists no cache
        const SYNC_TTL_SECS: u64 = 86_400; // 24 horas
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let should_sync = match last_sync_ts {
            Some(ts) => (now_ts.saturating_sub(ts) >= SYNC_TTL_SECS) || app.user_playlists.is_empty(),
            None => true,
        };

        if should_sync {
            app.spawn_account_sync();
        }

        Ok(app)
    }
}
