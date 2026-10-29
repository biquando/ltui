use crate::{
    app::{App, Effect, Modal},
    model::Scope,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, text::Line};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug)]
pub enum Target {
    Key(KeyEvent),
    Scope(Scope),
    Job(usize),
    Menu(usize),
    Field(usize),
    FieldText { index: usize, top: u16, left: u16 },
}
#[derive(Clone, Copy, Debug)]
pub enum Wheel {
    Jobs,
    Plist,
    Menu,
    Dialog,
    Form,
}
#[derive(Default)]
pub struct MouseMap {
    pub hits: Vec<(Rect, Target)>,
    pub wheels: Vec<(Rect, Wheel)>,
}
impl MouseMap {
    pub fn hit(&mut self, area: Rect, target: Target) {
        self.hits.push((area, target));
    }
    pub fn key(&mut self, area: Rect, code: KeyCode) {
        self.hit(area, Target::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }
    pub fn wheel(&mut self, area: Rect, wheel: Wheel) {
        self.wheels.push((area, wheel));
    }
}

impl App {
    pub fn handle_mouse(&mut self, event: MouseEvent) -> Effect {
        let position = (event.column, event.row).into();
        if matches!(
            event.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let Some((_, wheel)) = self
                .mouse
                .wheels
                .iter()
                .rev()
                .find(|(area, _)| area.contains(position))
            else {
                return Effect::None;
            };
            let down = event.kind == MouseEventKind::ScrollDown;
            match wheel {
                Wheel::Jobs => {
                    self.searching = false;
                    let offset = self.job_scroll.unwrap_or(self.table.offset());
                    let max = self.visible.len().saturating_sub(self.page_size);
                    self.job_scroll = Some(if down {
                        offset.saturating_add(1).min(max)
                    } else {
                        offset.saturating_sub(1).min(max)
                    });
                }
                Wheel::Plist | Wheel::Dialog => {
                    self.detail_scroll = if down {
                        self.detail_scroll.saturating_add(1)
                    } else {
                        self.detail_scroll.saturating_sub(1)
                    }
                }
                Wheel::Menu => {
                    if let Some(Modal::Actions(index)) = &mut self.modal {
                        *index = if down {
                            (*index + 1).min(crate::app::MenuAction::ALL.len() - 1)
                        } else {
                            index.saturating_sub(1)
                        };
                    }
                }
                Wheel::Form => {
                    return self.handle(KeyEvent::new(
                        KeyCode::Char(if down { 'n' } else { 'p' }),
                        KeyModifiers::CONTROL,
                    ));
                }
            }
            return Effect::None;
        }
        let MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) = event.kind
        else {
            return Effect::None;
        };
        let Some((area, target)) = self
            .mouse
            .hits
            .iter()
            .rev()
            .find(|(area, _)| area.contains(position))
            .cloned()
        else {
            return Effect::None;
        };
        if button == MouseButton::Right && !matches!(target, Target::Job(_)) {
            return Effect::None;
        }
        match target {
            Target::Key(key) => {
                if self.modal.is_none() {
                    self.searching = false;
                }
                return self.handle(key);
            }
            Target::Scope(scope) => {
                self.searching = false;
                if self.scope != scope {
                    self.scope = scope;
                    self.table.select(None);
                    self.job_scroll = None;
                    self.refresh();
                }
            }
            Target::Job(index) => {
                if index < self.visible.len() {
                    self.searching = false;
                    self.table.select(Some(index));
                    self.load_detail();
                    if button == MouseButton::Right {
                        self.modal = Some(Modal::Actions(0));
                    }
                }
            }
            Target::Menu(index) => {
                if let Some(Modal::Actions(selected)) = &mut self.modal {
                    *selected = index;
                    return self.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
            }
            Target::Field(index) => {
                if let Some(Modal::Form(form)) = &mut self.modal {
                    form.focus(index);
                }
            }
            Target::FieldText { index, top, left } => {
                if let Some(Modal::Form(form)) = &mut self.modal {
                    form.focus(index);
                    form.cursor = cursor_at(
                        &form.fields[index],
                        usize::from(top) + usize::from(event.row - area.y),
                        usize::from(left) + usize::from(event.column - area.x),
                    );
                }
            }
        }
        Effect::None
    }
}

fn cursor_at(value: &str, row: usize, column: usize) -> usize {
    let mut start = 0;
    for (index, line) in value.split('\n').enumerate() {
        if index == row {
            let mut cells = 0;
            // Ratatui's styled_graphemes filters controls out. Measure the
            // sanitized display text, but keep offsets into the original source.
            for (offset, grapheme) in line.grapheme_indices(true) {
                let shown: String = grapheme
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                let width = Line::from(shown).width();
                if cells + width > column {
                    return start + offset;
                }
                cells += width;
            }
            return start + line.len();
        }
        start += line.len() + 1;
    }
    value.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    fn app() -> App {
        App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true)
    }
    fn render(app: &mut App, w: u16, h: u16) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    }
    fn hit(app: &App, predicate: impl Fn(&Target) -> bool) -> Rect {
        app.mouse
            .hits
            .iter()
            .find(|(_, target)| predicate(target))
            .unwrap()
            .0
    }
    fn event(kind: MouseEventKind, area: Rect) -> MouseEvent {
        MouseEvent {
            kind,
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        }
    }
    fn key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn click(app: &mut App, area: Rect) -> Effect {
        app.handle_mouse(event(MouseEventKind::Down(MouseButton::Left), area))
    }
    fn click_key(app: &mut App, code: KeyCode) -> Effect {
        let area = hit(app, |t| matches!(t, Target::Key(k) if k.code == code));
        click(app, area)
    }
    #[test]
    fn cursor_positions_follow_display_columns_and_preserve_unicode_boundaries() {
        let text = "a界e\u{301}\n\tlast";
        assert_eq!(cursor_at(text, 0, 1), 1);
        assert_eq!(cursor_at(text, 0, 2), 1);
        assert_eq!(cursor_at(text, 0, 3), 4);
        assert_eq!(cursor_at(text, 0, 4), 7);
        assert_eq!(cursor_at(text, 1, 1), 9);
        assert_eq!(cursor_at(text, 99, 99), text.len());
    }
    #[test]
    fn cursor_positions_include_control_characters_in_source_offsets() {
        for (text, expected) in [
            ("\t界a", vec![0, 1, 1, 4, 5]),
            ("\u{85}界a", vec![0, 2, 2, 5, 6]),
            ("\t\r界a", vec![0, 1, 2, 2, 5, 6]),
            ("\t界e\u{301}", vec![0, 1, 1, 4, 7]),
        ] {
            for (column, expected) in expected.into_iter().enumerate() {
                let offset = cursor_at(text, 0, column);
                assert_eq!(
                    offset, expected,
                    "Wrong offset for {text:?} at column {column}"
                );
                assert!(text.is_char_boundary(offset));
            }
            let multiline = format!("first\n{text}");
            assert_eq!(cursor_at(&multiline, 1, 2), 6 + cursor_at(text, 0, 2));
        }
    }
    #[test]
    fn clicking_a_tabbed_unicode_field_preserves_unsaved_text_and_remains_editable() {
        let mut app = app();
        app.handle(key_event(KeyCode::Char('n')));
        let Some(Modal::Form(form)) = &mut app.modal else {
            panic!()
        };
        form.fields[3] = "\t界a".into();
        render(&mut app, 120, 40);
        let area = hit(&app, |t| matches!(t, Target::FieldText { index: 3, .. }));
        let mut mouse = event(MouseEventKind::Down(MouseButton::Left), area);
        mouse.column += 2;
        app.handle_mouse(mouse);
        let Some(Modal::Form(form)) = &app.modal else {
            panic!()
        };
        assert_eq!(form.cursor, 1);
        assert_eq!(form.fields[3], "\t界a");
        render(&mut app, 120, 40); // This render previously panicked inside the multibyte character.
        app.handle(key_event(KeyCode::Char('X')));
        render(&mut app, 120, 40);
        let Some(Modal::Form(form)) = &app.modal else {
            panic!()
        };
        assert_eq!(form.fields[3], "\tX界a");
        assert_eq!(
            form.value().unwrap().as_dictionary().unwrap()["WorkingDirectory"].as_string(),
            Some("\tX界a")
        );
    }
    #[test]
    fn tabs_job_rows_and_scroll_wheels_use_the_rendered_regions() {
        let mut app = app();
        render(&mut app, 120, 40);
        let area = hit(&app, |t| matches!(t, Target::Scope(Scope::Daemons)));
        click(&mut app, area);
        assert_eq!(app.scope, Scope::Daemons);
        render(&mut app, 120, 40);
        let area = hit(&app, |t| matches!(t, Target::Job(1)));
        click(&mut app, area);
        assert_eq!(app.table.selected(), Some(1));
        render(&mut app, 65, 18);
        let jobs = app
            .mouse
            .wheels
            .iter()
            .find(|(_, w)| matches!(w, Wheel::Jobs))
            .unwrap()
            .0;
        let preview = app.detail_text.clone();
        let offset = app.table.offset();
        app.handle_mouse(event(MouseEventKind::ScrollDown, jobs));
        render(&mut app, 65, 18);
        assert_eq!(app.table.selected(), Some(1));
        assert_eq!(app.table.offset(), offset + 1);
        assert_eq!(app.detail_text, preview);
        assert!(
            app.mouse
                .hits
                .iter()
                .any(|(_, t)| matches!(t, Target::Job(2)))
        );
        app.refresh();
        render(&mut app, 65, 18);
        assert_eq!(app.table.offset(), offset + 1); // Refresh must not snap back to the selection.
        app.handle_mouse(event(MouseEventKind::ScrollUp, jobs));
        render(&mut app, 65, 18);
        assert_eq!(app.table.offset(), offset);
        assert_eq!(app.table.selected(), Some(1));
        for _ in 0..20 {
            app.handle_mouse(event(MouseEventKind::ScrollDown, jobs));
            render(&mut app, 65, 18);
        }
        assert_eq!(app.table.offset(), app.visible.len() - app.page_size);
        assert_eq!(app.table.selected(), Some(1));
        assert_eq!(app.detail_text, preview);
        let last = hit(&app, |t| matches!(t, Target::Job(5)));
        click(&mut app, last);
        assert_eq!(app.table.selected(), Some(5));
        // Keyboard navigation returns to following the selected job.
        app.handle(key_event(KeyCode::Char('k')));
        render(&mut app, 65, 18);
        assert_eq!(app.table.selected(), Some(4));
        assert!(app.job_scroll.is_none());
        assert!(
            app.mouse
                .hits
                .iter()
                .any(|(_, t)| matches!(t, Target::Job(4)))
        );
        app.detail_text = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        render(&mut app, 120, 40);
        let plist = app
            .mouse
            .wheels
            .iter()
            .find(|(_, w)| matches!(w, Wheel::Plist))
            .unwrap()
            .0;
        app.handle_mouse(event(MouseEventKind::ScrollDown, plist));
        render(&mut app, 120, 40);
        assert_eq!(app.detail_scroll, 1);
        app.detail_scroll = u16::MAX;
        render(&mut app, 120, 40);
        let bottom = app.detail_scroll;
        app.handle_mouse(event(MouseEventKind::ScrollDown, plist));
        render(&mut app, 120, 40);
        assert_eq!(app.detail_scroll, bottom);
        // After resizing and scrolling, row hit indices include TableState's offset.
        app.table.select(Some(5));
        app.load_detail();
        render(&mut app, 65, 18);
        let area = hit(&app, |t| matches!(t, Target::Job(5)));
        app.handle_mouse(event(MouseEventKind::Down(MouseButton::Right), area));
        assert!(matches!(app.modal, Some(Modal::Actions(0))));
    }
    #[test]
    fn menus_activate_items_and_modals_block_background_controls() {
        let mut app = app();
        render(&mut app, 120, 40);
        let quit = Rect::new(1, 39, 1, 1);
        app.handle(key_event(KeyCode::Char(' ')));
        render(&mut app, 120, 40);
        assert!(matches!(click(&mut app, quit), Effect::None));
        assert!(matches!(app.modal, Some(Modal::Actions(0))));
        let area = hit(&app, |t| matches!(t, Target::Menu(1)));
        assert!(matches!(click(&mut app, area), Effect::Editor));
        for index in [2, 3] {
            app.modal = Some(Modal::Actions(index));
            render(&mut app, 120, 40);
            let area = hit(&app, |t| matches!(t, Target::Menu(i) if *i == index));
            assert!(matches!(click(&mut app, area), Effect::Log(_)));
        }
        app.modal = Some(Modal::Actions(0));
        render(&mut app, 120, 40);
        let menu = app.mouse.wheels[0].0;
        app.handle_mouse(event(MouseEventKind::ScrollDown, menu));
        assert!(matches!(app.modal, Some(Modal::Actions(1))));
        // Unload still requires confirmation when activated by a click.
        render(&mut app, 120, 40);
        let area = hit(&app, |t| matches!(t, Target::Menu(5)));
        click(&mut app, area);
        assert!(matches!(
            app.modal,
            Some(Modal::Confirm(crate::launchd::Action::Unload, _))
        ));
        render(&mut app, 120, 40);
        click_key(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        app.modal = Some(Modal::Confirm(
            crate::launchd::Action::Delete,
            Box::new(app.jobs[0].clone()),
        ));
        render(&mut app, 120, 40);
        click_key(&mut app, KeyCode::Char('y'));
        assert!(matches!(app.modal, Some(Modal::Message(_, _)))); // Demo backend refuses mutations.
        let mut job = app.jobs[0].clone();
        job.path = Some(format!("/tmp/y confirm/{}", "directory/".repeat(30)).into());
        app.modal = Some(Modal::Confirm(
            crate::launchd::Action::Delete,
            Box::new(job),
        ));
        render(&mut app, 65, 18);
        assert!(
            !app.mouse.hits.iter().any(
                |(_, target)| matches!(target, Target::Key(k) if k.code == KeyCode::Char('y'))
            )
        );
    }
    #[test]
    fn footer_clicks_are_inert_and_form_values_still_place_the_cursor() {
        let mut app = app();
        render(&mut app, 120, 40);
        for x in 0..120 {
            assert!(matches!(
                click(&mut app, Rect::new(x, 39, 1, 1)),
                Effect::None
            ));
            assert!(app.modal.is_none());
            assert!(app.notice.is_empty());
        }
        click_key(&mut app, KeyCode::Char('/'));
        assert!(app.searching);
        click(&mut app, Rect::new(1, 39, 1, 1));
        assert!(app.searching);
        app.handle(key_event(KeyCode::Esc));
        app.handle(key_event(KeyCode::Char('n')));
        if let Some(Modal::Form(form)) = &mut app.modal {
            form.fields[3] = "a界e\u{301}".into();
        }
        render(&mut app, 120, 40);
        let area = hit(&app, |t| matches!(t, Target::FieldText { index: 3, .. }));
        let mut mouse = event(MouseEventKind::Down(MouseButton::Left), area);
        mouse.column += 2;
        app.handle_mouse(mouse);
        let Some(Modal::Form(form)) = &app.modal else {
            panic!()
        };
        assert_eq!(form.focus, 3);
        assert_eq!(form.cursor, 1);
        assert_eq!(form.fields[3], "a界e\u{301}");
        render(&mut app, 120, 40);
        let form_area = app.mouse.wheels[0].0;
        app.handle_mouse(event(MouseEventKind::ScrollDown, form_area));
        let Some(Modal::Form(form)) = &app.modal else {
            panic!()
        };
        assert_eq!(form.focus, 4);
        render(&mut app, 120, 40);
        for y in [36, 37] {
            for x in 11..109 {
                assert!(matches!(
                    click(&mut app, Rect::new(x, y, 1, 1)),
                    Effect::None
                ));
                let Some(Modal::Form(form)) = &app.modal else {
                    panic!("Footer closed the form")
                };
                assert_eq!(form.focus, 4);
                assert!(form.error.is_none());
            }
        }
        app.handle(key_event(KeyCode::BackTab));
        let Some(Modal::Form(form)) = &app.modal else {
            panic!()
        };
        assert_eq!(form.focus, 3);
        render(&mut app, 120, 40);
        app.handle(key_event(KeyCode::Esc));
        assert!(app.modal.is_none());
        render(&mut app, 120, 40);
        app.handle(key_event(KeyCode::Char('?')));
        render(&mut app, 120, 40);
        let help = app.mouse.wheels[0].0;
        app.handle_mouse(event(MouseEventKind::ScrollDown, help));
        assert_eq!(app.detail_scroll, 1);
        app.detail_scroll = 20;
        render(&mut app, 120, 40);
        click_key(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }
}
