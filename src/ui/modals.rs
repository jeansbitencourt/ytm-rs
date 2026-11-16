use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::config::PlaybackAuthSource;
use crate::i18n::LanguagePreference;

fn text(app: &App, key: &str) -> String {
    app.i18n.t(key)
}

pub fn render_language_modal(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let width = 48.min(area.width.saturating_sub(4));
    let height = 10.min(area.height.saturating_sub(2));
    let modal_area = Rect::new(
        (area.width.saturating_sub(width)) / 2,
        (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.focused_border_style())
        .title(Span::styled(
            app.i18n.t("settings.title"),
            theme.title_style(),
        ))
        .style(theme.default_block_style());

    let choices = [
        LanguagePreference::Auto,
        LanguagePreference::PtBr,
        LanguagePreference::En,
    ];
    let mut lines = Vec::with_capacity(choices.len() + 2);
    for choice in choices {
        let marker = if choice == app.language_choice {
            "●"
        } else {
            "○"
        };
        let label = app.i18n.language_label(choice);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", marker),
                if choice == app.language_choice {
                    theme.accent_style()
                } else {
                    theme.muted_style()
                },
            ),
            Span::styled(label, theme.text_style()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        app.i18n.t("settings.apply"),
        theme.muted_style(),
    )));
    f.render_widget(Paragraph::new(lines).block(block), modal_area);
}

pub fn render_help_modal(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;

    // Centered modal calculation
    let width = 74.min(area.width.saturating_sub(4));
    let height = 27.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.focused_border_style())
        .title(Span::styled(app.i18n.t("help.title"), theme.title_style()))
        .style(theme.default_block_style());

    let shortcuts = [
        ("help.navigation_title", ""),
        ("help.nav_keys", "help.nav_desc"),
        ("help.focus_keys", "help.focus_desc"),
        ("help.filter_keys", "help.filter_desc"),
        ("help.open_keys", "help.open_desc"),
        ("help.back_keys", "help.back_desc"),
        ("", ""),
        ("help.playback_title", ""),
        ("help.space_keys", "help.space_desc"),
        ("help.album_keys", "help.album_desc"),
        ("help.next_keys", "help.next_desc"),
        ("help.volume_keys", "help.volume_desc"),
        ("help.like_keys", "help.like_desc"),
        ("help.video_keys", "help.video_desc"),
        ("help.shuffle_keys", "help.shuffle_desc"),
        ("help.queue_keys", "help.queue_desc"),
        ("help.radio_keys", "help.radio_desc"),
        ("help.sync_keys", "help.sync_desc"),
        ("", ""),
        ("help.window_title", ""),
        ("help.account_keys", "help.account_desc"),
        ("help.hide_keys", "help.hide_desc"),
        ("help.quit_keys", "help.quit_desc"),
        ("help.language_keys", "help.language_desc"),
        ("help.help_keys", "help.help_desc"),
    ];

    let lines: Vec<Line> = shortcuts
        .iter()
        .map(|(key, desc)| {
            if desc.is_empty() {
                Line::from(Span::styled(text(app, key), theme.title_style()))
            } else {
                Line::from(vec![
                    Span::styled(
                        format!("{:<24}", text(app, key)),
                        theme.accent_style().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text(app, desc), theme.text_style()),
                ])
            }
        })
        .collect();

    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, modal_area);
}

pub fn render_auth_modal(f: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;

    let width = 76.min(area.width.saturating_sub(4));
    let height = 20.min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.focused_border_style())
        .title(Span::styled(app.i18n.t("auth.title"), theme.title_style()))
        .style(theme.default_block_style());

    let inner_area = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // Library and playback auth status
            Constraint::Length(5), // Instructions
            Constraint::Length(3), // Input box
            Constraint::Length(2), // Feedback / Status Message
            Constraint::Length(2), // Shortcut footer
        ])
        .split(inner_area);

    // 1. Status Info
    let status_line = if app.auth.is_authenticated() {
        if let Some(prof) = &app.user_profile {
            let handle_disp = if !prof.handle.is_empty() {
                format!("── {} ({})", prof.name, prof.handle)
            } else {
                format!("── {}", prof.name)
            };
            let auth_type = if app.auth.has_oauth() {
                text(app, "auth.connected_oauth")
            } else {
                text(app, "auth.connected_cookie")
            };
            Line::from(vec![
                Span::styled(app.i18n.t("auth.status"), theme.title_style()),
                Span::styled(
                    auth_type,
                    theme.playing_indicator_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(handle_disp, theme.text_style()),
            ])
        } else if app.is_syncing_account {
            Line::from(vec![
                Span::styled(app.i18n.t("auth.status"), theme.title_style()),
                Span::styled(
                    text(app, "auth.syncing"),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ),
            ])
        } else if app.account_sync_error.is_some() {
            let err_desc = if app.auth.has_oauth() {
                text(app, "auth.oauth_rejected")
            } else {
                text(app, "auth.cookie_rejected")
            };
            Line::from(vec![
                Span::styled(app.i18n.t("auth.status"), theme.title_style()),
                Span::styled(
                    text(app, "auth.session_expired_label"),
                    theme.liked_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(err_desc, theme.muted_style()),
            ])
        } else {
            Line::from(vec![
                Span::styled(app.i18n.t("auth.status"), theme.title_style()),
                Span::styled(
                    text(app, "auth.connected_label"),
                    theme.playing_indicator_style().add_modifier(Modifier::BOLD),
                ),
            ])
        }
    } else {
        Line::from(vec![
            Span::styled(app.i18n.t("auth.status"), theme.title_style()),
            Span::styled(
                text(app, "auth.public_label"),
                theme.accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled(text(app, "auth.public_description"), theme.muted_style()),
        ])
    };

    let playback_source = match &app.playback_auth.source {
        PlaybackAuthSource::Disabled => text(app, "auth.playback_anonymous"),
        PlaybackAuthSource::Browser { browser, profile } => match profile {
            Some(profile) => app.i18n.tf(
                "auth.playback_browser_profile",
                [("browser", browser), ("profile", profile)],
            ),
            None => app.i18n.tf("auth.playback_browser", [("browser", browser)]),
        },
        PlaybackAuthSource::NetscapeCookieFile => text(app, "auth.playback_cookie_file"),
    };
    let playback_status = if app.playback_auth.source.is_enabled() {
        app.i18n
            .tf("auth.playback_configured", [("source", playback_source)])
    } else {
        text(app, "auth.playback_anonymous")
    };
    let p_status = Paragraph::new(vec![
        status_line,
        Line::from(vec![
            Span::styled(app.i18n.t("auth.playback_label"), theme.title_style()),
            Span::styled(playback_status, theme.text_style()),
        ]),
        Line::from(Span::styled(
            text(app, "auth.playback_note"),
            theme.muted_style(),
        )),
        Line::from(Span::styled(
            text(app, "auth.sync_description"),
            theme.text_style(),
        )),
    ]);
    f.render_widget(p_status, chunks[0]);

    // 2. Instructions / OAuth prompt
    let instr_lines = if let Some(code) = &app.oauth_user_code {
        let url = app
            .oauth_verification_url
            .as_deref()
            .unwrap_or("https://www.google.com/device");
        vec![
            Line::from(vec![
                Span::styled(text(app, "auth.device_login"), theme.title_style()),
                Span::styled(text(app, "auth.authorize_browser"), theme.text_style()),
            ]),
            Line::from(vec![
                Span::styled(text(app, "auth.open_label"), theme.muted_style()),
                Span::styled(
                    url,
                    theme
                        .accent_style()
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ),
            ]),
            Line::from(vec![
                Span::styled(text(app, "auth.code_label"), theme.muted_style()),
                Span::styled(
                    format!("[  {}  ]", code),
                    theme.playing_indicator_style().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                text(app, "auth.connect_allow"),
                theme.muted_style(),
            )),
            Line::from(Span::styled(
                text(app, "auth.waiting_approval"),
                theme.playing_indicator_style(),
            )),
        ]
    } else {
        vec![
            Line::from(vec![
                Span::styled(
                    text(app, "auth.start_login"),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ),
                Span::styled(text(app, "auth.generate_code"), theme.text_style()),
            ]),
            Line::from(Span::styled(
                text(app, "auth.paste_devtools"),
                theme.muted_style(),
            )),
            Line::from(Span::styled(
                text(app, "auth.open_music"),
                theme.muted_style(),
            )),
            Line::from(Span::styled(
                text(app, "auth.copy_cookie"),
                theme.muted_style(),
            )),
            Line::from(Span::styled(
                text(app, "auth.paste_cookie_instruction"),
                theme.muted_style(),
            )),
        ]
    };
    let p_instr = Paragraph::new(instr_lines);
    f.render_widget(p_instr, chunks[1]);

    // 3. Input Box
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.focused_border_style())
        .title(Span::styled(
            app.i18n.t("auth.cookie"),
            theme.accent_style(),
        ));

    let display_text = if app.auth_input.is_empty() {
        if app.is_oauth_polling {
            Span::styled(
                text(app, "auth.waiting_browser"),
                theme.playing_indicator_style(),
            )
        } else {
            Span::styled(text(app, "auth.prompt_cookie"), theme.muted_style())
        }
    } else {
        let char_count = app.auth_input.chars().count();
        let masked = if char_count > 46 {
            let prefix: String = app.auth_input.chars().take(36).collect();
            format!("{}... ({} caracteres) █", prefix, char_count)
        } else {
            format!("{} █", app.auth_input)
        };
        Span::styled(masked, theme.text_style())
    };

    let p_input = Paragraph::new(display_text).block(input_block);
    f.render_widget(p_input, chunks[2]);

    // 4. Feedback Message
    if let Some((msg, is_success)) = &app.auth_message {
        let style = if *is_success {
            theme.playing_indicator_style()
        } else {
            theme.accent_style()
        };
        let p_msg = Paragraph::new(Span::styled(msg.as_str(), style));
        f.render_widget(p_msg, chunks[3]);
    }

    // 5. Footer Shortcuts
    let footer_line = Line::from(vec![
        if app.is_oauth_polling {
            Span::styled(
                text(app, "auth.waiting_google"),
                theme.playing_indicator_style(),
            )
        } else {
            Span::styled(
                text(app, "auth.login_tv"),
                theme.accent_style().add_modifier(Modifier::BOLD),
            )
        },
        Span::styled(
            text(app, "auth.save_cookie"),
            theme.text_style().add_modifier(Modifier::BOLD),
        ),
        if app.auth.is_authenticated() {
            Span::styled(text(app, "auth.disconnect"), theme.liked_style())
        } else {
            Span::raw("")
        },
        Span::styled(text(app, "auth.close_window"), theme.muted_style()),
    ]);
    let p_footer = Paragraph::new(footer_line).alignment(Alignment::Right);
    f.render_widget(p_footer, chunks[4]);
}
