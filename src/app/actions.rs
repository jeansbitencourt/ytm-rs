use std::sync::Arc;
use tracing::info;

use super::{App, ContentView};
use crate::audio::daemon::send_daemon_command;
use crate::audio::DaemonCommand;
use crate::innertube::SearchFilter;
use crate::ui::sidebar::{SidebarEntry, SidebarItem};

impl App {
    pub async fn refresh_data(&mut self) {
        let db = self.db.lock().await;
        self.liked_tracks = db.get_liked_tracks().unwrap_or_default();
        self.history_tracks = db.get_history(50).unwrap_or_default();
        drop(db);

        if self.content_view == ContentView::Tracks && self.previous_view.is_none() {
            match self.active_sidebar_entry {
                SidebarEntry::Fixed(SidebarItem::LikedTracks) => {
                    self.displayed_tracks = self.liked_tracks.clone();
                    self.active_category_title = "Músicas Curtidas".to_string();
                }
                SidebarEntry::Fixed(SidebarItem::History) => {
                    self.displayed_tracks = self.history_tracks.clone();
                    self.active_category_title = "Histórico Recente".to_string();
                }
                SidebarEntry::Fixed(SidebarItem::CurrentQueue) => {
                    self.displayed_tracks = self.player_state.queue.clone();
                    self.active_category_title = "Fila / Rádio Atual".to_string();
                }
                SidebarEntry::Playlist(_) => {}
            }
        }
    }

    pub fn sync_queue_if_active(&mut self) {
        if self.content_view == ContentView::Tracks
            && self.active_sidebar_entry == SidebarEntry::Fixed(SidebarItem::CurrentQueue)
            && self.previous_view.is_none()
            && self.displayed_tracks != self.player_state.queue
        {
            self.displayed_tracks = self.player_state.queue.clone();
        }
    }

    pub fn spawn_account_sync(&mut self) {
        if !self.auth.is_authenticated() {
            return;
        }

        self.is_syncing_account = true;
        self.account_sync_error = None;

        let mut client = self.client.clone();
        let auth_file = self.config.auth_file.clone();
        let tx = self.account_sync_tx.clone();

        tokio::spawn(async move {
            // Auto refresh OAuth token if needed
            let _ = client.ensure_valid_token(Some(&auth_file)).await;

            let profile = client.get_user_profile().await.ok().flatten();
            let liked_tracks = client.get_liked_music().await.unwrap_or_default();
            let playlists = client.get_user_playlists().await.unwrap_or_default();

            let _ = tx.send(super::AccountSyncResult {
                profile,
                liked_tracks,
                playlists,
            });
        });
    }

    pub fn start_oauth_flow(&mut self) {
        if self.is_oauth_polling {
            return;
        }

        self.is_oauth_polling = true;
        self.oauth_user_code = None;
        self.oauth_verification_url = None;
        self.auth_message = Some(("⏳ Conectando ao Google...".to_string(), true));

        let tx = self.oauth_tx.clone();

        tokio::spawn(async move {
            let res = match crate::innertube::oauth::request_device_code(crate::innertube::oauth::DEFAULT_CLIENT_ID).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(super::OAuthProgress::Failed(format!("Erro ao solicitar código: {}", e)));
                    return;
                }
            };

            let _ = tx.send(super::OAuthProgress::CodeReady {
                user_code: res.user_code.clone(),
                verification_url: res.verification_url.clone(),
            });

            let poll_interval = std::time::Duration::from_secs(res.interval.max(5));
            let start = std::time::Instant::now();
            let timeout = std::time::Duration::from_secs(res.expires_in);

            while start.elapsed() < timeout {
                tokio::time::sleep(poll_interval).await;
                match crate::innertube::oauth::poll_token_once(
                    crate::innertube::oauth::DEFAULT_CLIENT_ID,
                    crate::innertube::oauth::DEFAULT_CLIENT_SECRET,
                    &res.device_code,
                ).await {
                    Ok(crate::innertube::oauth::PollResult::Success(tok)) => {
                        let _ = tx.send(super::OAuthProgress::TokenReady(tok));
                        return;
                    }
                    Ok(crate::innertube::oauth::PollResult::Pending) => {}
                    Ok(crate::innertube::oauth::PollResult::SlowDown) => {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                    Ok(crate::innertube::oauth::PollResult::Expired) => {
                        let _ = tx.send(super::OAuthProgress::Failed("O código expirou. Pressione 'o' para reiniciar.".to_string()));
                        return;
                    }
                    Ok(crate::innertube::oauth::PollResult::Error(err)) => {
                        let _ = tx.send(super::OAuthProgress::Failed(format!("Erro ao autorizar: {}", err)));
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(super::OAuthProgress::Failed(format!("Falha na rede: {}", e)));
                        return;
                    }
                }
            }
            let _ = tx.send(super::OAuthProgress::Failed("Tempo limite de autorização esgotado.".to_string()));
        });
    }

    pub async fn poll_oauth_progress(&mut self) {
        while let Ok(msg) = self.oauth_rx.try_recv() {
            match msg {
                super::OAuthProgress::CodeReady { user_code, verification_url } => {
                    self.oauth_user_code = Some(user_code);
                    self.oauth_verification_url = Some(verification_url);
                    self.auth_message = Some(("Código gerado! Digite o código no navegador.".to_string(), true));
                }
                super::OAuthProgress::TokenReady(tok) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    self.auth.oauth_token = Some(tok.access_token.clone());
                    if let Some(ref_tok) = tok.refresh_token {
                        self.auth.refresh_token = Some(ref_tok);
                    }
                    self.auth.token_expiry = Some(now + tok.expires_in);
                    let _ = self.auth.save(&self.config.auth_file);

                    self.client.auth = self.auth.clone();
                    self.is_oauth_polling = false;
                    self.oauth_user_code = None;
                    self.oauth_verification_url = None;
                    self.auth_message = Some(("✓ Conta Google conectada com sucesso!".to_string(), true));

                    self.spawn_account_sync();
                }
                super::OAuthProgress::Failed(err) => {
                    self.is_oauth_polling = false;
                    self.oauth_user_code = None;
                    self.oauth_verification_url = None;
                    self.auth_message = Some((err, false));
                }
            }
        }
    }

    pub async fn poll_account_sync(&mut self) {
        while let Ok(res) = self.account_sync_rx.try_recv() {
            self.is_syncing_account = false;

            if let Some(prof) = res.profile {
                self.account_sync_error = None;
                if !prof.avatar_url.is_empty() {
                    let should_download = self.user_profile.as_ref().map(|p| &p.avatar_url) != Some(&prof.avatar_url);
                    if should_download {
                        self.download_avatar(prof.avatar_url.clone());
                    }
                }
                self.user_profile = Some(prof);
            } else {
                self.account_sync_error = Some("Sessão ou cookie expirado no Google".to_string());
            }

            self.user_playlists = res.playlists;

            if !res.liked_tracks.is_empty() {
                let db_clone = Arc::clone(&self.db);
                let remote_liked = res.liked_tracks.clone();
                tokio::spawn(async move {
                    let db = db_clone.lock().await;
                    for t in &remote_liked {
                        let _ = db.add_liked_track(t);
                    }
                });

                self.liked_tracks = res.liked_tracks.clone();
                if self.active_sidebar_entry == SidebarEntry::Fixed(SidebarItem::LikedTracks)
                    && self.content_view == ContentView::Tracks
                    && self.previous_view.is_none()
                {
                    self.displayed_tracks = self.liked_tracks.clone();
                }
            }
        }
    }

    pub async fn select_sidebar_index(&mut self, idx: usize) {
        self.sidebar_state.select(Some(idx));
        if idx == 0 {
            self.active_sidebar_entry = SidebarEntry::Fixed(SidebarItem::LikedTracks);
            self.content_view = ContentView::Tracks;
            self.previous_view = None;
            self.displayed_tracks = self.liked_tracks.clone();
            self.active_category_title = "Músicas Curtidas".to_string();
            self.tracklist_state.select(Some(0));
        } else if idx == 1 {
            self.active_sidebar_entry = SidebarEntry::Fixed(SidebarItem::History);
            self.content_view = ContentView::Tracks;
            self.previous_view = None;
            self.displayed_tracks = self.history_tracks.clone();
            self.active_category_title = "Histórico Recente".to_string();
            self.tracklist_state.select(Some(0));
        } else if idx == 2 {
            self.active_sidebar_entry = SidebarEntry::Fixed(SidebarItem::CurrentQueue);
            self.content_view = ContentView::Tracks;
            self.previous_view = None;
            self.displayed_tracks = self.player_state.queue.clone();
            self.active_category_title = "Fila / Rádio Atual".to_string();
            let cur_idx = self.player_state.queue_index.min(self.displayed_tracks.len().saturating_sub(1));
            self.tracklist_state.select(Some(cur_idx));
        } else {
            let pl_idx = idx - 3;
            if let Some(pl) = self.user_playlists.get(pl_idx).cloned() {
                self.active_sidebar_entry = SidebarEntry::Playlist(pl_idx);
                self.content_view = ContentView::Tracks;
                self.previous_view = None;
                self.active_category_title = format!("Playlist: \"{}\"", pl.title);
                self.tracklist_state.select(Some(0));

                let client = self.client.clone();
                let pl_id = pl.id.clone();
                if let Ok((title, tracks)) = client.get_playlist_tracks(&pl_id).await {
                    if !tracks.is_empty() {
                        self.displayed_tracks = tracks;
                        self.active_category_title = format!("Playlist: \"{}\"", title);
                        self.tracklist_state.select(Some(0));
                    }
                }
            }
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
