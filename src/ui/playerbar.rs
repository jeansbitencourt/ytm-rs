use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
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
    } else if (56..=66).contains(&rel_x) {
        Some(PlayerButton::VolDown)
    } else if (67..=78).contains(&rel_x) {
        Some(PlayerButton::VolUp)
    } else if (79..=91).contains(&rel_x) {
        Some(PlayerButton::Account)
    } else if (92..=103).contains(&rel_x) {
        Some(PlayerButton::Help)
    } else {
        None
    }
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

    // Left: Music Note Icon / Thumbnail representation
    let icon_lines = vec![
        Line::from(Span::styled("  ♫ ", theme.title_style())),
        Line::from(Span::styled(" [YTM]", theme.muted_style())),
    ];
    let icon_p = Paragraph::new(icon_lines).alignment(Alignment::Center);
    f.render_widget(icon_p, chunks[0]);

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

    let top_line = Line::from(vec![
        Span::styled(status_icon, theme.accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(title, theme.title_style()),
        Span::styled(like_str, theme.liked_style()),
        Span::styled(" — ", theme.muted_style()),
        Span::styled(artist, theme.text_style()),
        Span::styled("  [Opus 160kbps • Daemon ON]", theme.muted_style()),
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

    let btn_style = theme.focused_border_style();

    let buttons_line = Line::from(vec![
        Span::styled("[⏮ Prev]  ", btn_style),
        Span::styled(format!("{}  ", play_label), theme.playing_indicator_style()),
        Span::styled("[⏭ Next]   ", btn_style),
        Span::styled("[♥ Like]   ", like_style),
        Span::styled("[📻 Rádio]   ", theme.accent_style()),
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
        let area = Rect::new(0, 10, 120, 5);
        // buttons start at area.x + 9 = 9; row is 10 + 1 (border) + 1 (row 1) = 12
        assert_eq!(get_player_button_at(9, 12, area), Some(PlayerButton::Prev));
        assert_eq!(get_player_button_at(18, 12, area), Some(PlayerButton::Prev));
        assert_eq!(get_player_button_at(20, 12, area), Some(PlayerButton::PlayPause));
        assert_eq!(get_player_button_at(32, 12, area), Some(PlayerButton::Next));
        assert_eq!(get_player_button_at(44, 12, area), Some(PlayerButton::Like));
        assert_eq!(get_player_button_at(55, 12, area), Some(PlayerButton::Radio));
        assert_eq!(get_player_button_at(68, 12, area), Some(PlayerButton::VolDown));
        assert_eq!(get_player_button_at(80, 12, area), Some(PlayerButton::VolUp));
        // Account button is at rel_x 79..=91 => col 88..=100
        assert_eq!(get_player_button_at(88, 12, area), Some(PlayerButton::Account));
        assert_eq!(get_player_button_at(95, 12, area), Some(PlayerButton::Account));
        assert_eq!(get_player_button_at(100, 12, area), Some(PlayerButton::Account));
        // Help button is at rel_x 92..=103 => col 101..=112
        assert_eq!(get_player_button_at(103, 12, area), Some(PlayerButton::Help));
        assert_eq!(get_player_button_at(110, 12, area), Some(PlayerButton::Help));
        // Wrong row
        assert_eq!(get_player_button_at(88, 11, area), None);
    }
}
