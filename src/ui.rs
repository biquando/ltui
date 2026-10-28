use crate::{
    app::{App, Detail, Modal},
    launchd::Action,
    model::{FIELD_NAMES, Form, Scope, State},
};
use ratatui::{
    Frame,
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
        Constraint::Length(2),
    ])
    .split(area);
    let header = Layout::horizontal([Constraint::Min(20), Constraint::Length(40)]).split(rows[0]);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ltui ", style(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("/ launchd job manager", style(FG)),
        ])),
        header[0],
    );
    let mode = if app.backend.demo {
        "DEMO · read-only"
    } else {
        app.scope.domain(&app.backend.agent_domain)
    };
    f.render_widget(
        Paragraph::new(mode)
            .alignment(Alignment::Right)
            .style(style(ACCENT)),
        header[1],
    );
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
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" n", style(ACCENT)),
                Span::raw(" new  "),
                Span::styled("e", style(ACCENT)),
                Span::raw(" edit  "),
                Span::styled("E", style(ACCENT)),
                Span::raw(" $EDITOR  "),
                Span::styled("a", style(ACCENT)),
                Span::raw(" actions  "),
                Span::styled("t", style(ACCENT)),
                Span::raw(" enable/disable  "),
                Span::styled("?", style(ACCENT)),
                Span::raw(" help  "),
                Span::styled("q", style(ACCENT)),
                Span::raw(" quit"),
            ]),
            Line::styled(
                " Tab scope · ↑↓ select · ←→ detail tab · Enter focus detail · R refresh",
                style(MUTED),
            ),
        ]),
        rows[5],
    );
    draw_modal(f, app);
}

fn draw_jobs(f: &mut Frame, app: &mut App, area: Rect) {
    let b = block(
        format!(
            " {} · {} ",
            app.scope.title(),
            app.scope.domain(&app.backend.agent_domain)
        ),
        !app.focus_detail,
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
    let rows = app.visible.iter().map(|index| {
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
    f.render_stateful_widget(table, area, &mut app.table);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let b = block(format!(" {} · ← → ", app.detail.title()), app.focus_detail);
    let inner = b.inner(area);
    f.render_widget(b, area);
    if let Some(job) = app.selected() {
        if app.detail == Detail::Summary {
            let d = job
                .definition
                .as_ref()
                .and_then(plist::Value::as_dictionary);
            let mut value = format!(
                "{}\n\nRuntime    {}\nPolicy     {}{}\nLast exit  {}\nDomain     {}\n\nPLIST\n{}\n\nCOMMAND / ARGUMENTS\n{}",
                job.label,
                job.state.text(),
                job.enabled_text(),
                if job.disabled.is_none() {
                    " (status unavailable)"
                } else if job.override_disabled.is_some() {
                    " (launchctl override)"
                } else {
                    " (plist default)"
                },
                job.exit.as_deref().unwrap_or("—"),
                job.domain,
                job.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "No plist found · runtime service".into()),
                if job.command().is_empty() {
                    "Not available".into()
                } else {
                    job.command()
                }
            );
            if let Some(d) = d {
                value.push_str("\n\nOPTIONS\n");
                for key in [
                    "Program",
                    "RunAtLoad",
                    "KeepAlive",
                    "StartInterval",
                    "StartCalendarInterval",
                    "WorkingDirectory",
                    "UserName",
                    "GroupName",
                    "StandardOutPath",
                    "StandardErrorPath",
                ] {
                    if let Some(v) = d.get(key) {
                        let s = match v {
                            plist::Value::String(s) => s.clone(),
                            plist::Value::Boolean(b) => b.to_string(),
                            plist::Value::Integer(n) => n.to_string(),
                            _ => "configured · see Plist tab".into(),
                        };
                        value.push_str(&format!("{key}  {s}\n"));
                    }
                }
            }
            if let Some(error) = &job.error {
                value.push_str(&format!(
                    "\nPLIST ERROR\n{error}\nUse E to repair the full file."
                ));
            }
            if job.read_only() {
                value.push_str("\n\nApple system plist · file editing is read-only.");
            }
            value.push_str("\n\nEnable/disable controls future loads.\nLoad/unload controls registration now.\nEdits take effect when the job is reloaded.");
            text(f, inner, value, app.detail_scroll);
        } else {
            text(f, inner, app.detail_text.clone(), app.detail_scroll);
        }
    } else {
        text(f, inner, "\n Select a job to inspect its configuration.", 0);
    }
}

fn draw_modal(f: &mut Frame, app: &App) {
    let Some(modal) = &app.modal else {
        return;
    };
    match modal {
        Modal::Help => {
            let inner = popup(f, centered(f.area(), 86, 34), " Keyboard shortcuts ".into());
            let mut value = "NAVIGATION\n  ↑/↓ or j/k     Select a job (or scroll the focused detail pane)\n  Tab / Shift-Tab Next / previous scope\n  /              Filter by label, command, runtime, or enabled state\n  Esc            Clear filter\n  ←/→            Switch overview / plist / runtime / logs / launch reason\n  Enter          Focus the detail pane; Enter again returns to the list\n  PgUp/PgDn      Move or scroll one page · g/G first/last job\n\nJOBS\n  n              Create a plist in the current scope\n  e              Edit common options (other keys are preserved)\n  E              Edit full XML plist in $EDITOR, then validate and save\n  a              All launchctl actions and explanations\n  l / u          Load / unload\n  s / r          Run now / restart\n  t              Toggle enabled / disabled (independent of load state)\n  x              Unload and delete the plist (requires y confirmation)\n  p / o          Show plist / configured stdout and stderr log files\n  R              Refresh (also refreshes automatically every 5 seconds)\n  q / Ctrl-C     Quit\n\nFORM\n  Tab/Shift-Tab  Move between fields · Ctrl-U clear field\n  ↑/↓            Move between lines (between fields at edges)\n  ←/→ Home/End   Move within a line · Ctrl-Home/End whole field\n  Enter          Insert newline in arguments/calendar/environment\n  Ctrl-S         Save · Esc cancel\n\nSystem jobs require root for changes. Apple plists are read-only.\nPress Enter, Esc, or ? to close. PgUp/PgDn scroll this help.".to_string();
            if !app.warnings.is_empty() {
                value.push_str("\n\nINVENTORY WARNINGS\n");
                value.push_str(&app.warnings.join("\n\n"));
            }
            text(f, inner, value, app.detail_scroll);
        }
        Modal::Message(title, body) => {
            let inner = popup(f, centered(f.area(), 90, 22), format!(" {title} "));
            text(
                f,
                inner,
                format!("{body}\n\nEnter / Esc close · PgUp/PgDn scroll"),
                app.detail_scroll,
            );
        }
        Modal::Confirm(action, job) => {
            let inner = popup(
                f,
                centered(f.area(), 78, 15),
                format!(" {}? ", action.title()),
            );
            text(
                f,
                inner,
                format!(
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
                ),
                0,
            );
        }
        Modal::Actions(selected) => {
            let inner = popup(f, centered(f.area(), 86, 29), " Job actions ".into());
            let rows = Layout::vertical([Constraint::Min(6), Constraint::Length(5)]).split(inner);
            let start = selected.saturating_sub(rows[0].height.saturating_sub(1) as usize);
            let lines: Vec<Line> = Action::ALL
                .iter()
                .enumerate()
                .skip(start)
                .map(|(i, a)| {
                    Line::styled(
                        format!(" {} {}", if i == *selected { "›" } else { " " }, a.title()),
                        if i == *selected {
                            style(BG).bg(ACCENT).add_modifier(Modifier::BOLD)
                        } else {
                            style(FG)
                        },
                    )
                })
                .collect();
            f.render_widget(Paragraph::new(lines), rows[0]);
            text(
                f,
                rows[1],
                format!(
                    "{}\n\n↑↓ choose · Enter select · Esc close",
                    Action::ALL[*selected].description()
                ),
                0,
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
            draw_form_fields(f, form, rows[1]);
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
            f.render_widget(Paragraph::new(" Ctrl-S save · Esc cancel · Tab/Shift-Tab field · Ctrl-U clear\n ↑↓ line/field · Enter newline in args/calendar/env · Home/End line").style(style(ACCENT)), rows[3]);
        }
    }
}

fn draw_form_fields(f: &mut Frame, form: &Form, area: Rect) {
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
        y += height;
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
            for detail in Detail::ALL {
                app.detail = detail;
                app.load_detail();
                terminal.draw(|f| draw(f, &mut app)).unwrap();
            }
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
}
