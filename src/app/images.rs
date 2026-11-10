use std::time::Duration;

use super::{App, ContentView};

impl App {
    pub fn update_cover_if_needed(&mut self) {
        // Dynamically track terminal font/cell dimensions
        let new_font_size = detect_terminal_font_size();
        if new_font_size != self.picker.font_size {
            self.picker.font_size = new_font_size;
            if let Some((_, img)) = &self.current_cover_image {
                self.cover_protocol = Some(self.picker.new_resize_protocol(img.clone()));
            }
            if let Some(img) = &self.current_avatar_image {
                self.avatar_protocol = Some(self.picker.new_resize_protocol(img.clone()));
            }
            self.thumb_protocols.clear();
        }

        while let Ok((vid, img)) = self.cover_rx.try_recv() {
            if self
                .player_state
                .current_track
                .as_ref()
                .map(|t| &t.video_id)
                == Some(&vid)
            {
                self.current_cover_image = Some((vid.clone(), img.clone()));
                self.cover_protocol = Some(self.picker.new_resize_protocol(img));
            }
        }

        let current_track = self.player_state.current_track.as_ref();
        let current_vid = current_track.map(|t| t.video_id.as_str());

        if current_vid != self.cover_video_id.as_deref() {
            self.cover_video_id = current_vid.map(|s| s.to_string());
            self.cover_protocol = None;
            self.current_cover_image = None;

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
                            .timeout(Duration::from_secs(8))
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

    pub fn update_thumbnails_if_needed(&mut self) {
        // 1. Drain newly loaded thumbnails
        while let Ok((id, img)) = self.thumb_rx.try_recv() {
            self.thumb_loading.remove(&id);
            let proto = self.picker.new_resize_protocol(img);
            self.thumb_protocols.insert(id, proto);
        }

        // 2. Bound cache size
        if self.thumb_protocols.len() > 180 {
            self.thumb_protocols.clear();
        }

        // 3. Determine items to prefetch depending on content_view
        let items_to_fetch: Vec<(String, String)> = match self.content_view {
            ContentView::Tracks => {
                let len = self.displayed_tracks.len();
                if len == 0 {
                    Vec::new()
                } else {
                    let offset = self.tracklist_state.offset().min(len.saturating_sub(1));
                    let start = offset.saturating_sub(1);
                    let end = (offset + 25).min(len);
                    if start < end && start < len {
                        self.displayed_tracks[start..end]
                            .iter()
                            .filter(|t| !t.thumbnail_url.is_empty())
                            .map(|t| (t.video_id.clone(), t.thumbnail_url.clone()))
                            .collect()
                    } else {
                        Vec::new()
                    }
                }
            }
            ContentView::Albums => {
                let len = self.search_results_albums.len();
                if len == 0 {
                    Vec::new()
                } else {
                    let offset = self.album_table_state.offset().min(len.saturating_sub(1));
                    let start = offset.saturating_sub(1);
                    let end = (offset + 25).min(len);
                    if start < end && start < len {
                        self.search_results_albums[start..end]
                            .iter()
                            .filter(|a| !a.thumbnail_url.is_empty())
                            .map(|a| (a.browse_id.clone(), a.thumbnail_url.clone()))
                            .collect()
                    } else {
                        Vec::new()
                    }
                }
            }
            ContentView::Artists => {
                let len = self.search_results_artists.len();
                if len == 0 {
                    Vec::new()
                } else {
                    let offset = self.artist_table_state.offset().min(len.saturating_sub(1));
                    let start = offset.saturating_sub(1);
                    let end = (offset + 25).min(len);
                    if start < end && start < len {
                        self.search_results_artists[start..end]
                            .iter()
                            .filter(|a| !a.thumbnail_url.is_empty())
                            .map(|a| (a.browse_id.clone(), a.thumbnail_url.clone()))
                            .collect()
                    } else {
                        Vec::new()
                    }
                }
            }
        };

        for (id, url) in items_to_fetch {
            if self.thumb_protocols.contains_key(&id) || self.thumb_loading.contains(&id) {
                continue;
            }

            self.thumb_loading.insert(id.clone());
            let tx = self.thumb_tx.clone();

            tokio::spawn(async move {
                let fetch_url = if url.contains("=w") && url.contains("-h") {
                    let prefix = url.split("=w").next().unwrap_or(&url);
                    format!("{}=w120-h120-l90-rj", prefix)
                } else {
                    url.clone()
                };

                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(6))
                    .build()
                    .unwrap_or_default();

                let resp = match client.get(&fetch_url).send().await {
                    Ok(r) if r.status().is_success() => Ok(r),
                    _ => client.get(&url).send().await,
                };

                if let Ok(res) = resp {
                    if let Ok(bytes) = res.bytes().await {
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            let small_img = img.thumbnail(80, 80);
                            let _ = tx.send((id, small_img));
                        }
                    }
                }
            });
        }
    }

    pub fn update_avatar_if_needed(&mut self) {
        while let Ok(img) = self.avatar_rx.try_recv() {
            let avatar_path = self.config.cache_dir.join("avatar.png");
            let img_to_save = img.clone();
            tokio::spawn(async move {
                let _ = img_to_save.save(&avatar_path);
            });
            self.current_avatar_image = Some(img.clone());
            self.avatar_protocol = Some(self.picker.new_resize_protocol(img));
        }
    }

    pub fn download_avatar(&mut self, url: String) {
        if url.is_empty() {
            return;
        }

        let tx = self.avatar_tx.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .build()
                .unwrap_or_default();

            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            let small_img = img.thumbnail(80, 80);
                            let _ = tx.send(small_img);
                        }
                    }
                }
            }
        });
    }
}

pub fn detect_terminal_font_size() -> (u16, u16) {
    #[cfg(unix)]
    {
        use rustix::termios::tcgetwinsize;

        let fds = [
            rustix::stdio::stdout(),
            rustix::stdio::stdin(),
            rustix::stdio::stderr(),
        ];
        for fd in fds {
            if let Ok(ws) = tcgetwinsize(fd) {
                if ws.ws_xpixel > 0 && ws.ws_ypixel > 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                    let fx = ws.ws_xpixel / ws.ws_col;
                    let fy = ws.ws_ypixel / ws.ws_row;
                    if fx > 0 && fy > 0 {
                        return (fx, fy);
                    }
                }
            }
        }

        if let Ok(tty) = std::fs::File::open("/dev/tty") {
            use std::os::fd::AsFd;
            if let Ok(ws) = tcgetwinsize(tty.as_fd()) {
                if ws.ws_xpixel > 0 && ws.ws_ypixel > 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                    let fx = ws.ws_xpixel / ws.ws_col;
                    let fy = ws.ws_ypixel / ws.ws_row;
                    if fx > 0 && fy > 0 {
                        return (fx, fy);
                    }
                }
            }
        }
    }

    (7, 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::innertube::Track;

    #[tokio::test]
    async fn test_update_thumbnails_safe_when_offset_exceeds_len() {
        let config = AppConfig::load().unwrap();
        let mut app = App::new(config).await.unwrap();

        // 1. Tracks view with offset larger than empty list
        app.content_view = ContentView::Tracks;
        app.displayed_tracks.clear();
        *app.tracklist_state.offset_mut() = 50;
        app.update_thumbnails_if_needed(); // Must not panic!

        // 2. Tracks view with offset larger than non-empty short list
        app.displayed_tracks = vec![
            Track::new(
                "id1",
                "Title 1",
                "Artist",
                "Album",
                180,
                "https://thumb1.jpg",
            ),
            Track::new(
                "id2",
                "Title 2",
                "Artist",
                "Album",
                200,
                "https://thumb2.jpg",
            ),
        ];
        *app.tracklist_state.offset_mut() = 50;
        app.update_thumbnails_if_needed(); // Must not panic!

        // 3. Albums view with offset larger than empty list
        app.content_view = ContentView::Albums;
        app.search_results_albums.clear();
        *app.album_table_state.offset_mut() = 100;
        app.update_thumbnails_if_needed(); // Must not panic!

        // 4. Artists view with offset larger than empty list
        app.content_view = ContentView::Artists;
        app.search_results_artists.clear();
        *app.artist_table_state.offset_mut() = 100;
        app.update_thumbnails_if_needed(); // Must not panic!
    }
}
