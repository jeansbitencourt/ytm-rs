use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::{ActivePanel, App, ContentView};
use crate::audio::daemon::send_daemon_command;
use crate::audio::DaemonCommand;
use crate::ui::playerbar::{get_player_button_at, PlayerButton};

impl App {
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
                    PlayerButton::Shuffle => {
                        let _ =
                            send_daemon_command(&self.config, DaemonCommand::ToggleShuffle).await;
                    }
                    PlayerButton::VolDown => {
                        let _ =
                            send_daemon_command(&self.config, DaemonCommand::VolumeDelta(-5)).await;
                    }
                    PlayerButton::VolUp => {
                        let _ =
                            send_daemon_command(&self.config, DaemonCommand::VolumeDelta(5)).await;
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

            // Check if search filter buttons were clicked
            if let Some(filter) = crate::ui::get_search_filter_button_at_for_language(
                mouse.column,
                mouse.row,
                self.header_search_area,
                &self.i18n,
            ) {
                self.search_filter = filter;
                self.active_panel = ActivePanel::Search;
                if !self.search_input.trim().is_empty() {
                    self.perform_search().await;
                }
                return Ok(());
            }

            // Clicking top header: left side enters search, right side opens auth
            if mouse.row <= 3 {
                let search_width = if self.header_search_area.width > 0 {
                    self.header_search_area.x + self.header_search_area.width
                } else {
                    (self.playerbar_area.width * 56) / 100
                };
                if mouse.column >= search_width {
                    self.show_auth_modal = true;
                    self.auth_input.clear();
                    self.auth_message = None;
                } else {
                    self.active_panel = ActivePanel::Search;
                }
                return Ok(());
            }

            // Clicking on sidebar items
            if self.sidebar_area.width > 0
                && mouse.column >= self.sidebar_area.left()
                && mouse.column < self.sidebar_area.right()
                && mouse.row > self.sidebar_area.top()
                && mouse.row < self.sidebar_area.bottom()
            {
                self.active_panel = ActivePanel::Sidebar;
                let rel_y = mouse.row.saturating_sub(self.sidebar_area.top() + 1);
                let total_items = 3 + self.user_playlists.len();
                let clicked_idx = self.sidebar_state.offset() + rel_y as usize;
                if clicked_idx < total_items {
                    self.select_sidebar_index(clicked_idx).await;
                }
                return Ok(());
            }

            // Clicking on tracklist / album / artist card
            if self.tracklist_area.width > 0
                && mouse.column >= self.tracklist_area.left()
                && mouse.column < self.tracklist_area.right()
                && mouse.row > self.tracklist_area.top()
                && mouse.row < self.tracklist_area.bottom()
            {
                self.active_panel = ActivePanel::Tracklist;
                let rel_y = mouse.row.saturating_sub(self.tracklist_area.top() + 1);
                let card_offset = (rel_y / 4) as usize;

                match self.content_view {
                    ContentView::Tracks => {
                        if !self.displayed_tracks.is_empty() {
                            let card_idx = self.tracklist_state.offset() + card_offset;
                            if card_idx < self.displayed_tracks.len() {
                                let was_selected =
                                    self.tracklist_state.selected() == Some(card_idx);
                                self.tracklist_state.select(Some(card_idx));
                                if was_selected {
                                    let now = std::time::Instant::now();
                                    let should_trigger = match self.last_track_click {
                                        Some((prev_idx, prev_time)) if prev_idx == card_idx => {
                                            now.duration_since(prev_time).as_millis() > 500
                                        }
                                        _ => true,
                                    };

                                    if should_trigger {
                                        self.last_track_click = Some((card_idx, now));
                                        let tracks = self.displayed_tracks.clone();
                                        let _ = send_daemon_command(
                                            &self.config,
                                            DaemonCommand::QueueTracks(tracks, card_idx),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                    ContentView::Albums => {
                        if !self.search_results_albums.is_empty() {
                            let card_idx = self.album_table_state.offset() + card_offset;
                            if card_idx < self.search_results_albums.len() {
                                let was_selected =
                                    self.album_table_state.selected() == Some(card_idx);
                                self.album_table_state.select(Some(card_idx));
                                if was_selected {
                                    self.open_selected_album(false).await;
                                }
                            }
                        }
                    }
                    ContentView::Artists => {
                        if !self.search_results_artists.is_empty() {
                            let card_idx = self.artist_table_state.offset() + card_offset;
                            if card_idx < self.search_results_artists.len() {
                                let was_selected =
                                    self.artist_table_state.selected() == Some(card_idx);
                                self.artist_table_state.select(Some(card_idx));
                                if was_selected {
                                    self.open_selected_artist(false).await;
                                }
                            }
                        }
                    }
                }
                return Ok(());
            }
        } else if mouse.kind == MouseEventKind::ScrollUp {
            if self.sidebar_area.width > 0
                && mouse.column >= self.sidebar_area.left()
                && mouse.column < self.sidebar_area.right()
                && mouse.row >= self.sidebar_area.top()
                && mouse.row <= self.sidebar_area.bottom()
            {
                let cur = self.sidebar_state.selected().unwrap_or(0);
                if cur > 0 {
                    self.select_sidebar_index(cur - 1).await;
                }
                return Ok(());
            }

            match self.content_view {
                ContentView::Tracks => {
                    let cur = self.tracklist_state.selected().unwrap_or(0);
                    if cur > 0 {
                        self.tracklist_state.select(Some(cur - 1));
                    }
                }
                ContentView::Albums => {
                    let cur = self.album_table_state.selected().unwrap_or(0);
                    if cur > 0 {
                        self.album_table_state.select(Some(cur - 1));
                    }
                }
                ContentView::Artists => {
                    let cur = self.artist_table_state.selected().unwrap_or(0);
                    if cur > 0 {
                        self.artist_table_state.select(Some(cur - 1));
                    }
                }
            }
        } else if mouse.kind == MouseEventKind::ScrollDown {
            if self.sidebar_area.width > 0
                && mouse.column >= self.sidebar_area.left()
                && mouse.column < self.sidebar_area.right()
                && mouse.row >= self.sidebar_area.top()
                && mouse.row <= self.sidebar_area.bottom()
            {
                let total_items = 3 + self.user_playlists.len();
                let cur = self.sidebar_state.selected().unwrap_or(0);
                if cur + 1 < total_items {
                    self.select_sidebar_index(cur + 1).await;
                }
                return Ok(());
            }

            match self.content_view {
                ContentView::Tracks => {
                    if !self.displayed_tracks.is_empty() {
                        let cur = self.tracklist_state.selected().unwrap_or(0);
                        if cur + 1 < self.displayed_tracks.len() {
                            self.tracklist_state.select(Some(cur + 1));
                        }
                    }
                }
                ContentView::Albums => {
                    if !self.search_results_albums.is_empty() {
                        let cur = self.album_table_state.selected().unwrap_or(0);
                        if cur + 1 < self.search_results_albums.len() {
                            self.album_table_state.select(Some(cur + 1));
                        }
                    }
                }
                ContentView::Artists => {
                    if !self.search_results_artists.is_empty() {
                        let cur = self.artist_table_state.selected().unwrap_or(0);
                        if cur + 1 < self.search_results_artists.len() {
                            self.artist_table_state.select(Some(cur + 1));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
