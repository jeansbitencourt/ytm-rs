use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, LineGauge, Paragraph};
use ratatui::Frame;

use crate::audio::{PlaybackStatus, PlayerState};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerButton {
    Prev,
    PlayPause,
    Next,
    Like,
    Radio,
    Shuffle,
    VolDown,
    VolUp,
    Account,
    Help,
}

pub fn get_player_button_at(col: u16, row: u16, playerbar_area: Rect) -> Option<PlayerButton> {
    let inner_y = playerbar_area.y + 1;
    let buttons_row = inner_y + 1;
    if row != buttons_row {
        return None;
    }

    let start_x = playerbar_area.x + 9;
    if col < start_x {
        return None;
    }

    let rel_x = col - start_x;
    if rel_x <= 9 {
        Some(PlayerButton::Prev)
    } else if (10..=20).contains(&rel_x) {
        Some(PlayerButton::PlayPause)
    } else if (21..=31).contains(&rel_x) {
        Some(PlayerButton::Next)
    } else if (32..=42).contains(&rel_x) {
        Some(PlayerButton::Like)
    } else if (43..=55).contains(&rel_x) {
        Some(PlayerButton::Radio)
    } else if (56..=68).contains(&rel_x) {
        Some(PlayerButton::Shuffle)
    } else if (69..=80).contains(&rel_x) {
        Some(PlayerButton::VolDown)
    } else if (81..=93).contains(&rel_x) {
        Some(PlayerButton::VolUp)
    } else if (94..=106).contains(&rel_x) {
        Some(PlayerButton::Account)
    } else if (107..=118).contains(&rel_x) {
        Some(PlayerButton::Help)
    } else {
        None
    }
}

const BLOCKS: [char; 9] = [' ', ' ', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn compute_visualizer_levels(status: PlaybackStatus, position_secs: f64) -> [u8; 6] {
    match status {
        PlaybackStatus::Playing => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let t = now * 3.2 + (position_secs * 0.1).sin() * 0.3;

            // Band 0: Sub-bass (~60 Hz kick/sub)
            let k0 = (t * 1.8).sin().max(0.0).powi(3);
            let s0 = (t * 0.9).cos() * 0.5 + 0.5;
            let e0 = 0.65 * k0 + 0.35 * s0;

            // Band 1: Bass (~150 Hz rhythm bass)
            let k1 = (t * 1.8 + 0.4).sin().max(0.0).powi(2);
            let s1 = (t * 2.7).sin() * 0.5 + 0.5;
            let e1 = 0.5 * k1 + 0.5 * s1;

            // Band 2: Low-mid (~400 Hz chords & body)
            let e2 = ((t * 2.2).cos() * 0.5 + 0.5) * 0.7 + ((t * 4.1).sin() * 0.3 + 0.3);

            // Band 3: Mid (~1 kHz melody & vocal core)
            let e3 = ((t * 3.1 + 1.0).sin() * 0.5 + 0.5) * 0.6 + ((t * 5.7).cos() * 0.4 + 0.4);

            // Band 4: High-mid (~3 kHz snare & presence)
            let k4 = (t * 3.6).sin().powi(4);
            let s4 = (t * 7.3).sin() * 0.5 + 0.5;
            let e4 = 0.4 * k4 + 0.6 * s4;

            // Band 5: Treble (~8 kHz hi-hat & shimmer)
            let e5 = (t * 7.2).sin().abs() * 0.5 + (t * 13.5).cos().abs() * 0.5;

            let energies = [e0, e1, e2, e3, e4, e5];
            let mut out = [0u8; 6];
            for (idx, &e) in energies.iter().enumerate() {
                let scaled = (e.clamp(0.05, 1.0) * 16.0).round() as u8;
                out[idx] = scaled.clamp(1, 16);
            }
            out
        }
        PlaybackStatus::Paused => [2, 3, 3, 3, 2, 1],
        PlaybackStatus::Buffering => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let phase = ((now * 5.0) as usize) % 6;
            let mut out = [2u8; 6];
            out[phase] = 12;
            if phase > 0 {
                out[phase - 1] = 7;
            }
            if phase + 1 < 6 {
                out[phase + 1] = 7;
            }
            out
        }
        PlaybackStatus::Stopped => [0, 0, 0, 0, 0, 0],
    }
}

fn render_audio_visualizer(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    player: &PlayerState,
) {
    if area.width < 6 || area.height < 2 {
        return;
    }

    let levels = compute_visualizer_levels(player.status, player.position_secs);

    // Row 0: Status badge / Title
    let row0 = match player.status {
        PlaybackStatus::Playing => {
            Line::from(vec![
                Span::styled(" ♫ ", theme.title_style()),
                Span::styled("LIVE ", theme.accent_style().add_modifier(Modifier::BOLD)),
            ])
        }
        PlaybackStatus::Paused => {
            Line::from(Span::styled(" ⏸ PAUS ", theme.muted_style()))
        }
        PlaybackStatus::Buffering => {
            Line::from(Span::styled(" ⏳ BUFF ", theme.accent_style()))
        }
        PlaybackStatus::Stopped => {
            Line::from(Span::styled(" ■ IDLE ", theme.muted_style()))
        }
    };

    // If area only has height 2:
    if area.height == 2 {
        let mut spans = vec![Span::raw(" ")];
        for &lvl in &levels {
            let single_lvl = ((lvl + 1) / 2).min(8) as usize;
            let ch = BLOCKS[single_lvl];
            let style = match player.status {
                PlaybackStatus::Playing => {
                    if single_lvl >= 7 {
                        theme.liked_style()
                    } else if single_lvl >= 5 {
                        Style::default().fg(theme.yellow).add_modifier(Modifier::BOLD)
                    } else {
                        theme.playing_indicator_style()
                    }
                }
                _ => theme.muted_style(),
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        spans.push(Span::raw(" "));
        let p = Paragraph::new(vec![row0, Line::from(spans)]);
        f.render_widget(p, area);
        return;
    }

    // Row 1 (Upper peaks) & Row 2 (Lower base):
    let mut upper_spans = vec![Span::raw(" ")];
    let mut lower_spans = vec![Span::raw(" ")];

    for &lvl in &levels {
        let (up_ch, up_style, low_ch, low_style) = if lvl == 0 {
            (' ', theme.muted_style(), ' ', theme.muted_style())
        } else if lvl <= 8 {
            let low_ch = BLOCKS[lvl as usize];
            let low_style = if player.status == PlaybackStatus::Playing {
                theme.playing_indicator_style()
            } else {
                theme.muted_style()
            };
            (' ', theme.muted_style(), low_ch, low_style)
        } else {
            let up_ch = BLOCKS[(lvl - 8) as usize];
            let up_style = if player.status == PlaybackStatus::Playing {
                if lvl >= 14 {
                    theme.liked_style()
                } else if lvl >= 11 {
                    Style::default().fg(theme.yellow).add_modifier(Modifier::BOLD)
                } else {
                    theme.accent_style()
                }
            } else {
                theme.muted_style()
            };
            let low_style = if player.status == PlaybackStatus::Playing {
                theme.playing_indicator_style()
            } else {
                theme.muted_style()
            };
            (up_ch, up_style, BLOCKS[8], low_style)
        };

        upper_spans.push(Span::styled(up_ch.to_string(), up_style));
        lower_spans.push(Span::styled(low_ch.to_string(), low_style));
    }
    upper_spans.push(Span::raw(" "));
    lower_spans.push(Span::raw(" "));

    let p = Paragraph::new(vec![
        row0,
        Line::from(upper_spans),
        Line::from(lower_spans),
    ]);
    f.render_widget(p, area);
}

pub fn render_playerbar(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    player: &PlayerState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .style(theme.default_block_style());

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 3 {
        return;
    }

    // Split horizontally: Left (Cover Icon / Art) | Right (Info & Controls)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(8), Constraint::Min(20)])
        .split(inner);

    // Left: Animated Bar Visualizer
    render_audio_visualizer(f, chunks[0], theme, player);

    // Right: 3 rows:
    // Row 0: Title — Artist [Codec tag]
    // Row 1: Interactive Buttons: [⏮ Prev] [⏯ Play] [⏭ Next] [♥ Like] [📻 Rádio] [🔉 Vol-] [🔊 Vol+] [❓ Ajuda]
    // Row 2: Progress LineGauge + Total Duration + Volume
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(chunks[1]);

    // Row 0: Title — Artist [Opus 160kbps • Background Daemon ON]
    let (title, artist) = if let Some(t) = &player.current_track {
        (t.title.as_str(), t.artist.as_str())
    } else {
        ("Nenhuma faixa em reprodução", "Selecione uma faixa e pressione Enter")
    };

    let status_icon = match player.status {
        PlaybackStatus::Playing => "▶ ",
        PlaybackStatus::Paused => "⏸ ",
        PlaybackStatus::Buffering => "⏳ ",
        PlaybackStatus::Stopped => "■ ",
    };

    let like_str = if player.is_liked { " ♥" } else { "" };
    let shuffle_tag = if player.shuffle_enabled { " • 🔀 Aleatório" } else { "" };

    let top_line = Line::from(vec![
        Span::styled(status_icon, theme.accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(title, theme.title_style()),
        Span::styled(like_str, theme.liked_style()),
        Span::styled(" — ", theme.muted_style()),
        Span::styled(artist, theme.text_style()),
        Span::styled(format!("  [Opus 160kbps • Daemon ON{}]", shuffle_tag), theme.muted_style()),
    ]);
    f.render_widget(Paragraph::new(top_line), right_chunks[0]);

    // Row 1: Interactive Buttons Row
    let play_label = if player.status == PlaybackStatus::Playing {
        "[⏸ Pause]"
    } else {
        "[▶ Play ]"
    };

    let like_style = if player.is_liked {
        theme.liked_style()
    } else {
        theme.muted_style()
    };

    let shuffle_style = if player.shuffle_enabled {
        theme.playing_indicator_style().add_modifier(Modifier::BOLD)
    } else {
        theme.muted_style()
    };

    let btn_style = theme.focused_border_style();

    let buttons_line = Line::from(vec![
        Span::styled("[⏮ Prev]  ", btn_style),
        Span::styled(format!("{}  ", play_label), theme.playing_indicator_style()),
        Span::styled("[⏭ Next]   ", btn_style),
        Span::styled("[♥ Like]   ", like_style),
        Span::styled("[📻 Rádio]   ", theme.accent_style()),
        Span::styled("[🔀 Aleat]   ", shuffle_style),
        Span::styled("[🔉 Vol-]  ", theme.text_style()),
        Span::styled("[🔊 Vol+]   ", theme.text_style()),
        Span::styled("[👤 Conta]   ", theme.accent_style()),
        Span::styled("[❓ Ajuda]", theme.muted_style()),
    ]);
    f.render_widget(Paragraph::new(buttons_line), right_chunks[1]);

    // Row 2: 01:42 ━━━●──────── 04:18  🔊 Vol: 85%
    let cur_secs = player.position_secs as u32;
    let dur_secs = player.duration_secs as u32;

    let cur_text = format!("{:02}:{:02}", cur_secs / 60, cur_secs % 60);
    let dur_text = format!("{:02}:{:02}", dur_secs / 60, dur_secs % 60);

    let progress_ratio = if dur_secs > 0 {
        (player.position_secs / player.duration_secs).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let progress_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(15),
            Constraint::Length(7),
            Constraint::Length(15),
        ])
        .split(right_chunks[2]);

    f.render_widget(Paragraph::new(cur_text).style(theme.muted_style()), progress_chunks[0]);

    let gauge = LineGauge::default()
        .filled_style(theme.accent_style().add_modifier(Modifier::BOLD))
        .unfilled_style(theme.muted_style())
        .ratio(progress_ratio)
        .line_set(ratatui::symbols::line::THICK);
    f.render_widget(gauge, progress_chunks[1]);

    f.render_widget(Paragraph::new(dur_text).style(theme.muted_style()), progress_chunks[2]);

    let vol_text = format!("🔊 Vol: {:>2}%", player.volume);
    f.render_widget(Paragraph::new(vol_text).style(theme.text_style()).alignment(Alignment::Right), progress_chunks[3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_button_hit_detection() {
        let area = Rect::new(0, 10, 130, 5);
        // buttons start at area.x + 9 = 9; row is 10 + 1 (border) + 1 (row 1) = 12
        assert_eq!(get_player_button_at(9, 12, area), Some(PlayerButton::Prev));
        assert_eq!(get_player_button_at(18, 12, area), Some(PlayerButton::Prev));
        assert_eq!(get_player_button_at(20, 12, area), Some(PlayerButton::PlayPause));
        assert_eq!(get_player_button_at(32, 12, area), Some(PlayerButton::Next));
        assert_eq!(get_player_button_at(44, 12, area), Some(PlayerButton::Like));
        assert_eq!(get_player_button_at(55, 12, area), Some(PlayerButton::Radio));
        assert_eq!(get_player_button_at(65, 12, area), Some(PlayerButton::Shuffle));
        assert_eq!(get_player_button_at(75, 12, area), Some(PlayerButton::Shuffle));
        assert_eq!(get_player_button_at(80, 12, area), Some(PlayerButton::VolDown));
        assert_eq!(get_player_button_at(92, 12, area), Some(PlayerButton::VolUp));
        // Account button is at rel_x 94..=106 => col 103..=115
        assert_eq!(get_player_button_at(105, 12, area), Some(PlayerButton::Account));
        assert_eq!(get_player_button_at(114, 12, area), Some(PlayerButton::Account));
        // Help button is at rel_x 107..=118 => col 116..=127
        assert_eq!(get_player_button_at(118, 12, area), Some(PlayerButton::Help));
        assert_eq!(get_player_button_at(125, 12, area), Some(PlayerButton::Help));
        // Wrong row
        assert_eq!(get_player_button_at(65, 11, area), None);
    }

    #[test]
    fn test_compute_visualizer_levels() {
        // Stopped returns all 0
        let stopped = compute_visualizer_levels(PlaybackStatus::Stopped, 0.0);
        assert_eq!(stopped, [0, 0, 0, 0, 0, 0]);

        // Paused returns steady baseline
        let paused = compute_visualizer_levels(PlaybackStatus::Paused, 10.0);
        assert_eq!(paused, [2, 3, 3, 3, 2, 1]);

        // Playing returns non-zero values within 1..=16
        let playing = compute_visualizer_levels(PlaybackStatus::Playing, 25.5);
        for &lvl in &playing {
            assert!(lvl >= 1 && lvl <= 16, "level should be in 1..=16, got {}", lvl);
        }

        // Buffering returns wave with peak
        let buffering = compute_visualizer_levels(PlaybackStatus::Buffering, 0.0);
        assert!(buffering.iter().any(|&l| l >= 10));
    }
}
