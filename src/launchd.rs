use crate::error::CommandFailure;
use crate::model::{self, Job, Scope, State};
use anyhow::{Context, Result, bail, ensure};
use plist::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub trait Runner {
    fn run(&self, args: &[String]) -> Result<String>;
}

pub struct Launchctl;
impl Runner for Launchctl {
    fn run(&self, args: &[String]) -> Result<String> {
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        let mut child = Command::new("/bin/launchctl")
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout.try_clone()?)
            .stderr(stderr.try_clone()?)
            .spawn()
            .map_err(|e| {
                CommandFailure::new(
                    args,
                    String::new(),
                    String::new(),
                    format!("Could not start /bin/launchctl (macOS is required): {e}"),
                )
            })?;
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if start.elapsed() > Duration::from_secs(8) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        stdout.rewind()?;
        stderr.rewind()?;
        let mut out = String::new();
        let mut err = String::new();
        stdout.take(4 * 1024 * 1024).read_to_string(&mut out)?;
        stderr.take(64 * 1024).read_to_string(&mut err)?;
        let message = match status {
            None => Some("Timed out after 8 seconds. Refresh to check the job's state.".into()),
            Some(status) if !status.success() => Some(format!(
                "launchctl failed ({status}).\nSystem jobs may require running ltui with sudo; Apple jobs may be protected by macOS."
            )),
            _ => None,
        };
        if let Some(message) = message {
            return Err(CommandFailure::new(args, out, err, message).into());
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub state: State,
}

pub fn parse_services(text: &str) -> Result<BTreeMap<String, Runtime>> {
    let mut jobs = BTreeMap::new();
    let mut inside = false;
    let mut found = false;
    let mut closed = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "services = {" {
            inside = true;
            found = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line == "}" {
            closed = true;
            break;
        }
        let mut columns = line.split_whitespace();
        let (Some(pid), Some(exit), Some(label)) = (columns.next(), columns.next(), columns.next())
        else {
            bail!("Unrecognized launchctl services row: {line}")
        };
        let pid: u32 = pid.parse().context("Unrecognized launchctl PID")?;
        let numeric_exit = exit.parse::<i64>().is_ok();
        // Modern macOS also emits opaque status tags, e.g. (pe) and (jt).
        ensure!(
            exit == "-" || numeric_exit || (exit.starts_with('(') && exit.ends_with(')')),
            "Unrecognized launchctl status: {exit}"
        );
        jobs.insert(
            label.into(),
            Runtime {
                state: if pid == 0 {
                    State::Waiting
                } else {
                    State::Running(pid)
                },
            },
        );
    }
    ensure!(
        found,
        "No services section in launchctl output; runtime status is unknown"
    );
    ensure!(
        closed,
        "Incomplete services section in launchctl output; runtime status is unknown"
    );
    Ok(jobs)
}

pub fn parse_disabled(text: &str) -> Result<BTreeMap<String, bool>> {
    let body = text
        .trim()
        .strip_prefix("disabled services = {")
        .context("Unrecognized launchctl print-disabled output")?;
    let body = body
        .trim_end()
        .strip_suffix('}')
        .context("Incomplete launchctl print-disabled output")?;
    let mut result = BTreeMap::new();
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (label, value) = line
            .split_once(" => ")
            .context("Unrecognized launchctl disable override")?;
        let label = label.trim().trim_matches('"');
        let disabled = match value.trim().trim_end_matches(';') {
            "true" | "disabled" => true,
            "false" | "enabled" => false,
            _ => bail!("Unrecognized disable override: {line}"),
        };
        result.insert(label.into(), disabled);
    }
    Ok(result)
}

pub struct Inventory {
    pub jobs: Vec<Job>,
    pub warnings: Vec<String>,
}
pub struct Backend<R = Launchctl> {
    pub runner: R,
    pub home: PathBuf,
    pub agent_domain: String,
    pub demo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Load,
    Unload,
    Enable,
    Disable,
    Start,
    Restart,
    Stop,
    Kill,
    Hup,
    Reload,
    Delete,
}
impl Action {
    pub fn title(self) -> &'static str {
        match self {
            Self::Load => "Load",
            Self::Unload => "Unload",
            Self::Enable => "Enable",
            Self::Disable => "Disable",
            Self::Start => "Run now",
            Self::Restart => "Restart",
            Self::Stop => "Send SIGTERM",
            Self::Kill => "Send SIGKILL",
            Self::Hup => "Send SIGHUP",
            Self::Reload => "Reload plist",
            Self::Delete => "Delete job",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Load => "Bootstrap the plist into its domain; does not enable a disabled job.",
            Self::Unload => "Boot out this service and stop it. The plist remains on disk.",
            Self::Enable => {
                "Allow future loads; persistently overrides Disabled. Does not load or start."
            }
            Self::Disable => {
                "Prevent future loads, including after reboot. Does not stop a running job."
            }
            Self::Start => "Kickstart a loaded service immediately, regardless of its schedule.",
            Self::Restart => "Kill any running instance, then kickstart the loaded service.",
            Self::Stop => "Send SIGTERM. KeepAlive or demand may cause the job to run again.",
            Self::Kill => "Send SIGKILL. KeepAlive or demand may cause the job to run again.",
            Self::Hup => "Send SIGHUP. The program decides whether to reload, exit, or ignore it.",
            Self::Reload => "Boot out the loaded service, then bootstrap the current plist.",
            Self::Delete => "Unload if loaded, then permanently delete this plist file.",
        }
    }
    pub fn confirm(self) -> bool {
        !matches!(self, Self::Load | Self::Enable | Self::Start)
    }
    pub fn args(self, job: &Job) -> Result<Vec<String>> {
        model::validate_label(&job.label)?;
        let target = job.target();
        Ok(match self {
            Self::Load => {
                let path = job
                    .path
                    .as_ref()
                    .context("This service has no plist path")?;
                vec![
                    "bootstrap".into(),
                    job.domain.clone(),
                    path.to_string_lossy().into_owned(),
                ]
            }
            Self::Unload => vec!["bootout".into(), target],
            Self::Enable => vec!["enable".into(), target],
            Self::Disable => vec!["disable".into(), target],
            Self::Start => vec!["kickstart".into(), "-p".into(), target],
            Self::Restart => vec!["kickstart".into(), "-kp".into(), target],
            Self::Stop | Self::Kill | Self::Hup => vec![
                "kill".into(),
                match self {
                    Self::Stop => "SIGTERM",
                    Self::Kill => "SIGKILL",
                    _ => "SIGHUP",
                }
                .into(),
                target,
            ],
            _ => bail!("This action requires multiple steps"),
        })
    }
}

impl<R: Runner> Backend<R> {
    fn run(&self, args: &[&str]) -> Result<String> {
        self.runner
            .run(&args.iter().map(|s| (*s).into()).collect::<Vec<_>>())
    }
    pub fn inventory(&self, scope: Scope) -> Inventory {
        if self.demo {
            return demo_inventory(scope, &self.home, &self.agent_domain);
        }
        let domain = scope.domain(&self.agent_domain);
        let mut warnings = Vec::new();
        let services = self
            .run(&["print", domain])
            .and_then(|s| parse_services(&s));
        if let Err(e) = &services {
            warnings.push(format!("Runtime status unavailable: {e:#}"));
        }
        let overrides = self
            .run(&["print-disabled", domain])
            .and_then(|s| parse_disabled(&s));
        if let Err(e) = &overrides {
            warnings.push(format!("Enable status unavailable: {e:#}"));
        }
        let dirs = match scope.directory(&self.home) {
            Some(dir) => vec![dir],
            None if scope == Scope::Session => [Scope::User, Scope::Agents, Scope::AppleAgents]
                .iter()
                .filter_map(|s| s.directory(&self.home))
                .collect(),
            None => [Scope::Daemons, Scope::AppleDaemons]
                .iter()
                .filter_map(|s| s.directory(&self.home))
                .collect(),
        };
        let mut jobs = Vec::new();
        for dir in dirs {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    warnings.push(format!("{}: {e}", dir.display()));
                    continue;
                }
            };
            for entry in entries {
                let path = match entry {
                    Ok(e) => e.path(),
                    Err(e) => {
                        warnings.push(e.to_string());
                        continue;
                    }
                };
                if path.extension().is_none_or(|e| e != "plist") {
                    continue;
                }
                let file_bytes = fs::read(&path).ok();
                let definition = file_bytes
                    .as_ref()
                    .context("Cannot read plist file")
                    .and_then(|b| Value::from_reader(std::io::Cursor::new(b)).map_err(Into::into));
                let (definition, error) = match definition {
                    Ok(v) => {
                        let error = model::validate(&v).err().map(|e| format!("{e:#}"));
                        (Some(v), error)
                    }
                    Err(e) => (None, Some(format!("Invalid plist: {e}"))),
                };
                let label = definition
                    .as_ref()
                    .and_then(Value::as_dictionary)
                    .and_then(|d| d.get("Label"))
                    .and_then(Value::as_string)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into()
                    });
                let runtime = services.as_ref().ok().and_then(|s| s.get(&label));
                let override_disabled =
                    overrides.as_ref().ok().and_then(|o| o.get(&label)).copied();
                let disabled = overrides.as_ref().ok().map(|_| {
                    override_disabled.unwrap_or_else(|| {
                        definition
                            .as_ref()
                            .and_then(Value::as_dictionary)
                            .and_then(|d| d.get("Disabled"))
                            .and_then(Value::as_boolean)
                            .unwrap_or(false)
                    })
                });
                jobs.push(Job {
                    label,
                    domain: domain.into(),
                    path: Some(path),
                    definition,
                    file_bytes,
                    error,
                    state: runtime
                        .map(|s| s.state.clone())
                        .unwrap_or(if services.is_ok() {
                            State::Unloaded
                        } else {
                            State::Unknown
                        }),
                    disabled,
                    override_disabled,
                });
            }
        }
        if scope.directory(&self.home).is_none()
            && let Ok(services) = &services
        {
            for (label, runtime) in services {
                if jobs.iter().any(|j| &j.label == label) {
                    continue;
                }
                let override_disabled = overrides.as_ref().ok().and_then(|o| o.get(label)).copied();
                jobs.push(Job {
                    label: label.clone(),
                    domain: domain.into(),
                    path: None,
                    definition: None,
                    file_bytes: None,
                    error: None,
                    state: runtime.state.clone(),
                    disabled: overrides
                        .as_ref()
                        .ok()
                        .map(|_| override_disabled.unwrap_or(false)),
                    override_disabled,
                });
            }
        }
        jobs.sort_by(|a, b| {
            a.label
                .to_lowercase()
                .cmp(&b.label.to_lowercase())
                .then(a.path.cmp(&b.path))
        });
        let mut counts = BTreeMap::new();
        for job in &jobs {
            *counts.entry(job.label.clone()).or_insert(0) += 1;
        }
        for job in &mut jobs {
            if counts[&job.label] > 1 {
                job.error = Some("Duplicate Label in this domain; resolve the conflicting plists before loading or editing".into());
            }
        }
        Inventory { jobs, warnings }
    }
    pub fn details(&self, job: &mut Job) -> Result<String> {
        if self.demo {
            return Ok(format!(
                "{} = {{\n    state = {}\n    program = /usr/bin/true\n    runs = 12\n    last exit code = 0\n}}\n\nDEMO MODE · No real jobs are changed.",
                job.target(),
                job.state.text()
            ));
        }
        let text = self.run(&["print", &job.target()])?;
        if job.path.is_none()
            && let Some(path) = text.lines().find_map(|l| l.trim().strip_prefix("path = "))
        {
            let path = PathBuf::from(path.trim_matches('"'));
            // Only expose actual plist files, not embedded/bundle definitions.
            if path.extension().is_some_and(|e| e == "plist") && path.is_file() {
                job.definition = Value::from_file(&path).ok();
                job.error = job
                    .definition
                    .as_ref()
                    .and_then(|v| model::validate(v).err())
                    .map(|e| format!("{e:#}"));
                if job.disabled.is_some() {
                    job.disabled = Some(job.override_disabled.unwrap_or_else(|| {
                        job.definition
                            .as_ref()
                            .and_then(Value::as_dictionary)
                            .and_then(|d| d.get("Disabled"))
                            .and_then(Value::as_boolean)
                            .unwrap_or(false)
                    }));
                }
                job.file_bytes = fs::read(&path).ok();
                job.path = Some(path);
            }
        }
        Ok(text)
    }
    pub fn check_collision(
        &self,
        value: &Value,
        path: &Path,
        domain: &str,
        original_label: Option<&str>,
    ) -> Result<()> {
        let label = model::validate(value)?;
        if self.demo {
            return Ok(());
        }
        let scopes: &[Scope] = if domain == "system" {
            &[Scope::Daemons, Scope::AppleDaemons]
        } else {
            &[Scope::User, Scope::Agents, Scope::AppleAgents]
        };
        for scope in scopes {
            let dir = scope.directory(&self.home).unwrap();
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Checking Label conflicts in {}", dir.display()));
                }
            };
            for entry in entries {
                let other = entry?.path();
                if other == path || other.extension().is_none_or(|e| e != "plist") {
                    continue;
                }
                if let Ok(v) = Value::from_file(&other) {
                    ensure!(
                        v.as_dictionary()
                            .and_then(|d| d.get("Label"))
                            .and_then(Value::as_string)
                            != Some(label),
                        "Label {label} already exists in {}",
                        other.display()
                    );
                }
            }
        }
        if original_label != Some(label) {
            let services = parse_services(&self.run(&["print", domain])?)?;
            ensure!(
                !services.contains_key(label),
                "A loaded job already uses Label {label}"
            );
        }
        Ok(())
    }
    pub fn action(&self, action: Action, job: &Job) -> Result<String> {
        ensure!(!self.demo, "Demo mode: actions do not change real jobs");
        if matches!(action, Action::Load | Action::Reload) {
            let value = Value::from_file(job.path.as_ref().context("This job has no plist file")?)?;
            ensure!(
                model::validate(&value)? == job.label,
                "The plist Label changed. Refresh before loading"
            );
            self.check_collision(
                &value,
                job.path.as_ref().unwrap(),
                &job.domain,
                Some(&job.label),
            )?;
        }
        match action {
            Action::Reload => {
                let overrides = parse_disabled(&self.run(&["print-disabled", &job.domain])?)?;
                let plist_disabled = Value::from_file(job.path.as_ref().unwrap())?
                    .as_dictionary()
                    .and_then(|d| d.get("Disabled"))
                    .and_then(Value::as_boolean)
                    .unwrap_or(false);
                ensure!(
                    !overrides.get(&job.label).copied().unwrap_or(plist_disabled),
                    "This job is disabled. Enable it before reloading; the running service was left intact"
                );
                self.runner.run(&Action::Unload.args(job)?)?;
                self.runner.run(&Action::Load.args(job)?).context("Job was unloaded, but loading the new plist failed. Fix the plist, then Load it again")
            }
            Action::Delete => {
                ensure!(!job.read_only(), "Apple system plists are read-only");
                let path = job
                    .path
                    .as_ref()
                    .context("No plist file to delete; use Unload for a runtime-only service")?;
                let metadata = fs::symlink_metadata(path)?;
                ensure!(
                    metadata.is_file() || metadata.is_symlink(),
                    "Refusing to delete a non-regular file"
                );
                let bytes = fs::read(path)?;
                ensure!(
                    job.file_bytes.as_deref() == Some(bytes.as_slice()),
                    "Plist changed since it was selected. Refresh before deleting"
                );
                // Query immediately before deletion rather than trusting a stale snapshot.
                let services = parse_services(&self.run(&["print", &job.domain])?)?;
                if services.contains_key(&job.label) {
                    self.runner.run(&Action::Unload.args(job)?)?;
                }
                ensure!(
                    fs::read(path)? == bytes,
                    "Plist changed while unloading; file was retained"
                );
                fs::remove_file(path).with_context(|| {
                    format!(
                        "Deleting {} (the job may already be unloaded)",
                        path.display()
                    )
                })?;
                Ok("Plist deleted".into())
            }
            _ => self.runner.run(&action.args(job)?),
        }
    }
    pub fn save_definition(
        &self,
        value: &Value,
        path: &Path,
        bytes: Option<&[u8]>,
        job: Option<&Job>,
        domain: &str,
    ) -> Result<bool> {
        ensure!(!self.demo, "Demo mode: edits do not change real files");
        ensure!(
            !path.starts_with("/System/"),
            "Apple system plists are read-only"
        );
        let label = model::validate(value)?;
        let services = parse_services(&self.run(&["print", domain])?)?;
        let loaded = job.is_some_and(|j| services.contains_key(&j.label));
        if let Some(job) = job {
            ensure!(
                !loaded || label == job.label,
                "Unload this job before changing its Label, so the old service is not left running"
            );
        }
        self.check_collision(value, path, domain, job.map(|j| j.label.as_str()))?;
        #[cfg(unix)]
        if bytes.is_none()
            && unsafe { libc::geteuid() } == 0
            && path.parent() == Some(self.home.join("Library/LaunchAgents").as_path())
        {
            use std::os::unix::fs::MetadataExt;
            let owner = fs::metadata(&self.home)?;
            for dir in [
                self.home.join("Library"),
                self.home.join("Library/LaunchAgents"),
            ] {
                if !dir.try_exists()? {
                    fs::create_dir(&dir)?;
                    std::os::unix::fs::chown(&dir, Some(owner.uid()), Some(owner.gid()))?;
                }
            }
        }
        model::save(path, value, bytes)?;
        Ok(loaded)
    }
}

fn demo_inventory(scope: Scope, home: &Path, agent_domain: &str) -> Inventory {
    let names = [
        "local.backup",
        "local.dev-server",
        "local.cleanup",
        "local.sync",
        "local.daily-report",
        "local.old-worker",
    ];
    let jobs = names
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let mut form = model::Form::new();
            form.fields[0] = (*label).into();
            form.fields[2] = [
                "/usr/bin/rsync -a /tmp/source/ /tmp/backup/",
                "/usr/bin/python3 -m http.server 8000",
                "/usr/bin/find /tmp -name '*.tmp'",
                "/usr/bin/true",
                "/bin/sh -c 'echo daily report'",
                "/usr/bin/true",
            ][i]
                .into();
            form.fields[6] = "true".into();
            form.fields[8] = "3600".into();
            Job {
                label: (*label).into(),
                domain: scope.domain(agent_domain).into(),
                path: Some(
                    scope
                        .directory(home)
                        .unwrap_or_else(|| home.join("Library/LaunchAgents"))
                        .join(format!("{label}.plist")),
                ),
                definition: Some(form.value().unwrap()),
                file_bytes: None,
                error: None,
                state: match i {
                    0 | 2 | 4 => State::Waiting,
                    1 => State::Running(4821),
                    3 => State::Running(5092),
                    _ => State::Unloaded,
                },
                disabled: Some(i == 5),
                override_disabled: if i == 5 { Some(true) } else { None },
            }
        })
        .collect();
    Inventory {
        jobs,
        warnings: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[cfg(target_os = "macos")]
    #[test]
    fn real_read_only_launchctl_failure_keeps_command_output_and_explanation_separate() {
        let args = vec!["print".into(), "ltui-invalid-domain-for-test".into()];
        let error = Launchctl.run(&args).unwrap_err();
        let failure = error.downcast_ref::<CommandFailure>().unwrap();
        assert_eq!(
            failure.command,
            "launchctl print ltui-invalid-domain-for-test"
        );
        assert!(!failure.stdout.is_empty() || !failure.stderr.is_empty());
        assert!(failure.message.contains("launchctl failed"));
        assert!(
            !failure
                .stderr
                .contains("System jobs may require running ltui")
        );
    }
    struct Fake {
        calls: RefCell<Vec<Vec<String>>>,
        fail: Option<String>,
        disabled: bool,
    }
    impl Runner for Fake {
        fn run(&self, args: &[String]) -> Result<String> {
            self.calls.borrow_mut().push(args.to_vec());
            if self.fail.as_deref() == Some(&args[0]) {
                bail!("simulated failure");
            }
            Ok(if args[0] == "print-disabled" {
                format!(
                    "disabled services = {{\n \"local.my-job\" => {}\n}}",
                    if self.disabled { "disabled" } else { "enabled" }
                )
            } else if args[0] == "print" {
                "services = {\n 0 0 local.my-job\n}".into()
            } else {
                String::new()
            })
        }
    }
    fn backend(home: &Path, fail: Option<&str>) -> Backend<Fake> {
        Backend {
            runner: Fake {
                calls: RefCell::new(Vec::new()),
                fail: fail.map(str::to_owned),
                disabled: true,
            },
            home: home.into(),
            agent_domain: "gui/501".into(),
            demo: false,
        }
    }
    #[test]
    fn parses_live_and_legacy_disable_formats() {
        let s = parse_services("gui/501 = {\n services = {\n 42 - local.running\n 0 -9 local.idle\n 12 (jt) local.tagged\n }\n}").unwrap();
        assert_eq!(s["local.running"].state, State::Running(42));

        assert_eq!(s["local.tagged"].state, State::Running(12));

        let d =
            parse_disabled("disabled services = {\n\"a\" => disabled\n\"b\" => false\n}").unwrap();
        assert!(d["a"]);
        assert!(!d["b"]);
        assert!(parse_services("unexpected output").is_err());
        assert!(parse_services("services = {\nwat\n}").is_err());
    }
    #[test]
    fn failed_runtime_queries_do_not_claim_unloaded_or_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Library/LaunchAgents/local.my-job.plist");
        model::save(&path, &model::Form::new().value().unwrap(), None).unwrap();
        let b = backend(dir.path(), Some("print"));
        let inventory = b.inventory(Scope::User);
        assert_eq!(inventory.jobs[0].state, State::Unknown);
        assert_eq!(inventory.jobs[0].disabled, Some(true));
        let b = backend(dir.path(), Some("print-disabled"));
        assert_eq!(b.inventory(Scope::User).jobs[0].disabled, None);
    }
    #[test]
    fn actions_always_target_one_service_and_preserve_files_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Library/LaunchAgents/local.my-job.plist");
        model::save(&path, &model::Form::new().value().unwrap(), None).unwrap();
        let mut b = backend(dir.path(), Some("bootout"));
        b.runner.disabled = false;
        let job = b.inventory(Scope::User).jobs.remove(0);
        assert_eq!(
            Action::Unload.args(&job).unwrap(),
            ["bootout", "gui/501/local.my-job"]
        );
        assert_eq!(Action::Load.args(&job).unwrap()[1], "gui/501");
        assert!(b.action(Action::Delete, &job).is_err());
        assert!(path.exists());
        assert!(b.action(Action::Reload, &job).is_err());
        assert!(!b.runner.calls.borrow().iter().any(|a| a[0] == "bootstrap"));
    }
    #[test]
    fn reload_and_delete_order_and_bootstrap_failure_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Library/LaunchAgents/local.my-job.plist");
        model::save(&path, &model::Form::new().value().unwrap(), None).unwrap();
        let mut b = backend(dir.path(), Some("bootstrap"));
        b.runner.disabled = false;
        let job = b.inventory(Scope::User).jobs.remove(0);
        let error = b.action(Action::Reload, &job).unwrap_err();
        assert!(error.to_string().contains("Job was unloaded"));
        let calls = b.runner.calls.borrow();
        let mutations: Vec<_> = calls
            .iter()
            .filter(|a| a[0] == "bootout" || a[0] == "bootstrap")
            .map(|a| a[0].as_str())
            .collect();
        assert_eq!(mutations, ["bootout", "bootstrap"]);
        drop(calls);
        assert!(path.exists());
        b.runner.fail = None;
        b.action(Action::Delete, &job).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn save_rejects_live_renames_and_duplicate_labels_then_round_trips_options() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Library/LaunchAgents/local.my-job.plist");
        let mut form = model::Form::new();
        model::save(&path, &form.value().unwrap(), None).unwrap();
        let bytes = fs::read(&path).unwrap();
        let b = backend(dir.path(), None);
        let job = b.inventory(Scope::User).jobs.remove(0);
        form.fields[0] = "local.renamed".into();
        assert!(
            b.save_definition(
                &form.value().unwrap(),
                &path,
                Some(&bytes),
                Some(&job),
                "gui/501"
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        form.fields[0] = "local.my-job".into();
        form.fields[8] = "60".into();
        assert!(
            b.save_definition(
                &form.value().unwrap(),
                &path,
                Some(&bytes),
                Some(&job),
                "gui/501"
            )
            .unwrap()
        );
        let new_path = path.with_file_name("another.plist");
        assert!(
            b.save_definition(&form.value().unwrap(), &new_path, None, None, "gui/501")
                .is_err()
        );
        assert!(!new_path.exists());
    }
    #[test]
    fn enabled_override_wins_over_disabled_plist_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Library/LaunchAgents/local.my-job.plist");
        let mut v = model::Form::new().value().unwrap();
        v.as_dictionary_mut()
            .unwrap()
            .insert("Disabled".into(), Value::Boolean(false));
        model::save(&path, &v, None).unwrap();
        let b = backend(dir.path(), None);
        assert_eq!(b.inventory(Scope::User).jobs[0].disabled, Some(true));
        assert!(
            b.action(Action::Reload, &b.inventory(Scope::User).jobs[0])
                .is_err()
        );
        assert!(!b.runner.calls.borrow().iter().any(|a| a[0] == "bootout"));
    }
}
