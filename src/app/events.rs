use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{ActivePanel, App, ContentView};
use crate::audio::daemon::send_daemon_command;
use crate::audio::DaemonCommand;
use crate::i18n::LanguagePreference;

impl App {
    pub async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
        if self.show_help {
            if key.code == KeyCode::Esc
                || key.code == KeyCode::Char('?')
                || key.code == KeyCode::Char('q')
            {
                self.show_help = false;
            }
            return Ok(());
        }

        if self.show_auth_modal {
            match key.code {
                KeyCode::Esc => {
                    self.show_auth_modal = false;
                    self.is_oauth_polling = false;
                    self.oauth_user_code = None;
                }
                KeyCode::Char('o') | KeyCode::Char('O') if self.auth_input.is_empty() => {
                    self.start_oauth_flow();
                }
                KeyCode::Enter => {
                    if !self.auth_input.trim().is_empty() {
                        let value = self.auth_input.trim().to_string();
                        let sapisid = crate::innertube::auth::extract_sapisid_from_cookies(&value);
                        let mut new_auth = self.auth.clone();
                        new_auth.cookies = Some(value);
                        new_auth.sapisid = sapisid;
                        if let Err(e) = new_auth.save(&self.config.auth_file) {
                            self.auth_message =
                                Some((self.i18n.tf("auth.save_error", [("error", e)]), false));
                        } else {
                            self.auth = new_auth.clone();
                            self.client.auth = new_auth;
                            self.spawn_account_sync();
                            self.auth_message =
                                Some((self.i18n.t("auth.cookie_authenticated"), true));
                            self.auth_input.clear();
                        }
                    } else if !self.is_oauth_polling && self.oauth_user_code.is_none() {
                        self.start_oauth_flow();
                    }
                }
                KeyCode::Backspace => {
                    self.auth_input.pop();
                }
                KeyCode::Char('l') | KeyCode::Char('L') if self.auth_input.is_empty() => {
                    if self.auth.is_authenticated() {
                        let playback_clear = self.config.clear_playback_auth();
                        if playback_clear.is_ok() {
                            let _ = send_daemon_command(
                                &self.config,
                                DaemonCommand::PlaybackAuthChanged,
                            )
                            .await;
                        }
                        let _ = std::fs::remove_file(&self.config.auth_file);
                        self.auth = crate::innertube::auth::AuthConfig::default();
                        self.client.auth = self.auth.clone();
                        if playback_clear.is_ok() {
                            self.playback_auth = crate::config::PlaybackAuthSettings::default();
                        }
                        self.user_profile = None;
                        self.avatar_protocol = None;
                        self.current_avatar_image = None;
                        self.user_playlists.clear();
                        self.is_oauth_polling = false;
                        self.oauth_user_code = None;
                        self.auth_message = match playback_clear {
                            Ok(()) => Some((self.i18n.t("auth.disconnected"), true)),
                            Err(error) => Some((
                                self.i18n
                                    .tf("auth.playback_clear_error", [("error", error)]),
                                false,
                            )),
                        };
                    }
                }
                KeyCode::Char(c) => {
                    self.auth_input.push(c);
                }
                _ => {}
            }
            return Ok(());
        }

        if self.show_language_modal {
            match key.code {
                KeyCode::Esc => self.show_language_modal = false,
                KeyCode::Up | KeyCode::Char('k') => self.move_language_choice(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_language_choice(1),
                KeyCode::Enter => self.apply_language_choice(),
                _ => {}
            }
            return Ok(());
        }

        if key.code == KeyCode::F(2) {
            self.show_language_modal = true;
            self.language_choice = self.i18n.preference;
            return Ok(());
        }

        // Global refresh shortcut (F5 or Ctrl+R)
        if key.code == KeyCode::F(5)
            || (key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.refresh_all_data();
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
                    if let Ok(settings) = self.config.load_playback_auth() {
                        self.playback_auth = settings;
                    }
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
                KeyCode::Char('p')
                    if self.active_panel != ActivePanel::Tracklist
                        || self.content_view == ContentView::Tracks =>
                {
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
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    let _ = send_daemon_command(&self.config, DaemonCommand::ToggleShuffle).await;
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
            ActivePanel::Search => match key.code {
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
            },
            ActivePanel::Sidebar => {
                let total_items = 3 + self.user_playlists.len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if total_items > 0 {
                            let cur = self.sidebar_state.selected().unwrap_or(0);
                            let new_idx = if cur == 0 { total_items - 1 } else { cur - 1 };
                            self.select_sidebar_index(new_idx).await;
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if total_items > 0 {
                            let cur = self.sidebar_state.selected().unwrap_or(0);
                            let new_idx = if cur >= total_items - 1 { 0 } else { cur + 1 };
                            self.select_sidebar_index(new_idx).await;
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                        self.active_panel = ActivePanel::Tracklist;
                    }
                    _ => {}
                }
            }
            ActivePanel::Tracklist => match self.content_view {
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
                                    let now = std::time::Instant::now();
                                    let should_trigger = match self.last_track_click {
                                        Some((prev_idx, prev_time)) if prev_idx == idx => {
                                            now.duration_since(prev_time).as_millis() > 500
                                        }
                                        _ => true,
                                    };
                                    if should_trigger {
                                        self.last_track_click = Some((idx, now));
                                        let tracks = self.displayed_tracks.clone();
                                        let _ = send_daemon_command(
                                            &self.config,
                                            DaemonCommand::QueueTracks(tracks, idx),
                                        )
                                        .await;
                                    }
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
                                    if let Ok((_, tracks)) =
                                        self.client.get_album_tracks(&browse_id).await
                                    {
                                        for t in tracks {
                                            let _ = send_daemon_command(
                                                &self.config,
                                                DaemonCommand::EnqueueTrack(t),
                                            )
                                            .await;
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
                                    if let Ok((_, tracks)) =
                                        self.client.get_artist_tracks(&browse_id).await
                                    {
                                        for t in tracks {
                                            let _ = send_daemon_command(
                                                &self.config,
                                                DaemonCommand::EnqueueTrack(t),
                                            )
                                            .await;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
        }

        Ok(())
    }

    fn move_language_choice(&mut self, delta: i32) {
        let choices = [
            LanguagePreference::Auto,
            LanguagePreference::PtBr,
            LanguagePreference::En,
        ];
        let current = choices
            .iter()
            .position(|choice| *choice == self.language_choice)
            .unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(choices.len() as i32) as usize;
        self.language_choice = choices[next];
    }

    fn apply_language_choice(&mut self) {
        self.i18n.set_preference(self.language_choice);
        self.config.settings.general.language = self.language_choice;
        let settings = self.config.settings.clone();
        if let Err(error) = self.config.save_settings(&settings) {
            self.auth_message = Some((
                format!("{}: {}", self.i18n.t("settings.changed"), error),
                false,
            ));
        }
        self.show_language_modal = false;
    }

    pub fn handle_paste(&mut self, text: &str) {
        if self.show_auth_modal {
            self.auth_input.push_str(text);
        } else if self.active_panel == ActivePanel::Search {
            self.search_input.push_str(text);
        }
    }
}
