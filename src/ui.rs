use crate::{
    app::{App, MenuAction, Modal},
    launchd::Action,
    model::{FIELD_NAMES, Form, Scope, State},
    mouse::{MouseMap, Target, Wheel},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    buffer::CellWidth,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Row, Table, Tabs, Wrap},
};

const BG: Color = Color::Rgb(17, 23, 34);
const PANEL: Color = Color::Rgb(23, 32, 46);
const FG: Color = Color::Rgb(219, 227, 239);
const MUTED: Color = Color::Rgb(139, 157, 181);
const ACCENT: Color = Color::Rgb(99, 218, 210);
const GREEN: Color = Color::Rgb(148, 219, 155);
const YELLOW: Color = Color::Rgb(245, 204, 122);
const RED: Color = Color::Rgb(243, 139, 150);
const LINE_NUMBER: Color = Color::Rgb(144, 162, 186);
const XML_TAG: Color = Color::Rgb(166, 202, 240);
const XML_KEY: Color = Color::Rgb(255, 255, 255);
const XML_STRING: Color = Color::Rgb(202, 229, 180);
const XML_NUMBER: Color = Color::Rgb(249, 216, 155);
const XML_META: Color = Color::Rgb(166, 184, 208);
fn style(color: Color) -> Style {
    Style::default().fg(color)
}
fn block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .border_style(style(if focused { ACCENT } else { MUTED }))
        .style(Style::default().fg(FG).bg(PANEL))
}
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
fn popup(f: &mut Frame, area: Rect, title: String) -> Rect {
    f.render_widget(Clear, area);
    let b = block(title, true);
    let inner = b.inner(area);
    f.render_widget(b, area);
    inner
}
fn text(f: &mut Frame, area: Rect, value: impl Into<String>, scroll: u16) {
    f.render_widget(
        Paragraph::new(clean(&value.into()))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.mouse = MouseMap::default();
    f.render_widget(Block::new().style(Style::default().bg(BG).fg(FG)), area);
    if area.width < 65 || area.height < 18 {
        f.render_widget(
            Paragraph::new("ltui · Enlarge the terminal to at least 65 × 18.\nPress q to quit.")
                .style(style(ACCENT))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(6),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(area);
    let header = Layout::horizontal([
        Constraint::Min(20),
        Constraint::Length(if app.backend.demo { 40 } else { 0 }),
    ])
    .split(rows[0]);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ltui ", style(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("/ launchd job manager", style(FG)),
        ])),
        header[0],
    );
    if app.backend.demo {
        f.render_widget(
            Paragraph::new("DEMO · read-only")
                .alignment(Alignment::Right)
                .style(style(ACCENT)),
            header[1],
        );
    }
    let labels = if area.width < 90 {
        [
            "User", "Shared", "Daemon", "A.Agent", "A.Daem", "Session", "System",
        ]
    } else {
        [
            "User",
            "Shared",
            "Daemons",
            "Apple agents",
            "Apple daemons",
            "Session",
            "System",
        ]
    };
    let mut tab_x = rows[1].x;
    for (index, label) in labels.iter().enumerate() {
        let width = Line::from(*label).width() as u16 + 1;
        app.mouse.hit(
            Rect::new(
                tab_x,
                rows[1].y,
                width.min(rows[1].right().saturating_sub(tab_x)),
                1,
            ),
            Target::Scope(Scope::ALL[index]),
        );
        tab_x += width + 1;
    }
    f.render_widget(
        Tabs::new(labels.to_vec())
            .select(Scope::ALL.iter().position(|s| *s == app.scope))
            .divider(" ")
            .highlight_style(style(BG).bg(ACCENT).add_modifier(Modifier::BOLD))
            .style(style(MUTED))
            .padding("", " "),
        rows[1],
    );
    let running = app
        .jobs
        .iter()
        .filter(|j| matches!(j.state, State::Running(_)))
        .count();
    let disabled = app.jobs.iter().filter(|j| j.disabled == Some(true)).count();
    let search = if app.searching {
        format!(" / {}▏", app.filter)
    } else if !app.filter.is_empty() {
        format!(" / {}  · Esc clears", app.filter)
    } else {
        " / search jobs".into()
    };
    let info = Layout::horizontal([Constraint::Min(15), Constraint::Length(36)]).split(rows[2]);
    app.mouse.key(
        Rect::new(info[0].x, info[0].y, info[0].width, 1),
        KeyCode::Char('/'),
    );
    f.render_widget(
        Paragraph::new(search).style(style(if app.searching { ACCENT } else { MUTED })),
        info[0],
    );
    f.render_widget(
        Paragraph::new(format!(
            "{} jobs · {running} running · {disabled} disabled ",
            app.visible.len()
        ))
        .alignment(Alignment::Right)
        .style(style(MUTED)),
        info[1],
    );
    let panes = if area.width >= 110 {
        Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).split(rows[3])
    } else {
        // Borders, header, and header margin use four rows. Always leave
        // room for a selected job, plus a bordered detail pane below it.
        let job_height = ((u32::from(rows[3].height) * 48 / 100) as u16).max(5);
        Layout::vertical([Constraint::Length(job_height), Constraint::Min(3)]).split(rows[3])
    };
    draw_jobs(f, app, panes[0]);
    draw_detail(f, app, panes[1]);
    let notice = if app.warnings.is_empty() {
        app.notice.clone()
    } else {
        format!(
            "{} · ? for details",
            app.warnings[0]
                .lines()
                .next()
                .unwrap_or("Status unavailable")
        )
    };
    f.render_widget(
        Paragraph::new(clean(&format!(" {notice}"))).style(style(if app.warnings.is_empty() {
            MUTED
        } else {
            YELLOW
        })),
        rows[4],
    );
    let guide = Line::from(vec![
        Span::styled(" n", style(ACCENT)),
        Span::raw(" new  "),
        Span::styled("Space", style(ACCENT)),
        Span::raw(" actions  "),
        Span::styled("Enter", style(ACCENT)),
        Span::raw(" edit plist  "),
        Span::styled("R", style(ACCENT)),
        Span::raw(" refresh  "),
        Span::styled("?", style(ACCENT)),
        Span::raw(" help  "),
        Span::styled("q", style(ACCENT)),
        Span::raw(" quit"),
    ]);
    if !app.warnings.is_empty() {
        app.mouse.key(rows[4], KeyCode::Char('?'));
    }
    f.render_widget(Paragraph::new(guide), rows[5]);
    draw_modal(f, app);
}

fn draw_jobs(f: &mut Frame, app: &mut App, area: Rect) {
    app.page_size = area.height.saturating_sub(4).max(1) as usize;
    app.mouse.wheel(area, Wheel::Jobs);
    let b = block(
        format!(
            " {} · {} ",
            app.scope.title(),
            app.scope.domain(&app.backend.agent_domain)
        ),
        true,
    );
    if app.visible.is_empty() {
        let inner = b.inner(area);
        f.render_widget(b, area);
        text(
            f,
            inner,
            if app.filter.is_empty() {
                "\n No jobs in this scope.\n Press n to create one, or Tab to switch scopes."
            } else {
                "\n No matching jobs. Press Esc to clear the filter."
            },
            0,
        );
        return;
    }
    let detached = app
        .job_scroll
        .map(|offset| offset.min(app.visible.len().saturating_sub(app.page_size)));
    let rows = app
        .visible
        .iter()
        .skip(detached.unwrap_or(0))
        .take(if detached.is_some() {
            app.page_size
        } else {
            usize::MAX
        })
        .map(|index| {
            let j = &app.jobs[*index];
            let name = format!(
                "{}{}",
                if j.error.is_some() { "! " } else { "" },
                clean(&j.label)
            );
            let runtime_color = match j.state {
                State::Running(_) => GREEN,
                State::Waiting => ACCENT,
                State::Unloaded => MUTED,
                State::Unknown => YELLOW,
            };
            Row::new(vec![
                ratatui::widgets::Cell::from(name).style(style(if j.error.is_some() {
                    RED
                } else {
                    FG
                })),
                ratatui::widgets::Cell::from(j.state.text()).style(style(runtime_color)),
                ratatui::widgets::Cell::from(j.enabled_text()).style(style(
                    if j.disabled == Some(true) {
                        YELLOW
                    } else {
                        MUTED
                    },
                )),
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Min(14),
            Constraint::Length(18),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(["JOB", "RUNTIME / PID", "POLICY"])
            .style(style(MUTED))
            .bottom_margin(1),
    )
    .block(b)
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(39, 64, 81))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("› ")
    .column_spacing(1);
    let offset = if let Some(offset) = detached {
        // A separate render state lets the viewport move without pulling the
        // selected job back into view or changing its plist preview.
        let selected = app
            .table
            .selected()
            .filter(|i| *i >= offset && *i < offset + app.page_size)
            .map(|i| i - offset);
        let mut view = ratatui::widgets::TableState::default().with_selected(selected);
        f.render_stateful_widget(
            table.highlight_spacing(ratatui::widgets::HighlightSpacing::Always),
            area,
            &mut view,
        );
        *app.table.offset_mut() = offset;
        app.job_scroll = Some(offset);
        offset
    } else {
        f.render_stateful_widget(table, area, &mut app.table);
        app.table.offset()
    };
    for row in 0..area.height.saturating_sub(4) {
        let index = offset + row as usize;
        if index >= app.visible.len() {
            break;
        }
        app.mouse.hit(
            Rect::new(
                area.x + 1,
                area.y + 3 + row,
                area.width.saturating_sub(2),
                1,
            ),
            Target::Job(index),
        );
    }
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let b = block(" Plist · J/K scroll ", false);
    let inner = b.inner(area);
    app.preview_height = inner.height;
    app.mouse.wheel(area, Wheel::Plist);
    f.render_widget(b, area);
    if app.selected().is_some() {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let lines = highlight_plist(&clean(&app.detail_text));
        let digits = lines.len().max(1).to_string().len();
        let gutter_width = (digits as u16 + 2).min(inner.width.saturating_sub(1));
        let content = Rect::new(
            inner.x + gutter_width,
            inner.y,
            inner.width - gutter_width,
            inner.height,
        );
        let mut numbers = Vec::new();
        let mut wrapped = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let rows = hard_wrap_line(line, content.width);
            numbers.push(Line::from(format!(" {:>digits$} ", index + 1)));
            // Wrapped rows belong to the same source line; leave their gutter blank.
            numbers.extend((1..rows.len()).map(|_| Line::default()));
            wrapped.extend(rows);
        }
        // Use the same rows for rendering, numbering, and the scroll limit.
        // Stop at the last full viewport; files that fit cannot scroll.
        let max_scroll = wrapped
            .len()
            .saturating_sub(content.height as usize)
            .min(u16::MAX as usize) as u16;
        let scroll = app.detail_scroll.min(max_scroll);
        // Dialogs use this offset for their own content while they are open.
        if app.modal.is_none() {
            app.detail_scroll = scroll;
        }
        f.render_widget(
            Paragraph::new(numbers)
                .style(style(LINE_NUMBER))
                .scroll((scroll, 0)),
            Rect::new(inner.x, inner.y, gutter_width, inner.height),
        );
        f.render_widget(Paragraph::new(wrapped).scroll((scroll, 0)), content);
    } else {
        text(f, inner, "Select a job to view its plist.", 0);
    }
}

fn hard_wrap_line(line: &Line<'_>, width: u16) -> Vec<Line<'static>> {
    let mut rows = vec![Line::default()];
    let mut used = 0;
    for grapheme in line.styled_graphemes(Style::default()) {
        let cells = grapheme.symbol.cell_width();
        if cells > width {
            continue;
        }
        if used + cells > width {
            rows.push(Line::default());
            used = 0;
        }
        let spans = &mut rows.last_mut().unwrap().spans;
        if let Some(last) = spans.last_mut()
            && last.style == grapheme.style
        {
            last.content.to_mut().push_str(grapheme.symbol);
        } else {
            spans.push(Span::styled(grapheme.symbol.to_owned(), grapheme.style));
        }
        used += cells;
    }
    rows
}

/// Color the XML without changing its text or source-line boundaries.
fn highlight_plist(source: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    let mut offset = 0;
    let mut value_color = FG;
    while offset < source.len() {
        let rest = &source[offset..];
        let (length, color) = if rest.starts_with("<!--") {
            (rest.find("-->").map_or(rest.len(), |i| i + 3), XML_META)
        } else if rest.starts_with("<![CDATA[") {
            (rest.find("]]>").map_or(rest.len(), |i| i + 3), value_color)
        } else if rest.starts_with("<?") {
            (rest.find("?>").map_or(rest.len(), |i| i + 2), XML_META)
        } else if rest.starts_with('<') {
            let length = xml_tag_end(rest);
            let tag = &rest[..length];
            let name = tag
                .trim_start_matches('<')
                .trim_start_matches('/')
                .split([' ', '\t', '\n', '\r', '/', '>'])
                .next()
                .unwrap_or_default();
            let metadata = tag.starts_with("<!");
            if !metadata {
                value_color = if tag.starts_with("</") || tag.ends_with("/>") {
                    FG
                } else {
                    match name {
                        "key" => XML_KEY,
                        "string" => XML_STRING,
                        "integer" | "real" | "date" => XML_NUMBER,
                        _ => FG,
                    }
                };
            }
            (
                length,
                if metadata {
                    XML_META
                } else if matches!(name, "true" | "false") {
                    XML_NUMBER
                } else {
                    XML_TAG
                },
            )
        } else {
            (rest.find('<').unwrap_or(rest.len()), value_color)
        };
        for (index, part) in rest[..length].split('\n').enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            if !part.is_empty() {
                lines
                    .last_mut()
                    .unwrap()
                    .spans
                    .push(Span::styled(part.to_owned(), style(color)));
            }
        }
        offset += length;
    }
    // Match Paragraph's normal handling of a terminating newline.
    if source.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn xml_tag_end(text: &str) -> usize {
    let mut quote = None;
    let mut subset_depth: usize = 0;
    for (index, c) in text.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else {
            match c {
                '\'' | '"' => quote = Some(c),
                '[' => subset_depth += 1,
                ']' => subset_depth = subset_depth.saturating_sub(1),
                '>' if subset_depth == 0 => return index + 1,
                _ => {}
            }
        }
    }
    text.len()
}

fn draw_modal(f: &mut Frame, app: &mut App) {
    let Some(modal) = &app.modal else {
        return;
    };
    app.mouse = MouseMap::default();
    match modal {
        Modal::Help => {
            let inner = popup(f, centered(f.area(), 86, 34), " Keyboard shortcuts ".into());
            let mut value = "NAVIGATION\n  ↑/↓ or j/k     Select a job (also Ctrl-N/Ctrl-P)\n  Tab/Shift-Tab  Next / previous scope\n  /              Filter jobs · Esc clears the filter\n  Space          Open actions (edit, $EDITOR, logs, launchctl)\n  Enter          Open the selected plist in $EDITOR\n  Ctrl-U/Ctrl-D  Previous / next half page of jobs\n  PgUp/PgDn      Previous / next full page of jobs\n  J/K            Scroll the plist down/up by half its visible height\n  g/G            First / last job\n\nJOBS\n  n              Create a plist in the current scope\n  e              Edit common options (other keys are preserved)\n  l/u            Load / unload\n  s/r            Run now / restart\n  t              Toggle enabled / disabled\n  x              Unload and delete the plist (requires y confirmation)\n  R              Refresh (also automatically every 5 seconds)\n  q/Ctrl-C       Quit\n\nFORMS\n  Tab/Shift-Tab  Move fields (also Ctrl-N/Ctrl-P) · Ctrl-U clears\n  ↑/↓            Move between lines (between fields at edges)\n  ←/→ Home/End   Move within a line · Ctrl-Home/End whole field\n  Enter          Insert newline in arguments/calendar/environment\n  Ctrl-S         Save · Esc cancel\n\nSystem jobs require root for changes. Apple plists are read-only.\nEnter / Esc / ? closes help. PgUp/PgDn or Ctrl-U/Ctrl-D scrolls help.".to_string();
            if !app.warnings.is_empty() {
                value.push_str("\n\nINVENTORY WARNINGS\n");
                value.push_str(&app.warnings.join("\n\n"));
            }
            text(f, inner, value, app.detail_scroll);
            app.mouse.wheel(inner, Wheel::Dialog);
            text_hits(
                f,
                &mut app.mouse,
                inner,
                &[("Enter / Esc / ?", key(KeyCode::Esc))],
            );
        }
        Modal::Message(title, body) => {
            let inner = popup(f, centered(f.area(), 90, 22), format!(" {title} "));
            text(
                f,
                inner,
                format!("{body}\n\nEnter / Esc close · PgUp/PgDn scroll"),
                app.detail_scroll,
            );
            app.mouse.wheel(inner, Wheel::Dialog);
            text_hits(
                f,
                &mut app.mouse,
                inner,
                &[("Enter / Esc close", key(KeyCode::Esc))],
            );
        }
        Modal::Confirm(action, job) => {
            let inner = popup(
                f,
                centered(f.area(), 78, 15),
                format!(" {}? ", action.title()),
            );
            let value = format!(
                "{}\n\n{}\n\n{}\n\n{}\n\ny confirm · Enter / n / Esc cancel",
                job.target(),
                action.description(),
                job.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                if *action == Action::Delete {
                    "The plist will be permanently removed. Its executable and logs are kept."
                } else {
                    "This affects the selected service only."
                }
            );
            let footer_row = Paragraph::new(clean(&value))
                .wrap(Wrap { trim: false })
                .line_count(inner.width)
                .saturating_sub(1);
            text(f, inner, value, 0);
            // Only the actual footer is actionable, never matching words in
            // a job label or path. A clipped footer has no mouse targets.
            if footer_row < inner.height as usize {
                text_hits(
                    f,
                    &mut app.mouse,
                    Rect::new(inner.x, inner.y + footer_row as u16, inner.width, 1),
                    &[
                        ("y confirm", key(KeyCode::Char('y'))),
                        ("Enter / n / Esc cancel", key(KeyCode::Esc)),
                    ],
                );
            }
        }

        Modal::Actions(selected) => {
            let inner = popup(f, centered(f.area(), 86, 29), " Job actions ".into());
            let rows = Layout::vertical([Constraint::Min(6), Constraint::Length(5)]).split(inner);
            let selected_line = selected
                + if *selected < MenuAction::FILE_COUNT {
                    1
                } else {
                    3
                };
            let scroll = selected_line.saturating_sub(rows[0].height.saturating_sub(1) as usize);
            let mut lines = vec![Line::styled(" FILES", style(MUTED))];
            for (i, action) in MenuAction::ALL.iter().enumerate() {
                if i == MenuAction::FILE_COUNT {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled(" LAUNCHCTL", style(MUTED)));
                }
                lines.push(Line::styled(
                    format!(
                        " {} {}",
                        if i == *selected { "›" } else { " " },
                        action.title()
                    ),
                    if i == *selected {
                        style(BG).bg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        style(FG)
                    },
                ));
            }
            f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), rows[0]);
            app.mouse.wheel(inner, Wheel::Menu);
            for i in 0..MenuAction::ALL.len() {
                let line = i + if i < MenuAction::FILE_COUNT { 1 } else { 3 };
                if line >= scroll && line - scroll < rows[0].height as usize {
                    app.mouse.hit(
                        Rect::new(
                            rows[0].x,
                            rows[0].y + (line - scroll) as u16,
                            rows[0].width,
                            1,
                        ),
                        Target::Menu(i),
                    );
                }
            }
            text(
                f,
                rows[1],
                format!(
                    "{}\n\n↑↓ choose · Enter select · Esc close",
                    MenuAction::ALL[*selected].description()
                ),
                0,
            );
            text_hits(
                f,
                &mut app.mouse,
                rows[1],
                &[
                    ("Enter select", key(KeyCode::Enter)),
                    ("Esc close", key(KeyCode::Esc)),
                    ("↑", key(KeyCode::Up)),
                    ("↓", key(KeyCode::Down)),
                ],
            );
        }
        Modal::Form(form) => {
            let area = centered(f.area(), 100, 38);
            let title = if form.path.is_some() {
                " Edit job · other keys are preserved "
            } else {
                " New job · saved unloaded "
            };
            let inner = popup(f, area, title.into());
            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(4),
                Constraint::Length(if form.error.is_some() {
                    4
                } else {
                    form.field_help().lines().count() as u16
                }),
                Constraint::Length(2),
            ])
            .split(inner);
            let destination = form.path.clone().unwrap_or_else(|| app.create_dir());
            f.render_widget(
                Paragraph::new(clean(&format!(" {}", destination.display()))).style(style(MUTED)),
                rows[0],
            );
            app.mouse.wheel(inner, Wheel::Form);
            render_form_fields(f, form, rows[1], Some(&mut app.mouse));
            if let Some(error) = &form.error {
                f.render_widget(
                    Paragraph::new(clean(error))
                        .wrap(Wrap { trim: false })
                        .style(style(RED)),
                    rows[2],
                );
            } else {
                f.render_widget(
                    Paragraph::new(form.field_help())
                        .wrap(Wrap { trim: false })
                        .style(style(MUTED)),
                    rows[2],
                );
            }
            f.render_widget(Paragraph::new(" Ctrl-S save · Esc cancel · Tab/Ctrl-N next · Shift-Tab/Ctrl-P prev\n ↑↓ line/field · Enter newline in args/calendar/env · Home/End line").style(style(ACCENT)), rows[3]);
        }
    }
}

#[cfg(test)]
fn draw_form_fields(f: &mut Frame, form: &Form, area: Rect) {
    render_form_fields(f, form, area, None);
}

fn render_form_fields(f: &mut Frame, form: &Form, area: Rect, mut mouse: Option<&mut MouseMap>) {
    // Expand the active field and show a short preview of other multiline fields.
    let heights: Vec<u16> = form
        .fields
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let max_lines = if i == form.focus {
                (area.height / 2).max(1)
            } else {
                3
            };
            1 + value.split('\n').count().min(max_lines as usize) as u16
        })
        .collect();
    let mut start = 0;
    while start < form.focus && heights[start..=form.focus].iter().sum::<u16>() > area.height {
        start += 1;
    }
    let mut y = area.y;
    for i in start..form.fields.len() {
        let remaining = area.bottom().saturating_sub(y);
        if remaining < 2 {
            break;
        }
        let height = heights[i].min(remaining);
        let focused = form.focus == i;
        let value = &form.fields[i];
        let lines = value.split('\n').count();
        let mut label = format!(" {} {}", if focused { "›" } else { " " }, FIELD_NAMES[i]);
        let value_area = Rect::new(area.x + 3, y + 1, area.width.saturating_sub(3), height - 1);
        let (scroll, cursor) = if focused {
            // Only slice the active field at its cursor's UTF-8 byte boundary.
            let before = &value[..form.cursor];
            let row = before.bytes().filter(|b| *b == b'\n').count();
            let prefix = Line::from(clean(before.rsplit('\n').next().unwrap_or_default()));
            let column = prefix.width();
            if lines > 1 {
                label.push_str(&format!(" · line {}/{lines}", row + 1));
            }
            let top = row
                .saturating_sub(value_area.height.saturating_sub(1) as usize)
                .min(u16::MAX as usize);
            let desired_left = column.saturating_sub(value_area.width.saturating_sub(1) as usize);
            // Scroll at a grapheme boundary so a partially clipped wide character
            // cannot push the cursor beyond the right edge of the viewport.
            let mut left = 0;
            for grapheme in prefix.styled_graphemes(Style::default()) {
                if left >= desired_left {
                    break;
                }
                left += Line::from(grapheme.symbol).width();
            }
            let left = left.min(u16::MAX as usize);
            let x = column.saturating_sub(left);
            let y = row.saturating_sub(top);
            let cursor = if x < value_area.width as usize && y < value_area.height as usize {
                Some((value_area.x + x as u16, value_area.y + y as u16))
            } else {
                None
            };
            ((top as u16, left as u16), cursor)
        } else {
            if lines > 1 {
                label.push_str(&format!(" · {lines} lines"));
            }
            ((0, 0), None)
        };
        f.render_widget(
            Paragraph::new(label).style(style(if focused { ACCENT } else { MUTED })),
            Rect::new(area.x, y, area.width, 1),
        );
        f.render_widget(
            Paragraph::new(clean(value))
                .scroll(scroll)
                .style(if focused {
                    style(FG).bg(Color::Rgb(39, 64, 81))
                } else {
                    style(FG)
                }),
            value_area,
        );
        if let Some(position) = cursor {
            f.set_cursor_position(position);
        }
        if let Some(mouse) = mouse.as_deref_mut() {
            mouse.hit(Rect::new(area.x, y, area.width, height), Target::Field(i));
            mouse.hit(
                value_area,
                Target::FieldText {
                    index: i,
                    top: scroll.0,
                    left: scroll.1,
                },
            );
        }
        y += height;
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

// Match the actual rendered cells, including clipping and wrapped dialog text.
fn text_hits(f: &mut Frame, mouse: &mut MouseMap, area: Rect, labels: &[(&str, KeyEvent)]) {
    let buffer = f.buffer_mut();
    for y in area.top()..area.bottom() {
        let mut text = String::new();
        let mut positions = Vec::new();
        for x in area.left()..area.right() {
            positions.push((text.len(), x));
            text.push_str(buffer[(x, y)].symbol());
        }
        positions.push((text.len(), area.right()));
        for (label, key) in labels {
            for (start, _) in text.match_indices(label) {
                let Some((_, x)) = positions.iter().find(|(byte, _)| *byte == start) else {
                    continue;
                };
                let end = positions
                    .iter()
                    .find(|(byte, _)| *byte >= start + label.len())
                    .unwrap()
                    .1;
                mouse.hit(Rect::new(*x, y, end - x, 1), Target::Key(*key));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    fn screen(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    #[test]
    fn multiline_field_expands_and_scrolls_to_the_cursor_without_covering_other_fields() {
        let mut form = Form::new();
        form.fields[2] = "/bin/bash -c '\nfirst-line\nsecond-line\nthird-line\nlast-line\n'".into();
        form.focus(2);
        for height in [8, 24] {
            let mut terminal = Terminal::new(TestBackend::new(95, height)).unwrap();
            terminal
                .draw(|f| draw_form_fields(f, &form, f.area()))
                .unwrap();
            let rendered = screen(&terminal);
            assert!(rendered.contains("last-line"));
            let cursor = terminal.backend().cursor_position();
            assert!(terminal.backend().cursor_visible());
            assert_eq!(
                terminal.backend().buffer()[(cursor.x - 1, cursor.y)].symbol(),
                "'"
            );
            assert_eq!(terminal.backend().buffer()[cursor].symbol(), " ");
            assert!(rendered.contains("line 6/6"));
            if height == 24 {
                assert!(rendered.contains("first-line"));
                assert!(rendered.contains("second-line"));
                assert!(rendered.contains("WorkingDirectory"));
            }
            form.cursor = 0;
            terminal
                .draw(|f| draw_form_fields(f, &form, f.area()))
                .unwrap();
            assert!(screen(&terminal).contains("/bin/bash"));
            let cursor = terminal.backend().cursor_position();
            assert_eq!(terminal.backend().buffer()[cursor].symbol(), "/");
            form.cursor = form.fields[2].len();
        }
    }
    #[test]
    fn form_cursor_does_not_slice_unfocused_unicode_fields() {
        let mut form = Form::new();
        form.fields[3] = "界界界".into();
        form.cursor = 1;
        let mut terminal = Terminal::new(TestBackend::new(95, 24)).unwrap();
        terminal
            .draw(|f| draw_form_fields(f, &form, f.area()))
            .unwrap();
        assert_eq!(screen(&terminal).matches('界').count(), 3);
        form.focus(2);
        form.fields[2] = "界".repeat(80);
        form.cursor = form.fields[2].len();
        terminal
            .draw(|f| draw_form_fields(f, &form, f.area()))
            .unwrap();
        let cursor = terminal.backend().cursor_position();
        assert!(terminal.backend().cursor_visible());
        assert!(cursor.x >= 3 && cursor.x < 95);
        assert_eq!(terminal.backend().buffer()[cursor].symbol(), " ");
    }
    #[test]
    fn moving_the_form_cursor_does_not_change_the_rendered_text() {
        let mut form = Form::new();
        let mut terminal = Terminal::new(TestBackend::new(95, 24)).unwrap();
        form.cursor = 5;
        terminal
            .draw(|f| draw_form_fields(f, &form, f.area()))
            .unwrap();
        let before = terminal.backend().buffer().clone();
        terminal.backend_mut().assert_cursor_position((8, 1));
        assert_eq!(before[(8, 1)].symbol(), ".");
        form.right();
        terminal
            .draw(|f| draw_form_fields(f, &form, f.area()))
            .unwrap();
        terminal.backend_mut().assert_cursor_position((9, 1));
        assert_eq!(terminal.backend().buffer(), &before);
        assert_eq!(terminal.backend().buffer()[(9, 1)].symbol(), "m");
        assert!(!screen(&terminal).contains('▏'));
    }
    #[test]
    fn calendar_and_environment_fields_show_format_help() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut form = Form::new();
        form.focus(9);
        app.modal = Some(Modal::Form(Box::new(form)));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let rendered = screen(&terminal);
        assert!(rendered.contains("StartCalendarInterval"));
        assert!(rendered.contains("Hour=23 Minute=31"));
        assert!(!rendered.contains("ThrottleInterval"));
        assert!(!rendered.contains("UserName"));
        assert!(!rendered.contains("GroupName"));
        let Some(Modal::Form(form)) = &mut app.modal else {
            unreachable!()
        };
        form.focus(10);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let rendered = screen(&terminal);
        assert!(rendered.contains("MESSAGE='hello world'"));
        assert!(rendered.contains("separated by spaces or newlines"));
    }
    #[test]
    fn renders_all_views_and_modals_at_supported_sizes() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        for (w, h) in [(160, 45), (100, 32), (65, 18), (30, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            app.load_detail();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            for modal in [
                Modal::Help,
                Modal::Actions(0),
                Modal::Form(Box::new(crate::model::Form::new())),
                Modal::Confirm(Action::Delete, Box::new(app.jobs[0].clone())),
                Modal::Message("Error".into(), "Test failure".into()),
            ] {
                app.modal = Some(modal);
                terminal.draw(|f| draw(f, &mut app)).unwrap();
            }
            app.modal = None;
        }
    }
    #[test]
    fn supported_terminal_sizes_show_the_selected_job_in_the_table() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        for width in [65, 80, 109, 110, 160] {
            for height in 18..=24 {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                for selected in [0, app.visible.len() - 1] {
                    app.table.select(Some(selected));
                    app.load_detail();
                    let label = app.selected().unwrap().label.clone();
                    terminal.draw(|f| draw(f, &mut app)).unwrap();
                    // Require the table's selection marker, since the detail
                    // pane also displays the selected job's label.
                    let visible = terminal
                        .backend()
                        .buffer()
                        .content
                        .chunks(width as usize)
                        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                        .any(|line| line.contains(&format!("› {label}")));
                    assert!(
                        visible,
                        "Selected job {label} is invisible at {width}x{height}"
                    );
                }
            }
        }
    }
    #[test]
    fn right_panel_is_plist_and_actions_have_file_and_launchctl_sections() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|f| draw_detail(f, &mut app, f.area()))
            .unwrap();
        let rendered = screen(&terminal);
        assert!(rendered.contains("<?xml"));
        assert!(rendered.contains("<key>Label</key>"));
        assert!(!rendered.contains("COMMAND / ARGUMENTS"));
        app.modal = Some(Modal::Actions(0));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let rendered = screen(&terminal);
        assert!(rendered.find("FILES").unwrap() < rendered.find("Edit with $EDITOR").unwrap());
        assert!(rendered.find("Open stderr log").unwrap() < rendered.find("LAUNCHCTL").unwrap());
        assert!(rendered.contains("Reload plist"));
        let mut small = Terminal::new(TestBackend::new(65, 18)).unwrap();
        app.modal = Some(Modal::Actions(MenuAction::ALL.len() - 1));
        small.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&small).contains("› Delete job"));
    }
    #[test]
    fn plist_scrolling_stops_at_the_last_full_view() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        app.detail_text = "first line\nlast line".into();
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        app.detail_scroll = u16::MAX;
        terminal
            .draw(|f| draw_detail(f, &mut app, f.area()))
            .unwrap();
        assert_eq!(app.detail_scroll, 0);
        assert!(screen(&terminal).contains("last line"));
        app.handle(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE));
        terminal
            .draw(|f| draw_detail(f, &mut app, f.area()))
            .unwrap();
        assert_eq!(app.detail_scroll, 0);
        app.handle(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::NONE));
        terminal
            .draw(|f| draw_detail(f, &mut app, f.area()))
            .unwrap();
        assert_eq!(app.detail_scroll, 0);
        assert!(screen(&terminal).contains("first line"));

        app.detail_text = "abcdefghijABCDEFGHIJ0123456789".into();
        app.detail_scroll = u16::MAX;
        let mut narrow = Terminal::new(TestBackend::new(15, 4)).unwrap();
        narrow.draw(|f| draw_detail(f, &mut app, f.area())).unwrap();
        assert_eq!(app.detail_scroll, 1);
        assert!(screen(&narrow).contains("0123456789"));
        let mut wide = Terminal::new(TestBackend::new(35, 8)).unwrap();
        wide.draw(|f| draw_detail(f, &mut app, f.area())).unwrap();
        assert_eq!(app.detail_scroll, 0);
        assert!(screen(&wide).contains(&app.detail_text));

        app.detail_text = (0..10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.detail_scroll = u16::MAX;
        let mut short = Terminal::new(TestBackend::new(30, 6)).unwrap();
        for _ in 0..2 {
            short.draw(|f| draw_detail(f, &mut app, f.area())).unwrap();
            assert_eq!(app.detail_scroll, 6);
            let bottom: String = short
                .backend()
                .buffer()
                .content
                .chunks(30)
                .nth(4)
                .unwrap()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(bottom.contains("line 9"));
            app.handle(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE));
        }
        let mut tall = Terminal::new(TestBackend::new(30, 14)).unwrap();
        tall.draw(|f| draw_detail(f, &mut app, f.area())).unwrap();
        assert_eq!(app.detail_scroll, 0);
    }
    #[test]
    fn plist_highlighting_preserves_source_and_distinguishes_xml_tokens() {
        let source = "<?xml version=\"1.0\"?>\n<!-- <key>comment\ncontinued -->\n<key>Name</key>\n<string>hello &amp; world</string>\n<integer>42</integer>\n<true/>\n";
        let lines = highlight_plist(source);
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text, source.strip_suffix('\n').unwrap());
        for (token, color) in [
            ("<key>", XML_TAG),
            ("Name", XML_KEY),
            ("hello &amp; world", XML_STRING),
            ("42", XML_NUMBER),
            ("<true/>", XML_NUMBER),
            ("continued -->", XML_META),
        ] {
            assert!(
                lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .any(|span| span.content == token && span.style.fg == Some(color)),
                "Missing style for {token}"
            );
        }
        let quoted = "<plist note=\"a > b\">\n<key>Real</key>\n<string><![CDATA[a < b]]></string>";
        let lines = highlight_plist(quoted);
        assert_eq!(lines[0].spans[0].content, "<plist note=\"a > b\">");
        assert!(
            lines[2].spans.iter().any(
                |span| span.content == "<![CDATA[a < b]]>" && span.style.fg == Some(XML_STRING)
            )
        );
    }
    #[test]
    fn plist_gutter_numbers_source_lines_and_leaves_wrapped_rows_blank() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        app.detail_text = "abcdefghijABCDEFGHIJ0123456789\nlast".into();
        // Border (2) + gutter (3) leave ten cells for each wrapped row.
        let mut terminal = Terminal::new(TestBackend::new(15, 8)).unwrap();
        terminal
            .draw(|f| draw_detail(f, &mut app, f.area()))
            .unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(2, 1)].symbol(), "1");
        assert_eq!(buf[(2, 1)].fg, LINE_NUMBER);
        assert!(!buf[(2, 1)].modifier.contains(Modifier::DIM));
        assert_eq!(buf[(2, 2)].symbol(), " ");
        assert_eq!(buf[(2, 3)].symbol(), " ");
        assert_eq!(buf[(2, 4)].symbol(), "2");
        app.detail_scroll = 2;
        let mut terminal = Terminal::new(TestBackend::new(15, 4)).unwrap();
        terminal
            .draw(|f| draw_detail(f, &mut app, f.area()))
            .unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(2, 1)].symbol(), " ");
        assert_eq!(buf[(2, 2)].symbol(), "2");
        assert!(screen(&terminal).contains("0123456789"));
    }
    #[test]
    fn plist_wraps_at_the_right_edge_without_preserving_word_boundaries() {
        let line = Line::styled("   alpha beta gamma", style(XML_STRING));
        let rows = hard_wrap_line(&line, 10);
        let text: Vec<_> = rows
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(text, ["   alpha b", "eta gamma"]);
        assert!(
            rows.iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style.fg == Some(XML_STRING))
        );
        let rows = hard_wrap_line(&Line::raw("abc界de\u{301}f"), 5);
        let text: Vec<_> = rows
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(text, ["abc界", "de\u{301}f"]);
    }
    #[test]
    fn plist_half_page_scrolling_follows_the_visible_height_after_resizing() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        app.detail_text = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        for (height, step) in [(14, 6), (8, 3), (3, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(30, height)).unwrap();
            terminal
                .draw(|f| draw_detail(f, &mut app, f.area()))
                .unwrap();
            app.handle(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE));
            terminal
                .draw(|f| draw_detail(f, &mut app, f.area()))
                .unwrap();
            assert_eq!(app.detail_scroll, step);
            let top: String = terminal
                .backend()
                .buffer()
                .content
                .chunks(30)
                .nth(1)
                .unwrap()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(top.contains(&format!("line {step}")));
            app.handle(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::NONE));
            assert_eq!(app.detail_scroll, 0);
        }
    }
}
