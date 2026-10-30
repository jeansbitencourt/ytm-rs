use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Frame;

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

pub fn render_sidebar(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &mut ListState,
    is_focused: bool,
    counts: (usize, usize, usize),
) {
    let border_style = if is_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let title = Span::styled(
        " Biblioteca [1] ",
        theme.title_style(),
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .style(theme.default_block_style());

    let (liked_count, history_count, queue_count) = counts;
    let counts_arr = [liked_count, history_count, queue_count];

    let items: Vec<ListItem> = SidebarItem::ALL
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

    let list = List::new(items)
        .block(block)
        .highlight_style(theme.selected_row_style())
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, state);
}
