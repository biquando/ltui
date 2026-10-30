use std::fmt;

/// Keep subprocess output separate from explanations added by ltui.
#[derive(Debug)]
pub struct CommandFailure {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub message: String,
}
impl CommandFailure {
    pub fn new(args: &[String], stdout: String, stderr: String, message: String) -> Self {
        Self {
            command: format!("launchctl {}", shell_words::join(args)),
            stdout,
            stderr,
            message,
        }
    }
}
impl fmt::Display for CommandFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.command)?;
        for output in [&self.stderr, &self.stdout] {
            if !output.trim().is_empty() {
                writeln!(f, "{}", output.trim_end())?;
            }
        }
        f.write_str(&self.message)
    }
}
impl std::error::Error for CommandFailure {}

#[derive(Debug)]
pub struct ErrorReport {
    pub message: String,
    pub command: Option<String>,
    pub stdout: String,
    pub stderr: String,
}
impl From<String> for ErrorReport {
    fn from(message: String) -> Self {
        Self {
            message,
            command: None,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}
impl From<&str> for ErrorReport {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl ErrorReport {
    pub fn from_error(error: anyhow::Error) -> Self {
        let mut context = Vec::new();
        for cause in error.chain() {
            if let Some(failure) = cause.downcast_ref::<CommandFailure>() {
                context.push(failure.message.clone());
                return Self {
                    message: context.join("\n\n"),
                    command: Some(failure.command.clone()),
                    stdout: failure.stdout.clone(),
                    stderr: failure.stderr.clone(),
                };
            }
            context.push(cause.to_string());
        }
        format!("{error:#}").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_and_subprocess_output_remain_distinct() {
        let failure = CommandFailure::new(
            &[
                "bootstrap".into(),
                "gui/501".into(),
                "/tmp/a job.plist".into(),
            ],
            "stdout: first\nsecond\n".into(),
            "stderr: launchctl example\nLTUI\n".into(),
            "Try enabling the job.".into(),
        );
        let error = anyhow::Error::new(failure).context("Job was unloaded, but loading failed.");
        let report = ErrorReport::from_error(error);
        assert_eq!(
            report.command.as_deref(),
            Some("launchctl bootstrap gui/501 '/tmp/a job.plist'")
        );
        assert_eq!(report.stdout, "stdout: first\nsecond\n");
        assert_eq!(report.stderr, "stderr: launchctl example\nLTUI\n");
        assert_eq!(
            report.message,
            "Job was unloaded, but loading failed.\n\nTry enabling the job."
        );
        let report =
            ErrorReport::from_error(anyhow::anyhow!("Invalid plist").context("Saving job"));
        assert!(report.command.is_none());
        assert_eq!(report.message, "Saving job: Invalid plist");
    }
}
