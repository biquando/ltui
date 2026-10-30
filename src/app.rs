use crate::error::ErrorReport;
use crate::{
    launchd::{Action, Backend, Launchctl},
    model::{self, Form, Job, Scope, State},
};
use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use plist::Value;
use ratatui::widgets::TableState;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

#[derive(Clone, Copy)]
pub enum MenuAction {
    Edit,
    Editor,
    Log(&'static str),
    Launchctl(Action),
}
impl MenuAction {
    pub const FILE_COUNT: usize = 4;
    pub const ALL: [Self; 15] = [
        Self::Edit,
        Self::Editor,
        Self::Log("StandardOutPath"),
        Self::Log("StandardErrorPath"),
        Self::Launchctl(Action::Load),
        Self::Launchctl(Action::Unload),
        Self::Launchctl(Action::Enable),
        Self::Launchctl(Action::Disable),
        Self::Launchctl(Action::Start),
        Self::Launchctl(Action::Restart),
        Self::Launchctl(Action::Stop),
        Self::Launchctl(Action::Kill),
        Self::Launchctl(Action::Hup),
        Self::Launchctl(Action::Reload),
        Self::Launchctl(Action::Delete),
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Editor => "Edit with $EDITOR",
            Self::Log("StandardOutPath") => "Open stdout log",
            Self::Log(_) => "Open stderr log",
            Self::Launchctl(a) => a.title(),
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Edit => "Edit common plist options in the form.",
            Self::Editor => "Edit the whole plist in $EDITOR, then validate and save.",
            Self::Log(_) => {
                "Open the configured log file in $PAGER (default: less). Quit the pager to return."
            }
            Self::Launchctl(a) => a.description(),
        }
    }
}
pub enum Modal {
    Help,
    Actions(usize),
    Confirm(Action, Box<Job>),
    Form(Box<Form>),
    Message(String, ErrorReport),
}
pub enum Effect {
    None,
    Quit,
    Editor,
    Log(&'static str),
}

pub struct App {
    pub mouse: crate::mouse::MouseMap,
    pub backend: Backend,
    pub scope: Scope,
    pub jobs: Vec<Job>,
    pub visible: Vec<usize>,
    pub table: TableState,
    pub filter: String,
    pub searching: bool,
    pub detail_text: String,
    pub detail_scroll: u16,
    pub preview_height: u16,
    pub modal: Option<Modal>,
    pub notice: String,
    pub warnings: Vec<String>,
    pub refreshed: Instant,
    pub page_size: usize,
    pub job_scroll: Option<usize>,
}

impl App {
    pub fn new(home: PathBuf, agent_domain: String, scope: Scope, demo: bool) -> Self {
        let mut app = Self {
            mouse: crate::mouse::MouseMap::default(),
            backend: Backend {
                runner: Launchctl,
                home,
                agent_domain,
                demo,
            },
            scope,
            jobs: vec![],
            visible: vec![],
            table: TableState::default(),
            filter: String::new(),
            searching: false,
            detail_text: String::new(),
            detail_scroll: 0,
            preview_height: 0,
            modal: None,
            notice: String::new(),
            warnings: vec![],
            refreshed: Instant::now(),
            page_size: 10,
            job_scroll: None,
        };
        app.refresh();
        app
    }
    pub fn selected(&self) -> Option<&Job> {
        self.table
            .selected()
            .and_then(|i| self.visible.get(i))
            .and_then(|i| self.jobs.get(*i))
    }
    pub fn refresh(&mut self) {
        let selected = self.selected().map(|j| (j.label.clone(), j.path.clone()));
        let scroll = self.detail_scroll;
        let job_scroll = self.job_scroll;
        let inventory = self.backend.inventory(self.scope);
        self.jobs = inventory.jobs;
        self.warnings = inventory.warnings;
        self.refilter();
        let mut same_selection = false;
        if let Some((label, path)) = selected
            && let Some(i) = self
                .visible
                .iter()
                .position(|i| self.jobs[*i].label == label && self.jobs[*i].path == path)
                .or_else(|| {
                    self.visible
                        .iter()
                        .position(|i| self.jobs[*i].label == label)
                })
        {
            self.table.select(Some(i));
            same_selection = true;
        }
        self.refreshed = Instant::now();
        self.load_detail();
        if same_selection {
            self.detail_scroll = scroll;
            self.job_scroll = job_scroll;
        }
    }
    pub fn refilter(&mut self) {
        let filter = self.filter.to_lowercase();
        self.visible = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| {
                format!(
                    "{} {} {} {}",
                    j.label,
                    j.command(),
                    j.enabled_text(),
                    j.state.text()
                )
                .to_lowercase()
                .contains(&filter)
            })
            .map(|(i, _)| i)
            .collect();
        self.table.select(if self.visible.is_empty() {
            None
        } else {
            Some(
                self.table
                    .selected()
                    .unwrap_or(0)
                    .min(self.visible.len() - 1),
            )
        });
    }
    pub fn load_detail(&mut self) {
        self.job_scroll = None;
        self.detail_scroll = 0;
        let Some(index) = self
            .table
            .selected()
            .and_then(|i| self.visible.get(i))
            .copied()
        else {
            self.detail_text.clear();
            return;
        };
        // Resolve runtime-only plist paths on demand, avoiding a command per row.
        let job = &mut self.jobs[index];
        if job.state.loaded() && job.path.is_none() {
            let _ = self.backend.details(job);
        }
        self.detail_text = job.definition.as_ref().map(model::xml)
            .map(|r| r.map(|s| String::from_utf8_lossy(&s).into_owned()))
            .unwrap_or_else(|| {
                if let Some(bytes) = &job.file_bytes { Ok(String::from_utf8_lossy(bytes).into_owned()) }
                else { Ok("No readable plist. Press Enter to repair the file in $EDITOR, or select a job with a plist.".into()) }
            }).unwrap_or_else(|e| format!("{e:#}"));
    }

    pub(crate) fn select(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let i = self
            .table
            .selected()
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(self.visible.len() - 1);
        self.table.select(Some(i));
        self.load_detail();
    }
    fn scope_step(&mut self, delta: isize) {
        let i = Scope::ALL
            .iter()
            .position(|s| *s == self.scope)
            .unwrap_or(0);
        self.scope =
            Scope::ALL[(i as isize + delta).rem_euclid(Scope::ALL.len() as isize) as usize];
        self.table.select(None);
        self.job_scroll = None;
        self.refresh();
    }
    pub fn error(&mut self, error: impl std::fmt::Display) {
        self.show_error(error.to_string().into());
    }
    pub fn report_error(&mut self, error: anyhow::Error) {
        self.show_error(ErrorReport::from_error(error));
    }
    fn show_error(&mut self, error: ErrorReport) {
        self.detail_scroll = 0;
        self.modal = Some(Modal::Message("Could not complete action".into(), error));
    }
    fn request(&mut self, action: Action) {
        let Some(job) = self.selected().cloned() else {
            return;
        };
        if action.confirm() {
            self.modal = Some(Modal::Confirm(action, Box::new(job)));
        } else {
            self.perform(action, &job);
        }
    }
    fn perform(&mut self, action: Action, job: &Job) {
        match self.backend.action(action, job) {
            Ok(out) => {
                self.notice = format!(
                    "{} · {}{}",
                    action.title(),
                    job.label,
                    if out.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", out.trim())
                    }
                );
                self.refresh();
            }
            Err(e) => {
                self.refresh();
                self.report_error(e);
            }
        }
    }
    fn open_form(&mut self, create: bool) -> Result<()> {
        if create {
            ensure!(
                !self.scope.is_apple(),
                "Choose User agents, Shared agents, or Daemons to create a job"
            );
            self.modal = Some(Modal::Form(Box::new(Form::new())));
        } else if let Some(job) = self.selected().cloned() {
            ensure!(!job.read_only(), "Apple system plists are read-only");
            ensure!(
                job.error.is_none(),
                "This plist has an error. Press Enter to edit the full file in $EDITOR or resolve duplicate Labels first"
            );
            let path = job
                .path
                .as_ref()
                .context("This service has no plist. Only launchctl actions are available")?;
            let bytes = if self.backend.demo {
                model::xml(job.definition.as_ref().unwrap())?
            } else {
                fs::read(path)?
            };
            let value = if self.backend.demo {
                job.definition.clone().unwrap()
            } else {
                Value::from_reader(std::io::Cursor::new(&bytes))?
            };
            self.modal = Some(Modal::Form(Box::new(Form::from_value(
                value,
                Some(path.clone()),
                Some(bytes),
                Some(job),
            ))));
        }
        Ok(())
    }
    pub fn create_dir(&self) -> PathBuf {
        match self.scope {
            Scope::Session => Scope::User,
            Scope::System => Scope::Daemons,
            scope => scope,
        }
        .directory(&self.backend.home)
        .unwrap()
    }
    fn commit(
        &mut self,
        value: &Value,
        path: &Path,
        bytes: Option<&[u8]>,
        job: Option<&Job>,
    ) -> Result<()> {
        let label = model::validate(value)?;
        let domain = job
            .map(|j| j.domain.as_str())
            .unwrap_or_else(|| self.scope.domain(&self.backend.agent_domain));
        let loaded = self
            .backend
            .save_definition(value, path, bytes, job, domain)?;
        let mut saved = job.cloned().unwrap_or_else(|| Job {
            label: label.into(),
            domain: domain.into(),
            path: Some(path.into()),
            definition: Some(value.clone()),
            file_bytes: None,
            error: None,
            state: State::Unloaded,
            disabled: Some(false),
            override_disabled: None,
        });
        saved.label = label.into();
        saved.definition = Some(value.clone());
        self.notice = format!(
            "Saved {}{}",
            path.display(),
            if loaded {
                " · reload to apply changes"
            } else {
                " · press l to load"
            }
        );
        // Keep an edited Label visible even if the old filter no longer matches.
        self.filter.clear();
        self.refresh();
        if let Some(i) = self
            .visible
            .iter()
            .position(|i| self.jobs[*i].path.as_deref() == Some(path))
        {
            self.table.select(Some(i));
            self.load_detail();
        }
        if loaded {
            self.modal = Some(Modal::Confirm(Action::Reload, Box::new(saved)));
        }
        Ok(())
    }
    fn save_form(&mut self, form: &Form) -> Result<()> {
        let value = form.value()?;
        let label = model::validate(&value)?;
        let path = form
            .path
            .clone()
            .unwrap_or_else(|| self.create_dir().join(format!("{label}.plist")));
        self.commit(&value, &path, form.bytes.as_deref(), form.job.as_ref())
    }
    pub fn handle(&mut self, key: KeyEvent) -> Effect {
        // Reuse j/k navigation without turning control keys into typed text.
        let mut key = key;
        if !self.searching
            && !matches!(&self.modal, Some(Modal::Form(_)))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            match key.code {
                KeyCode::Char('n') => {
                    key.code = KeyCode::Char('j');
                    key.modifiers.remove(KeyModifiers::CONTROL);
                }
                KeyCode::Char('p') => {
                    key.code = KeyCode::Char('k');
                    key.modifiers.remove(KeyModifiers::CONTROL);
                }
                _ => {}
            }
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
            && !matches!(&self.modal, Some(Modal::Confirm(_, _)))
        {
            return Effect::Quit;
        }
        if let Some(modal) = self.modal.take() {
            match modal {
                Modal::Help | Modal::Message(_, _) => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?') => {
                            return Effect::None;
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.detail_scroll = self.detail_scroll.saturating_add(4)
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.detail_scroll = self.detail_scroll.saturating_sub(4)
                        }
                        KeyCode::PageDown => {
                            self.detail_scroll = self.detail_scroll.saturating_add(8)
                        }
                        KeyCode::PageUp => {
                            self.detail_scroll = self.detail_scroll.saturating_sub(8)
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            self.detail_scroll = self.detail_scroll.saturating_add(1)
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            self.detail_scroll = self.detail_scroll.saturating_sub(1)
                        }
                        _ => {}
                    }
                    self.modal = Some(modal);
                }
                Modal::Confirm(action, job) => match key.code {
                    KeyCode::Char('y') if key.modifiers.is_empty() => self.perform(action, &job),
                    KeyCode::Esc if key.modifiers.is_empty() => self.notice = "Cancelled".into(),
                    _ => self.modal = Some(Modal::Confirm(action, job)),
                },
                Modal::Actions(mut i) => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => return Effect::None,
                        KeyCode::Down | KeyCode::Char('j') => {
                            i = (i + 1).min(MenuAction::ALL.len() - 1)
                        }
                        KeyCode::Up | KeyCode::Char('k') => i = i.saturating_sub(1),
                        KeyCode::PageDown => {
                            i = (i + self.page_size).min(MenuAction::ALL.len() - 1)
                        }
                        KeyCode::PageUp => i = i.saturating_sub(self.page_size),
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            i = (i + (self.page_size / 2).max(1)).min(MenuAction::ALL.len() - 1)
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            i = i.saturating_sub((self.page_size / 2).max(1))
                        }
                        KeyCode::Enter => {
                            match MenuAction::ALL[i] {
                                MenuAction::Edit => {
                                    if let Err(e) = self.open_form(false) {
                                        self.report_error(e);
                                    }
                                }
                                MenuAction::Editor => return Effect::Editor,
                                MenuAction::Log(key) => return Effect::Log(key),
                                MenuAction::Launchctl(a) => self.request(a),
                            }
                            return Effect::None;
                        }
                        _ => {}
                    }
                    self.modal = Some(Modal::Actions(i));
                }
                Modal::Form(mut form) => {
                    match key.code {
                        KeyCode::Esc => {
                            self.notice = "Edit cancelled".into();
                            return Effect::None;
                        }
                        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            match self.save_form(&form) {
                                Ok(()) => return Effect::None,
                                Err(e) => form.error = Some(format!("{e:#}")),
                            }
                        }
                        KeyCode::Tab => form.focus((form.focus + 1) % form.fields.len()),
                        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            form.focus((form.focus + 1) % form.fields.len())
                        }
                        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            form.focus((form.focus + form.fields.len() - 1) % form.fields.len())
                        }
                        KeyCode::BackTab => {
                            form.focus((form.focus + form.fields.len() - 1) % form.fields.len())
                        }
                        KeyCode::Down => {
                            if !form.move_line(true) {
                                form.focus((form.focus + 1) % form.fields.len());
                            }
                        }
                        KeyCode::Up => {
                            if !form.move_line(false) {
                                form.focus(
                                    (form.focus + form.fields.len() - 1) % form.fields.len(),
                                );
                            }
                        }
                        KeyCode::Enter => {
                            if form.accepts_newlines() {
                                form.insert("\n");
                            } else {
                                form.focus((form.focus + 1) % form.fields.len());
                            }
                        }
                        KeyCode::Backspace => form.backspace(),
                        KeyCode::Delete => form.delete(),
                        KeyCode::Left => form.left(),
                        KeyCode::Right => form.right(),
                        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            form.cursor = 0
                        }
                        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            form.cursor = form.fields[form.focus].len()
                        }
                        KeyCode::Home => form.home(),
                        KeyCode::End => form.end(),
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            form.fields[form.focus].clear();
                            form.cursor = 0;
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            form.insert(&c.to_string())
                        }
                        _ => {}
                    }
                    self.modal = Some(Modal::Form(form));
                }
            }
            return Effect::None;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.filter.clear();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.filter.push(c)
                }
                _ => {}
            }
            self.table.select(Some(0));
            self.refilter();
            self.load_detail();
            return Effect::None;
        }
        match key.code {
            KeyCode::Char('q') => return Effect::Quit,
            KeyCode::Char('?') => {
                self.detail_scroll = 0;
                self.modal = Some(Modal::Help);
            }
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Esc => {
                self.filter.clear();
                self.refilter();
                self.load_detail();
            }
            KeyCode::Tab => self.scope_step(1),
            KeyCode::BackTab => self.scope_step(-1),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select(-((self.page_size / 2).max(1) as isize))
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select((self.page_size / 2).max(1) as isize)
            }
            KeyCode::Down | KeyCode::Char('j') => self.select(1),
            KeyCode::Up | KeyCode::Char('k') => self.select(-1),
            KeyCode::PageDown => self.select(self.page_size as isize),
            KeyCode::PageUp => self.select(-(self.page_size as isize)),
            KeyCode::Char('J') => {
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add((self.preview_height / 2).max(1))
            }
            KeyCode::Char('K') => {
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_sub((self.preview_height / 2).max(1))
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.table.select(Some(0));
                self.detail_scroll = 0;
                self.load_detail();
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.table.select(self.visible.len().checked_sub(1));
                self.load_detail();
            }
            KeyCode::Char(' ') => {
                if self.selected().is_some() {
                    self.modal = Some(Modal::Actions(0));
                }
            }
            KeyCode::Enter => {
                if self.selected().is_some() {
                    return Effect::Editor;
                }
            }
            KeyCode::Char('R') => {
                self.refresh();
                self.notice = "Refreshed".into();
            }
            KeyCode::Char('n') => {
                if let Err(e) = self.open_form(true) {
                    self.report_error(e);
                }
            }
            KeyCode::Char('e') => {
                if let Err(e) = self.open_form(false) {
                    self.report_error(e);
                }
            }
            KeyCode::Char('l') => self.request(Action::Load),
            KeyCode::Char('u') => self.request(Action::Unload),
            KeyCode::Char('s') => self.request(Action::Start),
            KeyCode::Char('r') => self.request(Action::Restart),
            KeyCode::Char('t') => {
                if let Some(job) = self.selected() {
                    match job.disabled { Some(true) => self.request(Action::Enable), Some(false) => self.request(Action::Disable), None => self.error("Enable state is unknown. Use the actions menu to explicitly Enable or Disable") }
                }
            }
            KeyCode::Char('x') => self.request(Action::Delete),
            _ => {}
        }
        Effect::None
    }

    pub fn prepare_log(&self, key: &str) -> Result<LogSession> {
        ensure!(
            !self.backend.demo,
            "Demo mode: sample log files are not available"
        );
        let job = self.selected().context("Select a job first")?;
        let d = job
            .definition
            .as_ref()
            .and_then(Value::as_dictionary)
            .context("This job has no readable plist")?;
        let path = d
            .get(key)
            .and_then(Value::as_string)
            .with_context(|| format!("{key} is not configured in this plist"))?;
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            Path::new(
                d.get("WorkingDirectory")
                    .and_then(Value::as_string)
                    .unwrap_or("/"),
            )
            .join(path)
        }
        .canonicalize()
        .with_context(|| format!("Opening {key}"))?;
        ensure!(
            fs::metadata(&path)?.is_file(),
            "The log path is not a regular file"
        );
        fs::File::open(&path).with_context(|| format!("Reading {}", path.display()))?;
        let pager = std::env::var("PAGER")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "less".into());
        let argv = shell_words::split(&pager).context("Invalid quoting in $PAGER")?;
        ensure!(!argv.is_empty(), "$PAGER is empty");
        Ok(LogSession { path, argv })
    }

    /// Prepare a private XML copy. The real plist is committed only after a valid edit.
    pub fn prepare_editor(&self) -> Result<EditorSession> {
        ensure!(
            !self.backend.demo,
            "Demo mode: external editing is disabled"
        );
        let job = self.selected().cloned().context("Select a job first")?;
        ensure!(!job.read_only(), "Apple system plists are read-only");
        let path = job.path.clone().context("This service has no plist file")?;
        ensure!(
            fs::metadata(&path)?.is_file(),
            "Refusing to edit a non-regular file"
        );
        let bytes = fs::read(&path)?;
        let contents = Value::from_reader(std::io::Cursor::new(&bytes))
            .ok()
            .map(|v| model::xml(&v))
            .transpose()?
            .unwrap_or_else(|| bytes.clone());
        let mut temp = tempfile::Builder::new()
            .prefix("ltui-")
            .suffix(".plist")
            .tempfile()?;
        temp.write_all(&contents)?;
        temp.flush()?;
        let editor = std::env::var("EDITOR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| std::env::var("VISUAL").ok())
            .unwrap_or_else(|| "vi".into());
        let argv = shell_words::split(&editor).context("Invalid quoting in $EDITOR")?;
        ensure!(!argv.is_empty(), "$EDITOR is empty");
        Ok(EditorSession {
            job,
            path,
            bytes,
            temp,
            argv,
            initial: contents,
        })
    }
    pub fn finish_editor(&mut self, session: EditorSession) {
        let result = (|| -> Result<()> {
            let edited = fs::read(session.temp.path())?;
            if edited == session.initial {
                self.notice = "No changes made".into();
                return Ok(());
            }
            let value = Value::from_reader(std::io::Cursor::new(edited))
                .context("The edited file is not a valid plist")?;
            self.commit(
                &value,
                &session.path,
                Some(&session.bytes),
                Some(&session.job),
            )
        })();
        if let Err(e) = result {
            let recovery = session.temp.keep().map(|(_, p)| p);
            self.report_error(e.context(format!(
                    "Your original plist was preserved. Edited copy: {}",
                    recovery
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|e| format!("could not retain temporary file: {e}"))
                )));
        }
    }
}

pub struct LogSession {
    path: PathBuf,
    argv: Vec<String>,
}
impl LogSession {
    pub fn run(&self) -> Result<()> {
        let status = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .arg(&self.path)
            .status()
            .context("Starting $PAGER")?;
        ensure!(status.success(), "$PAGER exited with {status}");
        Ok(())
    }
}

pub struct EditorSession {
    pub job: Job,
    path: PathBuf,
    bytes: Vec<u8>,
    pub temp: tempfile::NamedTempFile,
    argv: Vec<String>,
    initial: Vec<u8>,
}
impl EditorSession {
    pub fn run(&self) -> Result<()> {
        let status = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .arg(self.temp.path())
            .status()
            .context(
                "Starting $EDITOR (use a waiting option such as code --wait for GUI editors)",
            )?;
        ensure!(status.success(), "$EDITOR exited with {status}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    #[test]
    fn search_and_confirmation_do_not_execute_actions() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        app.handle(key('/'));
        for c in "disabled".chars() {
            app.handle(key(c));
        }
        assert_eq!(app.visible.len(), 1);
        app.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle(key('x'));
        assert!(matches!(app.modal, Some(Modal::Confirm(Action::Delete, _))));
        for key in [
            key('n'),
            key('q'),
            key('Y'),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        ] {
            assert!(matches!(app.handle(key), Effect::None));
            assert!(matches!(app.modal, Some(Modal::Confirm(Action::Delete, _))));
        }
        app.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.modal.is_none());
        assert_eq!(app.jobs.len(), 6);
    }
    #[test]
    fn form_errors_keep_unsaved_changes() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        app.handle(key('n'));
        if let Some(Modal::Form(f)) = &mut app.modal {
            f.fields[0] = "../invalid".into();
        }
        app.handle(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let Some(Modal::Form(f)) = &app.modal else {
            panic!("form closed")
        };
        assert!(f.error.is_some());
        assert_eq!(f.fields[0], "../invalid");
    }
    #[test]
    fn multiline_form_keys_edit_lines_and_tab_still_changes_fields() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        let mut form = Form::new();
        form.fields[2] = "/bin/sh -c 'first\nsecond'".into();
        form.focus(2);
        form.cursor = form.fields[2].find("second").unwrap();
        app.modal = Some(Modal::Form(Box::new(form)));
        for code in [KeyCode::Up, KeyCode::End, KeyCode::Enter] {
            app.handle(KeyEvent::new(code, KeyModifiers::NONE));
        }
        let Some(Modal::Form(form)) = &app.modal else {
            panic!("form closed")
        };
        assert_eq!(form.focus, 2);
        assert_eq!(form.fields[2], "/bin/sh -c 'first\n\nsecond'");
        app.handle(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let Some(Modal::Form(form)) = &app.modal else {
            panic!("form closed")
        };
        assert_eq!(form.focus, 3);
        app.handle(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        let Some(Modal::Form(form)) = &app.modal else {
            panic!("form closed")
        };
        assert_eq!(form.focus, 2);
        let text = form.fields[2].clone();
        app.handle(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        let Some(Modal::Form(form)) = &app.modal else {
            panic!("form closed")
        };
        assert_eq!(form.focus, 3);
        assert_eq!(form.fields[2], text);
    }
    #[test]
    fn revised_shortcuts_page_jobs_scroll_plist_and_open_actions() {
        let mut app = App::new("/tmp/demo".into(), "gui/501".into(), Scope::User, true);
        app.page_size = 2;
        for code in [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Char('E'),
            KeyCode::Char('a'),
            KeyCode::Char('p'),
            KeyCode::Char('o'),
        ] {
            assert!(matches!(
                app.handle(KeyEvent::new(code, KeyModifiers::NONE)),
                Effect::None
            ));
            assert!(app.modal.is_none());
            assert_eq!(app.table.selected(), Some(0));
        }
        app.handle(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(app.table.selected(), Some(1));
        assert!(app.modal.is_none());
        app.handle(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(app.table.selected(), Some(0));
        app.handle(key('j'));
        app.handle(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.table.selected(), Some(2));
        app.handle(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.table.selected(), Some(1));
        assert!(app.modal.is_none()); // Ctrl-U must not invoke Unload.
        app.handle(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.table.selected(), Some(3));
        app.handle(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.table.selected(), Some(1));
        app.preview_height = 14;
        app.handle(key('J'));
        assert_eq!(app.detail_scroll, 7);
        app.handle(key('K'));
        assert_eq!(app.detail_scroll, 0);
        assert!(matches!(
            app.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Effect::Editor
        ));
        assert!(app.modal.is_none());
        app.handle(key(' '));
        assert!(matches!(app.modal, Some(Modal::Actions(0))));
        app.handle(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert!(matches!(app.modal, Some(Modal::Actions(1))));
        app.handle(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(matches!(app.modal, Some(Modal::Actions(0))));
        app.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.modal, Some(Modal::Form(_))));
        app.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.modal = Some(Modal::Actions(1));
        assert!(matches!(
            app.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Effect::Editor
        ));
        for (index, key) in [(2, "StandardOutPath"), (3, "StandardErrorPath")] {
            app.modal = Some(Modal::Actions(index));
            assert!(
                matches!(app.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Effect::Log(k) if k == key)
            );
        }
        app.modal = Some(Modal::Actions(5));
        app.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.modal, Some(Modal::Confirm(Action::Unload, _))));
    }
}
