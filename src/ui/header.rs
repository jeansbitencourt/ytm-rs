use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use ratatui_image::StatefulImage;

use crate::app::{ActivePanel, App};
use crate::innertube::SearchFilter;

pub fn get_search_filter_button_at(col: u16, row: u16, search_area: Rect) -> Option<SearchFilter> {
    if row > search_area.y + 1 {
        return None;
    }
    let start_x = search_area.x + 1;
    if col < start_x + 27 {
        return None;
    }
    let rel_x = col - start_x;
    if (27..=40).contains(&rel_x) {
        Some(SearchFilter::Tracks)
    } else if (41..=52).contains(&rel_x) {
        Some(SearchFilter::Albums)
    } else if (53..=67).contains(&rel_x) {
        Some(SearchFilter::Artists)
    } else {
        None
    }
}

pub fn render_header(f: &mut Frame, search_area: Rect, status_area: Rect, app: &mut App) {
    let theme = &app.theme;

    let is_search_focused = app.active_panel == ActivePanel::Search;
    let search_border_style = if is_search_focused {
        theme.focused_border_style()
    } else {
        theme.unfocused_border_style()
    };

    let chip = |filter: SearchFilter| {
        let is_active = app.search_filter == filter;
        if is_active {
            Span::styled(format!("[{}]", filter.label()), theme.selected_row_style())
        } else {
            Span::styled(format!("[{}]", filter.label()), theme.muted_style())
        }
    };

    let title_line = Line::from(vec![
        Span::styled(" Busca [/] ── Filtro [Tab]: ", theme.title_style()),
        chip(SearchFilter::Tracks),
        Span::raw(" "),
        chip(SearchFilter::Albums),
        Span::raw(" "),
        chip(SearchFilter::Artists),
        Span::raw(" "),
    ]);

    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(search_border_style)
        .title(title_line)
        .style(theme.default_block_style());

    let search_text = if app.search_input.is_empty() {
        if is_search_focused {
            Span::styled("█", theme.accent_style())
        } else {
            Span::styled("digite para buscar músicas, álbuns ou artistas...", theme.muted_style())
        }
    } else {
        let cursor = if is_search_focused { "█" } else { "" };
        Span::styled(format!("{}{}", app.search_input, cursor), theme.text_style())
    };

    let search_line = Line::from(vec![
        Span::styled("🔍 ", theme.accent_style().add_modifier(Modifier::BOLD)),
        search_text,
    ]);

    let search_hint = Line::from(Span::styled(
        "Enter: Buscar | Tab: Músicas/Álbuns/Artistas | Esc: Fechar",
        theme.muted_style(),
    ));

    let search_p = Paragraph::new(vec![search_line, search_hint]).block(search_block);
    f.render_widget(search_p, search_area);

    // Right: Status indicator with Account info (Name & Avatar)
    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .title(Span::styled(" Perfil & Conexão [c] ", theme.title_style()))
        .style(theme.default_block_style());

    let inner_status = status_block.inner(status_area);
    f.render_widget(status_block, status_area);

    if inner_status.width < 10 || inner_status.height < 1 {
        return;
    }

    let has_avatar = app.avatar_protocol.is_some() && inner_status.width >= 24 && inner_status.height >= 2;

    if has_avatar {
        let avatar_rect = Rect {
            x: inner_status.x + 1,
            y: inner_status.y,
            width: 4,
            height: 2,
        };
        if let Some(protocol) = &mut app.avatar_protocol {
            let img = StatefulImage::new(None);
            f.render_stateful_widget(img, avatar_rect, protocol);
        }
    }

    let text_x = if has_avatar {
        inner_status.x + 6
    } else {
        inner_status.x + 1
    };
    let text_w = inner_status.width.saturating_sub(if has_avatar { 6 } else { 1 });
    let text_rect = Rect {
        x: text_x,
        y: inner_status.y,
        width: text_w,
        height: inner_status.height,
    };

    let account_line = if app.auth.is_authenticated() {
        if let Some(profile) = &app.user_profile {
            let handle_disp = if !profile.handle.is_empty() {
                format!(" ({})", profile.handle)
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled("👤 ", theme.playing_indicator_style()),
                Span::styled(&profile.name, theme.text_style().add_modifier(Modifier::BOLD)),
                Span::styled(handle_disp, theme.muted_style()),
            ])
        } else if app.is_syncing_account {
            Line::from(vec![
                Span::styled("⏳ Sincronizando perfil e biblioteca...", theme.accent_style()),
            ])
        } else if app.account_sync_error.is_some() {
            let update_hint = if app.auth.has_oauth() {
                "[c: Reconectar Google]"
            } else {
                "[c: Atualizar cookie]"
            };
            Line::from(vec![
                Span::styled("⚠️ Sessão expirada ", theme.liked_style().add_modifier(Modifier::BOLD)),
                Span::styled(update_hint, theme.accent_style()),
            ])
        } else {
            Line::from(vec![
                Span::styled("👤 Conectado ao YouTube Music", theme.playing_indicator_style().add_modifier(Modifier::BOLD)),
            ])
        }
    } else {
        Line::from(vec![
            Span::styled("👤 Modo Público ", theme.accent_style().add_modifier(Modifier::BOLD)),
            Span::styled("[c: Conectar Conta Google]", theme.muted_style()),
        ])
    };

    let system_line = Line::from(vec![
        Span::styled("● Daemon: ", theme.muted_style()),
        Span::styled("Ativo", theme.playing_indicator_style()),
        Span::styled(" | MPRIS2: ", theme.muted_style()),
        Span::styled("OK", theme.accent_style()),
        Span::styled(" | ?: Ajuda", theme.muted_style()),
    ]);

    let text_p = Paragraph::new(vec![account_line, system_line]).alignment(Alignment::Left);
    f.render_widget(text_p, text_rect);
}
