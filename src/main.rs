mod app;
mod browser;
mod config;
mod clip;
mod free;
mod inspect;
mod inspector;
mod kill;
mod model;
mod palette;
mod scanner;
mod services;
mod theme;
mod ui;
mod verdict;

use std::fmt::Write as _;
use std::io::{self, IsTerminal, Write as _};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
use ratatui::crossterm::execute;

use app::App;

/// How often we check the keyboard (keeps keys responsive between scans).
const POLL_EVERY: Duration = Duration::from_millis(100);

const HELP: &str = "\
portwatch: monitor and manage the network ports on this machine

USAGE:
    portwatch                open the interactive dashboard
    portwatch list           print listening ports and exit (no dashboard)
    portwatch list --all     include every connection, with its state
    portwatch free <port>    find what uses a port, stop it (asks first),
                             and check that the port is really free
    portwatch inspect <port> show everything about the program using a port
                             (add --json for JSON you can pipe anywhere)
    portwatch config         show where the optional settings file lives
    portwatch --help         show this help
    portwatch --version      show the version

Inside the dashboard press ? for the key list.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => dashboard(),
        Some("list") => list(&args[1..]),
        Some("free") => free::run(&args[1..]),
        Some("inspect") => inspector::run_cli(&args[1..]),
        Some("config") => show_config(),
        Some("-h" | "--help" | "help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("portwatch {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("portwatch: unknown argument '{other}'\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn dashboard() -> ExitCode {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        eprintln!("portwatch: the dashboard needs an interactive terminal. Use `portwatch list` instead.");
        return ExitCode::FAILURE;
    }

    let loaded = config::load();

    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(err) => {
            eprintln!("portwatch: could not start the terminal UI: {err}");
            return ExitCode::FAILURE;
        }
    };
    // Mouse capture changes how the terminal behaves, so it must be switched
    // off again on every way out, including a crash.
    let mouse = loaded.config.mouse;
    if mouse {
        let _ = execute!(io::stdout(), EnableMouseCapture);
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(io::stdout(), DisableMouseCapture);
            previous_hook(info);
        }));
    }
    let result = run(&mut terminal, &loaded);
    // Always give the terminal back, even if the loop failed.
    if mouse {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
    let restored = ratatui::try_restore();

    if let Err(err) = result.and(restored) {
        eprintln!("portwatch: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// The account "mine" means: the person at the keyboard, even under sudo.
fn current_user(is_root: bool) -> String {
    if is_root {
        if let Some(name) = std::env::var("SUDO_USER").ok().filter(|n| !n.is_empty()) {
            return name;
        }
    }
    nix::unistd::User::from_uid(nix::unistd::geteuid())
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_default()
}

fn run(terminal: &mut DefaultTerminal, loaded: &config::Loaded) -> io::Result<()> {
    let is_root = nix::unistd::geteuid().is_root();
    // As root nothing is ever written (a root-owned file in your home would
    // get in the way later), so the tour is only offered to normal users.
    let tour_dir = if is_root { None } else { config::state_dir() };
    let mut app = App::new(is_root, current_user(is_root))
        .with_look(loaded.config.look())
        .with_tour_dir(tour_dir);
    let scan_every = loaded.config.refresh();
    app.rescan();
    match &loaded.warning {
        Some(warning) => app.set_status(warning.clone(), true),
        None if app.tour_due() => app.start_tour(),
        None => app.welcome(),
    }
    let mut last_scan = Instant::now();

    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        if event::poll(POLL_EVERY)? {
            // Take everything that is waiting before drawing again. Mouse
            // capture reports every movement of the pointer, and drawing once
            // per movement would make keypresses wait behind a queue of them.
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                    Event::Mouse(mouse) if !matches!(mouse.kind, MouseEventKind::Moved) => {
                        app.handle_mouse(mouse)
                    }
                    _ => {}
                }
                if app.should_quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }

        // Answers from background work, and a clipboard escape code if no
        // clipboard program was found.
        app.tick();
        if let Some(sequence) = app.take_osc52() {
            let mut out = io::stdout();
            let _ = out.write_all(sequence.as_bytes());
            let _ = out.flush();
        }

        let due = last_scan.elapsed() >= scan_every;
        if app.rescan_now || (due && !app.paused) {
            app.rescan();
            app.rescan_now = false;
            last_scan = Instant::now();
        }
    }
    Ok(())
}

/// `portwatch config`: say where the optional settings file goes and what
/// it can contain. portwatch only reads it; it never creates the file.
fn show_config() -> ExitCode {
    let loaded = config::load();
    match &loaded.path {
        Some(path) => {
            println!("Settings file: {}", path.display());
            println!(
                "{}",
                if path.exists() { "(found)" } else { "(not created yet; defaults are used)" }
            );
        }
        None => println!("Settings file: unknown (HOME is not set)"),
    }
    if let Some(warning) = &loaded.warning {
        println!("Problem: {warning}");
    }
    println!("\nCreate it with these lines (all optional):\n\n{}", config::SAMPLE);
    ExitCode::SUCCESS
}

fn list(args: &[String]) -> ExitCode {
    let mut show_all = false;
    for arg in args {
        match arg.as_str() {
            "--all" | "-a" => show_all = true,
            other => {
                eprintln!("portwatch list: unknown option '{other}'\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }

    let entries = match scanner::scan() {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("portwatch: could not read /proc: {err}");
            return ExitCode::FAILURE;
        }
    };
    let shown: Vec<_> = entries
        .iter()
        .filter(|e| show_all || e.listening)
        .collect();

    // Build the whole output first and write it once, so a closed pipe
    // (`portwatch list | head`) ends quietly instead of panicking.
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<5} {:>5}  {:<39} {:<11} {:>7}  {:<16} USER",
        "PROTO", "PORT", "ADDRESS", "STATE", "PID", "PROCESS"
    );
    for e in &shown {
        let (pid, name) = match &e.process {
            Some(p) => (p.pid.to_string(), p.name.as_str()),
            None => ("-".to_string(), "-"),
        };
        let _ = writeln!(
            out,
            "{:<5} {:>5}  {:<39} {:<11} {:>7}  {:<16} {}",
            e.protocol,
            e.local.port(),
            e.local.ip(),
            e.state,
            pid,
            name,
            e.user
        );
    }

    let listening = shown.iter().filter(|e| e.listening).count();
    let _ = writeln!(out, "\n{listening} listening");
    let hidden = shown
        .iter()
        .filter(|e| e.listening && e.process.is_none())
        .count();
    if hidden > 0 {
        let _ = writeln!(
            out,
            "{hidden} without process info (owned by other users; run `sudo \"$(which portwatch)\" list` to see them)"
        );
    }
    let _ = io::stdout().write_all(out.as_bytes());
    ExitCode::SUCCESS
}
