use anyhow::{Context, Result, bail, ensure};
use plist::{Dictionary, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Agents,
    Daemons,
    AppleAgents,
    AppleDaemons,
    Session,
    System,
}

impl Scope {
    pub const ALL: [Self; 7] = [
        Self::User,
        Self::Agents,
        Self::Daemons,
        Self::AppleAgents,
        Self::AppleDaemons,
        Self::Session,
        Self::System,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::User => "User agents",
            Self::Agents => "Shared agents",
            Self::Daemons => "Daemons",
            Self::AppleAgents => "Apple agents",
            Self::AppleDaemons => "Apple daemons",
            Self::Session => "Session services",
            Self::System => "System services",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "user" => Ok(Self::User),
            "agents" => Ok(Self::Agents),
            "daemons" => Ok(Self::Daemons),
            "apple-agents" => Ok(Self::AppleAgents),
            "apple-daemons" => Ok(Self::AppleDaemons),
            "session" => Ok(Self::Session),
            "system" => Ok(Self::System),
            _ => bail!("Unknown scope: {s}"),
        }
    }
    pub fn directory(self, home: &Path) -> Option<PathBuf> {
        Some(match self {
            Self::User => home.join("Library/LaunchAgents"),
            Self::Agents => "/Library/LaunchAgents".into(),
            Self::Daemons => "/Library/LaunchDaemons".into(),
            Self::AppleAgents => "/System/Library/LaunchAgents".into(),
            Self::AppleDaemons => "/System/Library/LaunchDaemons".into(),
            _ => return None,
        })
    }
    pub fn is_system(self) -> bool {
        matches!(self, Self::Daemons | Self::AppleDaemons | Self::System)
    }
    pub fn is_apple(self) -> bool {
        matches!(self, Self::AppleAgents | Self::AppleDaemons)
    }
    pub fn domain(self, agent_domain: &str) -> &str {
        if self.is_system() {
            "system"
        } else {
            agent_domain
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Running(u32),
    Waiting,
    Unloaded,
    Unknown,
}
impl State {
    pub fn text(&self) -> String {
        match self {
            Self::Running(pid) => format!("running ({pid})"),
            Self::Waiting => "loaded".into(),
            Self::Unloaded => "unloaded".into(),
            Self::Unknown => "unknown".into(),
        }
    }
    pub fn loaded(&self) -> bool {
        matches!(self, Self::Running(_) | Self::Waiting)
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub label: String,
    pub domain: String,
    pub path: Option<PathBuf>,
    pub definition: Option<Value>,
    pub file_bytes: Option<Vec<u8>>,
    pub error: Option<String>,
    pub state: State,
    pub disabled: Option<bool>,
    pub override_disabled: Option<bool>,
}
impl Job {
    pub fn target(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }
    pub fn enabled_text(&self) -> &'static str {
        match self.disabled {
            Some(true) => "disabled",
            Some(false) => "enabled",
            None => "unknown",
        }
    }
    pub fn command(&self) -> String {
        let Some(d) = self.definition.as_ref().and_then(Value::as_dictionary) else {
            return String::new();
        };
        if let Some(args) = d.get("ProgramArguments").and_then(Value::as_array) {
            return shell_words::join(args.iter().filter_map(Value::as_string));
        }
        d.get("Program")
            .and_then(Value::as_string)
            .unwrap_or("")
            .into()
    }
    pub fn read_only(&self) -> bool {
        self.path.as_ref().is_some_and(|p| {
            p.starts_with("/System/")
                || fs::canonicalize(p).is_ok_and(|p| p.starts_with("/System/"))
        })
    }
}

pub fn validate_label(label: &str) -> Result<()> {
    ensure!(
        !label.is_empty() && label.trim() == label,
        "Label must not be empty or have surrounding spaces"
    );
    ensure!(
        label != "."
            && label != ".."
            && !label
                .chars()
                .any(|c| c == '/' || c == ':' || c.is_control() || c.is_whitespace()),
        "Label cannot contain whitespace, slashes, colons, or control characters"
    );
    Ok(())
}

pub fn validate(value: &Value) -> Result<&str> {
    let d = value
        .as_dictionary()
        .context("A launchd plist must be a dictionary")?;
    let label = d
        .get("Label")
        .and_then(Value::as_string)
        .context("Label must be a string")?;
    validate_label(label)?;
    let program = d.get("Program");
    if let Some(p) = program {
        ensure!(
            p.as_string()
                .is_some_and(|s| !s.is_empty() && !s.contains('\0')),
            "Program must be a nonempty string without NUL characters"
        );
    }
    let args = d.get("ProgramArguments");
    if let Some(args) = args {
        let a = args
            .as_array()
            .context("ProgramArguments must be an array")?;
        ensure!(
            !a.is_empty()
                && a.iter()
                    .all(|v| v.as_string().is_some_and(|s| !s.contains('\0'))),
            "ProgramArguments must contain strings without NUL characters"
        );
        ensure!(
            a[0].as_string().is_some_and(|s| !s.is_empty()),
            "The first program argument cannot be empty"
        );
    }
    // BundleProgram is used by jobs installed with SMAppService.
    ensure!(
        program.is_some()
            || args.is_some()
            || d.get("BundleProgram").and_then(Value::as_string).is_some(),
        "Set Program or ProgramArguments"
    );
    for key in [
        "RunAtLoad",
        "Disabled",
        "LaunchOnlyOnce",
        "AbandonProcessGroup",
    ] {
        if let Some(v) = d.get(key) {
            ensure!(v.as_boolean().is_some(), "{key} must be a boolean");
        }
    }
    if let Some(v) = d.get("KeepAlive") {
        ensure!(
            v.as_boolean().is_some() || v.as_dictionary().is_some(),
            "KeepAlive must be a boolean or dictionary"
        );
    }
    if let Some(v) = d.get("StartInterval") {
        ensure!(
            v.as_unsigned_integer().is_some_and(|n| n > 0),
            "StartInterval must be a positive integer"
        );
    }
    if let Some(v) = d.get("StartCalendarInterval") {
        validate_calendar(v)?;
    }
    if let Some(v) = d.get("ThrottleInterval") {
        ensure!(
            v.as_unsigned_integer().is_some(),
            "ThrottleInterval must be a nonnegative integer"
        );
    }
    for key in [
        "WorkingDirectory",
        "StandardOutPath",
        "StandardErrorPath",
        "UserName",
        "GroupName",
    ] {
        if let Some(v) = d.get(key) {
            ensure!(v.as_string().is_some(), "{key} must be a string");
        }
    }
    if let Some(v) = d.get("EnvironmentVariables") {
        let env = v
            .as_dictionary()
            .context("EnvironmentVariables must be a dictionary")?;
        ensure!(
            env.iter().all(|(k, v)| !k.is_empty()
                && !k.contains(['=', '\0'])
                && v.as_string().is_some_and(|s| !s.contains('\0'))),
            "Environment variables need valid names and string values"
        );
    }
    Ok(label)
}

pub fn xml(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    value.to_writer_xml(&mut bytes)?;
    Ok(bytes)
}

/// Replace only after validation; preserve symlinks by editing their targets.
pub fn save(path: &Path, value: &Value, expected: Option<&[u8]>) -> Result<()> {
    validate(value)?;
    let resolved = if expected.is_some() {
        fs::canonicalize(path)?
    } else {
        path.into()
    };
    let path = resolved.as_path();
    ensure!(
        !path.starts_with("/System/"),
        "Apple system plists are read-only"
    );
    let parent = path.parent().context("Missing parent directory")?;
    if let Some(old) = expected {
        let meta =
            fs::symlink_metadata(path).with_context(|| format!("Reading {}", path.display()))?;
        ensure!(meta.is_file(), "Refusing to replace a non-regular file");
        ensure!(
            fs::read(path)? == old,
            "The plist changed on disk. Cancel and reopen the edit"
        );
    } else {
        ensure!(!path.try_exists()?, "{} already exists", path.display());
    }
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&xml(value)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = if expected.is_some() {
            let meta = fs::metadata(path)?;
            // The replacement must keep both owner and group, even for an
            // unprivileged save. Keep its private mode until that succeeds.
            let current = temp.as_file().metadata()?;
            let uid = (current.uid() != meta.uid()).then_some(meta.uid());
            let gid = (current.gid() != meta.gid()).then_some(meta.gid());
            if uid.is_some() || gid.is_some() {
                std::os::unix::fs::fchown(temp.as_file(), uid, gid).with_context(|| {
                    format!(
                        "Cannot preserve ownership of {}; original plist was not replaced",
                        path.display()
                    )
                })?;
            }
            let retained = temp.as_file().metadata()?;
            ensure!(
                retained.uid() == meta.uid() && retained.gid() == meta.gid(),
                "Cannot preserve ownership of {}; original plist was not replaced",
                path.display()
            );
            meta.mode() & 0o777
        } else {
            if unsafe { libc::geteuid() } == 0 {
                let owner = fs::metadata(parent)?;
                std::os::unix::fs::chown(temp.path(), Some(owner.uid()), Some(owner.gid()))?;
            }
            0o644
        };
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    temp.as_file().sync_all()?;
    if expected.is_some() {
        temp.persist(path).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|e| e.error)?;
    }
    Ok(())
}

pub const FIELD_NAMES: [&str; 11] = [
    "Label",
    "Program (optional executable override)",
    "ProgramArguments",
    "WorkingDirectory",
    "StandardOutPath",
    "StandardErrorPath",
    "RunAtLoad (true / false / blank)",
    "KeepAlive (true / false / blank)",
    "StartInterval (seconds)",
    "StartCalendarInterval (one schedule per line)",
    "EnvironmentVariables (KEY=value)",
];
const KEYS: [&str; 11] = [
    "Label",
    "Program",
    "ProgramArguments",
    "WorkingDirectory",
    "StandardOutPath",
    "StandardErrorPath",
    "RunAtLoad",
    "KeepAlive",
    "StartInterval",
    "StartCalendarInterval",
    "EnvironmentVariables",
];
const COMPLEX: &str = "<dictionary: preserved; use $EDITOR to edit>";

const CALENDAR_PARTS: [(&str, u64, u64); 5] = [
    ("Hour", 0, 23),
    ("Minute", 0, 59),
    ("Weekday", 0, 7),
    ("Day", 1, 31),
    ("Month", 1, 12),
];

fn validate_calendar(value: &Value) -> Result<()> {
    let schedules = match value {
        Value::Array(schedules) => schedules.as_slice(),
        _ => std::slice::from_ref(value),
    };
    for schedule in schedules {
        let d = schedule
            .as_dictionary()
            .context("StartCalendarInterval must be a dictionary or array of dictionaries")?;
        for (key, value) in d {
            let (_, min, max) = CALENDAR_PARTS
                .iter()
                .find(|(name, _, _)| *name == key)
                .with_context(|| {
                    format!("Unknown calendar key {key}; use Hour, Minute, Weekday, Day, or Month")
                })?;
            ensure!(
                value
                    .as_unsigned_integer()
                    .is_some_and(|n| (*min..=*max).contains(&n)),
                "StartCalendarInterval: {key} must be an integer from {min} to {max}"
            );
        }
    }
    Ok(())
}

fn calendar_text(value: &Value) -> Result<String> {
    validate_calendar(value)?;
    if value.as_array().is_some_and(Vec::is_empty) {
        return Ok("[]".into());
    }
    let schedules = match value {
        Value::Array(schedules) => schedules.as_slice(),
        _ => std::slice::from_ref(value),
    };
    Ok(schedules
        .iter()
        .map(|schedule| {
            let d = schedule.as_dictionary().unwrap();
            if d.is_empty() {
                return "*".into();
            }
            CALENDAR_PARTS
                .iter()
                .filter_map(|(key, _, _)| {
                    d.get(key)
                        .map(|v| format!("{key}={}", v.as_unsigned_integer().unwrap()))
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn parse_calendar(text: &str, preserve_array: bool) -> Result<Value> {
    if text.trim() == "[]" {
        return Ok(Value::Array(Vec::new()));
    }
    let mut schedules = Vec::new();
    for line in text.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let mut d = Dictionary::new();
        if line != "*" {
            for part in line.split_whitespace() {
                let (key, value) = part.split_once('=').context(
                    "Calendar format: Hour=23 Minute=31; use a new line for each schedule",
                )?;
                let (name, _, _) = CALENDAR_PARTS
                    .iter()
                    .find(|(name, _, _)| name.eq_ignore_ascii_case(key))
                    .with_context(|| {
                        format!(
                            "Unknown calendar key {key}; use Hour, Minute, Weekday, Day, or Month"
                        )
                    })?;
                ensure!(
                    !d.contains_key(name),
                    "Calendar key {name} appears twice in one schedule"
                );
                d.insert(
                    (*name).into(),
                    Value::Integer(
                        value
                            .parse::<u64>()
                            .with_context(|| format!("Calendar {name} must be an integer"))?
                            .into(),
                    ),
                );
            }
        }
        schedules.push(Value::Dictionary(d));
    }
    ensure!(
        !schedules.is_empty(),
        "Enter a calendar schedule or leave the field blank"
    );
    let value = if schedules.len() == 1 && !preserve_array {
        schedules.pop().unwrap()
    } else {
        Value::Array(schedules)
    };
    validate_calendar(&value)?;
    Ok(value)
}

#[derive(Clone)]
pub struct Form {
    pub fields: Vec<String>,
    pub focus: usize,
    pub cursor: usize,
    pub original: Value,
    pub path: Option<PathBuf>,
    pub bytes: Option<Vec<u8>>,
    pub job: Option<Job>,
    pub error: Option<String>,
}
impl Form {
    pub fn new() -> Self {
        let mut d = Dictionary::new();
        d.insert("Label".into(), Value::String("local.my-job".into()));
        d.insert(
            "ProgramArguments".into(),
            Value::Array(vec![Value::String("/usr/bin/true".into())]),
        );
        Self::from_value(Value::Dictionary(d), None, None, None)
    }
    pub fn from_value(
        original: Value,
        path: Option<PathBuf>,
        bytes: Option<Vec<u8>>,
        job: Option<Job>,
    ) -> Self {
        let fields: Vec<String> = KEYS
            .iter()
            .map(|k| {
                let Some(v) = original.as_dictionary().and_then(|d| d.get(k)) else {
                    return String::new();
                };
                if *k == "StartCalendarInterval" {
                    return calendar_text(v).unwrap_or_else(|_| COMPLEX.into());
                }
                match v {
                    Value::String(s) => s.clone(),
                    Value::Boolean(b) => b.to_string(),
                    Value::Integer(n) => n.to_string(),
                    Value::Array(a) => shell_words::join(a.iter().filter_map(Value::as_string)),
                    Value::Dictionary(d) if *k == "EnvironmentVariables" => shell_words::join(
                        d.iter()
                            .map(|(k, v)| format!("{k}={}", v.as_string().unwrap_or(""))),
                    ),
                    _ => COMPLEX.into(),
                }
            })
            .collect();
        let cursor = fields[0].len();
        Self {
            fields,
            focus: 0,
            cursor,
            original,
            path,
            bytes,
            job,
            error: None,
        }
    }
    pub fn focus(&mut self, index: usize) {
        self.focus = index;
        self.cursor = self.fields[index].len();
    }
    pub fn insert(&mut self, s: &str) {
        self.fields[self.focus].insert_str(self.cursor, s);
        self.cursor += s.len();
    }
    pub fn accepts_newlines(&self) -> bool {
        matches!(
            KEYS[self.focus],
            "ProgramArguments" | "StartCalendarInterval" | "EnvironmentVariables"
        ) || self.fields[self.focus].contains('\n')
    }
    pub fn field_help(&self) -> &'static str {
        match KEYS[self.focus] {
            "ProgramArguments" => {
                "Example: /usr/bin/head -n10 '/tmp/my file.txt'\nUse quotes for arguments with spaces. Does not support shell expansion."
            }
            "StartCalendarInterval" => {
                "Example: Hour=23 Minute=31 (daily 23:31).\nMissing fields are wildcards. Semantics are similar to crontab(5).\nAllowed fields are Minute, Hour, Day, Weekday, and Month."
            }
            "EnvironmentVariables" => {
                // "Use KEY=value pairs, separated by spaces or newlines.\nQuote spaces: MODE=dev MESSAGE='hello world'\nEmpty value: EMPTY=''. No export, commas, or expansion.\nBlank clears all variables."
                "Example: MODE=dev MESSAGE='hello world'\nNo export, commas, or expansion."
            }
            _ => {
                // "Label changes keep the filename. Unload before renaming.\nEnter opens the full plist in $EDITOR from the job list."
                ""
            }
        }
    }
    pub fn paste(&mut self, s: &str) {
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        if self.accepts_newlines() {
            self.insert(&normalized);
        } else {
            self.insert(&normalized.replace('\n', " "));
        }
    }
    pub fn home(&mut self) {
        self.cursor = self.fields[self.focus][..self.cursor]
            .rfind('\n')
            .map_or(0, |i| i + 1);
    }
    pub fn end(&mut self) {
        self.cursor += self.fields[self.focus][self.cursor..]
            .find('\n')
            .unwrap_or(self.fields[self.focus].len() - self.cursor);
    }
    /// Move between logical lines, keeping the character column when possible.
    /// Return false at the first/last line so arrows can move to another field.
    pub fn move_line(&mut self, down: bool) -> bool {
        let value = &self.fields[self.focus];
        let start = value[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let column = value[start..self.cursor].chars().count();
        let (next_start, next_end) = if down {
            let Some(end) = value[self.cursor..].find('\n').map(|i| self.cursor + i) else {
                return false;
            };
            let next_start = end + 1;
            let next_end = value[next_start..]
                .find('\n')
                .map_or(value.len(), |i| next_start + i);
            (next_start, next_end)
        } else {
            if start == 0 {
                return false;
            }
            let next_end = start - 1;
            let next_start = value[..next_end].rfind('\n').map_or(0, |i| i + 1);
            (next_start, next_end)
        };
        self.cursor = value[next_start..next_end]
            .char_indices()
            .nth(column)
            .map_or(next_end, |(offset, _)| next_start + offset);
        true
    }
    pub fn left(&mut self) {
        self.cursor = self.fields[self.focus][..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
    }
    pub fn right(&mut self) {
        self.cursor += self.fields[self.focus][self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    }
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.fields[self.focus].replace_range(self.cursor..end, "");
    }
    pub fn delete(&mut self) {
        let start = self.cursor;
        self.right();
        let end = self.cursor;
        self.cursor = start;
        self.fields[self.focus].replace_range(start..end, "");
    }
    pub fn value(&self) -> Result<Value> {
        let mut d = self
            .original
            .as_dictionary()
            .cloned()
            .context("Plist must be a dictionary; use $EDITOR to repair it")?;
        for (i, key) in KEYS.iter().enumerate() {
            let s = &self.fields[i];
            if s == COMPLEX {
                continue;
            }
            if s.is_empty() || (*key == "StartCalendarInterval" && s.trim().is_empty()) {
                d.remove(key);
                continue;
            }
            let v = match *key {
                "ProgramArguments" => Value::Array(
                    shell_words::split(s)
                        .context("Invalid quoting in ProgramArguments")?
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
                "RunAtLoad" | "KeepAlive" => Value::Boolean(
                    s.parse()
                        .with_context(|| format!("{key}: enter true, false, or leave blank"))?,
                ),
                "StartInterval" => Value::Integer(
                    s.parse::<u64>()
                        .with_context(|| format!("{key}: enter a nonnegative integer"))?
                        .into(),
                ),
                "StartCalendarInterval" => parse_calendar(
                    s,
                    self.original
                        .as_dictionary()
                        .and_then(|d| d.get(key))
                        .is_some_and(|v| v.as_array().is_some()),
                )?,
                "EnvironmentVariables" => {
                    let mut env = Dictionary::new();
                    for pair in
                        shell_words::split(s).context("Invalid quoting in environment variables")?
                    {
                        let (key, value) = pair
                            .split_once('=')
                            .context("Use KEY=value pairs separated by spaces or newlines, e.g. MODE=dev MESSAGE='hello world'")?;
                        env.insert(key.into(), Value::String(value.into()));
                    }
                    Value::Dictionary(env)
                }
                _ => Value::String(s.clone()),
            };
            d.insert((*key).into(), v);
        }
        let value = Value::Dictionary(d);
        validate(&value)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn form_preserves_advanced_keys_and_argument_boundaries() {
        let mut f = Form::new();
        let d = f.original.as_dictionary_mut().unwrap();
        d.insert(
            "StartCalendarInterval".into(),
            Value::Dictionary(Dictionary::new()),
        );
        d.insert("KeepAlive".into(), Value::Dictionary(Dictionary::new()));
        d.insert("ThrottleInterval".into(), Value::Integer(30.into()));
        d.insert("UserName".into(), Value::String("worker".into()));
        d.insert("GroupName".into(), Value::String("staff".into()));
        let mut f = Form::from_value(f.original, None, None, None);
        f.fields[2] = "/bin/echo 'hello world' '' '$HOME'".into();
        f.fields[10] = "NAME='hello world' EMPTY=''".into();
        let value = f.value().unwrap();
        let d = value.as_dictionary().unwrap();
        assert!(d.contains_key("StartCalendarInterval"));
        assert!(d["KeepAlive"].as_dictionary().is_some());
        assert_eq!(d["ThrottleInterval"].as_unsigned_integer(), Some(30));
        assert_eq!(d["UserName"].as_string(), Some("worker"));
        assert_eq!(d["GroupName"].as_string(), Some("staff"));
        assert_eq!(
            d["ProgramArguments"].as_array().unwrap()[1].as_string(),
            Some("hello world")
        );
        assert_eq!(
            d["ProgramArguments"].as_array().unwrap()[3].as_string(),
            Some("$HOME")
        );
        assert_eq!(
            d["EnvironmentVariables"].as_dictionary().unwrap()["NAME"].as_string(),
            Some("hello world")
        );
    }
    #[test]
    fn saves_are_validated_and_do_not_clobber_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.plist");
        let v = Form::new().value().unwrap();
        save(&path, &v, None).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(save(&path, &v, None).is_err());
        fs::write(&path, "other edit").unwrap();
        assert!(save(&path, &v, Some(&bytes)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "other edit");
        assert!(save(&path, &Value::String("bad".into()), Some(b"other edit")).is_err());
    }
    #[test]
    fn rejects_path_traversal_and_bad_types() {
        for label in ["../bad", "foo/bar", "", "job\n", ".."] {
            assert!(validate_label(label).is_err());
        }
        let mut f = Form::new();
        f.fields[8] = "0".into();
        assert!(f.value().is_err());
        f.fields[8] = "60".into();
        f.fields[2] = "'unclosed".into();
        assert!(f.value().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn atomic_edit_preserves_symlink_target_and_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.plist");
        let link = dir.path().join("agent.plist");
        let mut form = Form::new();
        let v = form.value().unwrap();
        save(&target, &v, None).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &link).unwrap();
        let old = fs::read(&link).unwrap();
        form.fields[8] = "60".into();
        save(&link, &form.value().unwrap(), Some(&old)).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            Value::from_file(&target).unwrap().as_dictionary().unwrap()["StartInterval"]
                .as_unsigned_integer(),
            Some(60)
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(unix)]
    #[test]
    fn saves_preserve_a_group_that_differs_from_the_parent_directory() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, chown, symlink};
        let dir = tempfile::tempdir().unwrap();
        let parent_gid = fs::metadata(dir.path()).unwrap().gid();
        // Exercise a real unprivileged group change when the account belongs
        // to more than one group, as in the reported staff/admin case.
        let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        assert!(count >= 0, "{}", std::io::Error::last_os_error());
        let mut groups = vec![0; count as usize];
        let count = unsafe { libc::getgroups(count, groups.as_mut_ptr()) };
        assert!(count >= 0, "{}", std::io::Error::last_os_error());
        groups.truncate(count as usize);
        let Some(gid) = groups.into_iter().find(|gid| *gid != parent_gid) else {
            eprintln!("Skipping alternate-group regression: this account has no other group");
            return;
        };
        for through_symlink in [false, true] {
            let path = dir.path().join(format!("job-{through_symlink}.plist"));
            let mut form = Form::new();
            save(&path, &form.value().unwrap(), None).unwrap();
            chown(&path, None, Some(gid)).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            let original = fs::metadata(&path).unwrap();
            let old = fs::read(&path).unwrap();
            let edit_path = if through_symlink {
                let link = dir.path().join("linked-job.plist");
                symlink(&path, &link).unwrap();
                link
            } else {
                path.clone()
            };
            form.fields[8] = "60".into();
            save(&edit_path, &form.value().unwrap(), Some(&old)).unwrap();
            let saved = fs::metadata(&path).unwrap();
            assert_eq!(
                (saved.uid(), saved.gid(), saved.mode() & 0o777),
                (original.uid(), original.gid(), 0o640)
            );
            assert_eq!(Value::from_file(&path).unwrap(), form.value().unwrap());
            if through_symlink {
                assert!(fs::symlink_metadata(edit_path).unwrap().is_symlink());
            }
        }
    }
    #[test]
    fn cursor_edits_preserve_unicode_boundaries() {
        let mut form = Form::new();
        form.fields[0].clear();
        form.cursor = 0;
        form.insert("aé界z");
        form.left();
        form.left();
        form.backspace();
        assert_eq!(form.fields[0], "a界z");
        form.insert("é");
        form.delete();
        assert_eq!(form.fields[0], "aéz");
    }
    #[test]
    fn multiline_script_arguments_round_trip_without_changing_argv() {
        let script = "\nexec > >(tee -a \"$HOME/Logs/example.log\") 2>&1\nprintf 'hello\\n'\necho \"世界\"\n";
        let mut value = Form::new().value().unwrap();
        value.as_dictionary_mut().unwrap().insert(
            "ProgramArguments".into(),
            Value::Array(
                ["/bin/bash", "-c", script]
                    .into_iter()
                    .map(|s| Value::String(s.into()))
                    .collect(),
            ),
        );
        let mut form = Form::from_value(value.clone(), None, None, None);
        assert_eq!(form.value().unwrap(), value);
        form.focus(2);
        form.cursor = form.fields[2].find("printf").unwrap();
        form.paste("# first comment\r\n# second comment\r\n");
        let edited = form.value().unwrap();
        let args = edited.as_dictionary().unwrap()["ProgramArguments"]
            .as_array()
            .unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0].as_string(), Some("/bin/bash"));
        assert_eq!(args[1].as_string(), Some("-c"));
        assert_eq!(
            args[2].as_string(),
            Some(
                script
                    .replace("printf", "# first comment\n# second comment\nprintf")
                    .as_str()
            )
        );
    }
    #[test]
    fn multiline_cursor_navigation_stays_within_character_boundaries() {
        let mut form = Form::new();
        form.fields[2] = "ab\naé界z\nlast\n".into();
        form.focus(2);
        assert!(!form.move_line(true));
        assert!(form.move_line(false)); // beginning of last
        form.right();
        form.right();
        assert!(form.move_line(false)); // same character column in aé界z
        assert_eq!(&form.fields[2][..form.cursor], "ab\naé");
        form.insert("\n");
        form.end();
        assert_eq!(&form.fields[2][..form.cursor], "ab\naé\n界z");
        form.home();
        assert_eq!(&form.fields[2][..form.cursor], "ab\naé\n");
        form.backspace();
        assert_eq!(form.fields[2], "ab\naé界z\nlast\n");
    }
    fn schedule(parts: &[(&str, u64)]) -> Value {
        Value::Dictionary(
            parts
                .iter()
                .map(|(key, value)| (String::from(*key), Value::Integer((*value).into())))
                .collect(),
        )
    }
    #[test]
    fn calendar_schedules_round_trip_including_arrays_and_wildcards() {
        let daily = schedule(&[("Hour", 23), ("Minute", 31)]);
        for calendar in [
            daily.clone(),
            Value::Array(vec![daily.clone()]),
            Value::Array(vec![daily, schedule(&[("Weekday", 1), ("Hour", 9)])]),
            schedule(&[]),
            Value::Array(Vec::new()),
        ] {
            let mut original = Form::new().value().unwrap();
            original
                .as_dictionary_mut()
                .unwrap()
                .insert("StartCalendarInterval".into(), calendar);
            let form = Form::from_value(original.clone(), None, None, None);
            assert_eq!(form.value().unwrap(), original);
        }
    }
    #[test]
    fn calendar_form_adds_edits_and_removes_schedules() {
        let mut form = Form::new();
        form.fields[9] = "Hour=23 Minute=31".into();
        let value = form.value().unwrap();
        assert_eq!(
            value.as_dictionary().unwrap()["StartCalendarInterval"],
            schedule(&[("Hour", 23), ("Minute", 31)])
        );
        form.fields[9] = "Weekday=1 Hour=9 Minute=0\nweekday=5 hour=17 minute=0".into();
        assert_eq!(
            form.value().unwrap().as_dictionary().unwrap()["StartCalendarInterval"],
            Value::Array(vec![
                schedule(&[("Weekday", 1), ("Hour", 9), ("Minute", 0)]),
                schedule(&[("Weekday", 5), ("Hour", 17), ("Minute", 0)])
            ])
        );
        form.fields[9] = "*".into();
        assert_eq!(
            form.value().unwrap().as_dictionary().unwrap()["StartCalendarInterval"],
            schedule(&[])
        );
        form.fields[9] = " \n ".into();
        assert!(
            !form
                .value()
                .unwrap()
                .as_dictionary()
                .unwrap()
                .contains_key("StartCalendarInterval")
        );
        for invalid in [
            "23:31",
            "Hour=24",
            "Minute=60",
            "Day=0",
            "Month=13",
            "Weekday=8",
            "Year=2026",
            "Hour=1 hour=2",
            "Minute=-1",
            "* Hour=9",
        ] {
            form.fields[9] = invalid.into();
            assert!(
                form.value().is_err(),
                "Accepted invalid calendar: {invalid}"
            );
        }
    }
    #[test]
    fn environment_help_example_accepts_spaces_newlines_and_empty_values() {
        let mut form = Form::new();
        form.fields[10] = "MODE=dev MESSAGE='hello world'\nEMPTY=''\nLITERAL='$HOME'".into();
        let value = form.value().unwrap();
        let env = value.as_dictionary().unwrap()["EnvironmentVariables"]
            .as_dictionary()
            .unwrap();
        assert_eq!(env["MESSAGE"].as_string(), Some("hello world"));
        assert_eq!(env["EMPTY"].as_string(), Some(""));
        assert_eq!(env["LITERAL"].as_string(), Some("$HOME"));
        form.focus(10);
        assert!(form.field_help().contains("MESSAGE='hello world'"));
        form.fields[10] = "export MODE=dev".into();
        assert!(form.value().unwrap_err().to_string().contains("KEY=value"));
    }
}
