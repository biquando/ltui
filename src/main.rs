mod app;
mod launchd;
mod model;
mod ui;

use anyhow::{Context, Result, bail, ensure};
use app::{App, Effect, Modal};
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
};
use model::Scope;
use std::{io::IsTerminal, path::PathBuf, time::Duration};

const HELP: &str = "ltui — a launchd job manager for macOS

Usage: ltui [OPTIONS]

  --scope SCOPE   user (default), agents, daemons, apple-agents,
                  apple-daemons, session, system
  --domain DOMAIN Agent target: gui/UID (default: your UID) or user/UID
                  Daemons always use the system domain
  --list          Print a read-only job inventory and exit
  --demo          Explore sample jobs without changing real files or services
  -h, --help      Show this help
  -V, --version   Show version

Run ltui, then press ? for keyboard shortcuts.
Full plist editing uses $EDITOR, then $VISUAL, then vi.
Use a waiting option for GUI editors, e.g. EDITOR='code --wait'.
For headless sessions use --domain user/UID.
System changes require root; run sudo ltui --scope daemons.
";

fn main() -> Result<()> {
    let mut scope = Scope::User;
    let mut demo = false;
    let mut list = false;
    #[cfg(unix)]
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0;
    // sudo should still browse the invoking user's GUI domain and home.
    let uid = std::env::var("SUDO_UID")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(uid);
    let mut domain = format!("gui/{uid}");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("ltui {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--demo" => demo = true,
            "--list" => list = true,
            "--scope" => scope = Scope::parse(&args.next().context("--scope requires a value")?)?,
            "--domain" => {
                domain = args.next().context("--domain requires a value")?;
                let parts: Vec<_> = domain.split('/').collect();
                ensure!(
                    parts.len() == 2
                        && matches!(parts[0], "gui" | "user")
                        && parts[1].parse::<u32>().is_ok(),
                    "--domain must be gui/UID or user/UID"
                );
            }
            _ => bail!("Unknown option: {arg}\nRun ltui --help for usage"),
        }
    }
    ensure!(
        cfg!(target_os = "macos") || demo,
        "ltui requires macOS. Use --demo to preview the interface elsewhere"
    );
    let home = home_for_uid(uid)?;
    let mut app = App::new(home, domain, scope, demo);
    if list {
        println!("{:<55} {:<19} {:<10} PLIST", "LABEL", "RUNTIME", "POLICY");
        for job in &app.jobs {
            println!(
                "{:<55} {:<19} {:<10} {}{}",
                job.label,
                job.state.text(),
                job.enabled_text(),
                job.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                job.error
                    .as_ref()
                    .map(|e| format!(" [ERROR: {e}]"))
                    .unwrap_or_default()
            );
        }
        for warning in &app.warnings {
            eprintln!("Warning: {warning}");
        }
        return Ok(());
    }
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "An interactive terminal is required. Use --list for a text inventory"
    );
    let mut terminal = init_terminal()?;
    let result = run(&mut terminal, &mut app);
    restore_terminal();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => match app.handle(key) {
                    Effect::Quit => break,
                    Effect::None => {}
                    Effect::Editor => match app.prepare_editor() {
                        Err(e) => app.error(format!("{e:#}")),
                        Ok(session) => {
                            restore_terminal();
                            let edited = session.run();
                            *terminal = init_terminal()?;
                            match edited {
                                Ok(()) => app.finish_editor(session),
                                Err(e) => {
                                    let recovery = session
                                        .temp
                                        .keep()
                                        .map(|(_, p)| p.display().to_string())
                                        .unwrap_or_else(|e| e.to_string());
                                    app.error(format!("{e:#}\n\nOriginal file unchanged. Temporary copy: {recovery}"));
                                }
                            }
                        }
                    },
                },
                Event::Paste(contents) => {
                    if let Some(Modal::Form(form)) = &mut app.modal {
                        form.paste(&contents);
                    } else if app.searching {
                        app.filter.push_str(&contents.replace(['\n', '\r'], " "));
                        app.refilter();
                        app.load_detail();
                    }
                }
                _ => {}
            }
        }
        if app.modal.is_none()
            && !app.searching
            && app.refreshed.elapsed() >= Duration::from_secs(5)
        {
            app.refresh();
        }
    }
    Ok(())
}

fn init_terminal() -> Result<ratatui::DefaultTerminal> {
    let terminal = ratatui::try_init()?;
    if let Err(e) = execute!(
        std::io::stdout(),
        EnableBracketedPaste,
        SetCursorStyle::BlinkingBar
    ) {
        restore_terminal();
        return Err(e.into());
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            SetCursorStyle::DefaultUserShape
        );
        previous(info);
    }));
    Ok(terminal)
}
fn restore_terminal() {
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        SetCursorStyle::DefaultUserShape
    );
    ratatui::restore();
}

fn home_for_uid(uid: u32) -> Result<PathBuf> {
    #[cfg(unix)]
    if std::env::var_os("SUDO_UID").is_some() {
        // getpwuid's storage is copied before any further libc call.
        let entry = unsafe { libc::getpwuid(uid) };
        if !entry.is_null() {
            let home = unsafe { std::ffi::CStr::from_ptr((*entry).pw_dir) };
            use std::os::unix::ffi::OsStrExt;
            return Ok(PathBuf::from(std::ffi::OsStr::from_bytes(home.to_bytes())));
        }
    }
    Ok(PathBuf::from(
        std::env::var_os("HOME").context("HOME is not set")?,
    ))
}
