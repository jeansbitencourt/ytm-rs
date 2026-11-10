use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use ratatui_image::StatefulImage;

use crate::app::{ActivePanel, App};
use crate::i18n::I18n;
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

pub fn get_search_filter_button_at_for_language(
    col: u16,
    row: u16,
    search_area: Rect,
    i18n: &I18n,
) -> Option<SearchFilter> {
    if row > search_area.y + 1 {
        return None;
    }
    let start_x = search_area.x + 1;
    let mut cursor = start_x + display_width(&i18n.t("search.title")).saturating_sub(1);
    for filter in SearchFilter::ALL {
        let width = display_width(&format!("[{}]", filter.localized_label(i18n)));
        if col >= cursor && col < cursor.saturating_add(width) {
            return Some(filter);
        }
        cursor = cursor.saturating_add(width + 1);
    }
    None
}

fn display_width(value: &str) -> u16 {
    value
        .chars()
        .map(|ch| if ch as u32 >= 0x1f000 { 2 } else { 1 })
        .sum()
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
            Span::styled(
                format!("[{}]", filter.localized_label(&app.i18n)),
                theme.selected_row_style(),
            )
        } else {
            Span::styled(
                format!("[{}]", filter.localized_label(&app.i18n)),
                theme.muted_style(),
            )
        }
    };

    let title_line = Line::from(vec![
        Span::styled(app.i18n.t("search.title"), theme.title_style()),
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
            Span::styled(app.i18n.t("search.placeholder"), theme.muted_style())
        }
    } else {
        let cursor = if is_search_focused { "█" } else { "" };
        Span::styled(
            format!("{}{}", app.search_input, cursor),
            theme.text_style(),
        )
    };

    let search_line = Line::from(vec![
        Span::styled("🔍 ", theme.accent_style().add_modifier(Modifier::BOLD)),
        search_text,
    ]);

    let search_hint = Line::from(Span::styled(app.i18n.t("search.hint"), theme.muted_style()));

    let search_p = Paragraph::new(vec![search_line, search_hint]).block(search_block);
    f.render_widget(search_p, search_area);

    // Right: Status indicator with Account info (Name & Avatar)
    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.unfocused_border_style())
        .title(Span::styled(
            app.i18n.t("profile.title"),
            theme.title_style(),
        ))
        .style(theme.default_block_style());

    let inner_status = status_block.inner(status_area);
    f.render_widget(status_block, status_area);

    if inner_status.width < 10 || inner_status.height < 1 {
        return;
    }

    let has_avatar =
        app.avatar_protocol.is_some() && inner_status.width >= 24 && inner_status.height >= 2;

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
    let text_w = inner_status
        .width
        .saturating_sub(if has_avatar { 6 } else { 1 });
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
                Span::styled(
                    &profile.name,
                    theme.text_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(handle_disp, theme.muted_style()),
            ])
        } else if app.is_syncing_account {
            Line::from(vec![Span::styled(
                app.i18n.t("status.syncing"),
                theme.accent_style(),
            )])
        } else if app.account_sync_error.is_some() {
            let update_hint = if app.auth.has_oauth() {
                app.i18n.t("status.reconnect")
            } else {
                app.i18n.t("status.update_cookie")
            };
            Line::from(vec![
                Span::styled(
                    app.i18n.t("status.expired"),
                    theme.liked_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(update_hint, theme.accent_style()),
            ])
        } else {
            Line::from(vec![Span::styled(
                app.i18n.t("status.connected"),
                theme.playing_indicator_style().add_modifier(Modifier::BOLD),
            )])
        }
    } else {
        Line::from(vec![
            Span::styled(
                app.i18n.t("status.public"),
                theme.accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled(app.i18n.t("status.connect"), theme.muted_style()),
        ])
    };

    let (sync_text, sync_style) = if app.is_syncing_account {
        (
            app.i18n.t("status.sync"),
            theme.accent_style().add_modifier(Modifier::BOLD),
        )
    } else {
        (app.i18n.t("status.reload"), theme.muted_style())
    };

    let system_line = Line::from(vec![
        Span::styled(app.i18n.t("status.daemon"), theme.muted_style()),
        Span::styled(app.i18n.t("status.active"), theme.playing_indicator_style()),
        Span::styled(app.i18n.t("status.mpris"), theme.muted_style()),
        Span::styled(app.i18n.t("status.ok"), theme.accent_style()),
        Span::styled(sync_text, sync_style),
        Span::styled(app.i18n.t("status.help"), theme.muted_style()),
    ]);

    let text_p = Paragraph::new(vec![account_line, system_line]).alignment(Alignment::Left);
    f.render_widget(text_p, text_rect);
}
