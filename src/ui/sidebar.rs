use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::innertube::PlaylistSummary;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    LikedTracks,
    History,
    CurrentQueue,
}

impl SidebarItem {
    pub const ALL: [SidebarItem; 3] = [
        SidebarItem::LikedTracks,
        SidebarItem::History,
        SidebarItem::CurrentQueue,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SidebarItem::LikedTracks => "♥ Músicas Curtidas",
            SidebarItem::History => "🕒 Histórico Recente",
            SidebarItem::CurrentQueue => "📻 Fila / Rádio Atual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEntry {
    Fixed(SidebarItem),
    Playlist(usize),
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else if max_chars > 1 {
        let truncated: String = chars[..max_chars - 1].iter().collect();
        format!("{}…", truncated)
    } else if max_chars == 1 {
        chars[0].to_string()
    } else {
        String::new()
    }
}

pub fn render_sidebar(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &mut ListState,
    is_focused: bool,
    counts: (usize, usize, usize),
    playlists: &[PlaylistSummary],
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title_text = if playlists.is_empty() {
        " Biblioteca [1] "
    } else {
        " Biblioteca & Playlists [1] "
    };

    let title = Span::styled(title_text, theme.title_style());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let (liked_count, history_count, queue_count) = counts;
    let counts_arr = [liked_count, history_count, queue_count];

    let mut items: Vec<ListItem> = SidebarItem::ALL
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let count_str = format!(" ({})", counts_arr[i]);
            let line = Line::from(vec![
                Span::styled(item.label(), theme.text_style()),
                Span::styled(count_str, theme.muted_style()),
            ]);
            ListItem::new(line)
        })
        .collect();

    // Append user's playlists if available
    let inner_w = area.width.saturating_sub(4) as usize;
    for (idx, pl) in playlists.iter().enumerate() {
        let count_str = if !pl.count_text.is_empty() {
            format!(" ({})", pl.count_text)
        } else {
            String::new()
        };

        let avail_for_title = inner_w.saturating_sub(count_str.chars().count() + 3);
        let title_disp = truncate_str(&pl.title, avail_for_title);

        let icon = if idx == 0 { "♪ " } else { "♫ " };
        let line = Line::from(vec![
            Span::styled(icon, theme.accent_style()),
            Span::styled(title_disp, theme.text_style()),
            Span::styled(count_str, theme.muted_style()),
        ]);
        items.push(ListItem::new(line));
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, state);
}
