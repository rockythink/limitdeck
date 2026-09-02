mod adapter;
mod adapters;
mod app;
mod domain;
mod history;
mod theme;
mod ui;

use std::{
    env,
    io::{self, stdout, Stdout},
    process::ExitCode,
    time::{Duration, Instant},
};

use app::{App, PlanEvent, PlanWorker};
use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use history::UsageHistory;
use ratatui::{backend::CrosstermBackend, Terminal};

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HELP: &str = concat!(
    "LimitDeck ",
    env!("CARGO_PKG_VERSION"),
    "\nA privacy-safe terminal dashboard for AI subscription limits.\n\n",
    "Usage:\n",
    "  limitdeck\n",
    "  limitdeck ingest claude\n",
    "  limitdeck --help\n",
    "  limitdeck --version\n\n",
    "Options:\n",
    "  -h, --help       Print help\n",
    "  -V, --version    Print version\n\n",
    "Commands:\n",
    "  ingest claude    Store a quota-only snapshot from Claude Code status-line JSON"
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Dashboard,
    IngestClaude,
    Help,
    Version,
}

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match parse_command(&arguments) {
        Ok(Command::Dashboard) => match run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("LimitDeck 无法启动：{error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::IngestClaude) => {
            let line = adapters::ingest_claude_statusline()
                .unwrap_or_else(|_| "Claude quota unavailable".to_owned());
            println!("{line}");
            ExitCode::SUCCESS
        }
        Ok(Command::Help) => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("limitdeck {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Err(()) => {
            eprintln!("Invalid arguments.\n\n{HELP}");
            ExitCode::FAILURE
        }
    }
}

fn parse_command(arguments: &[String]) -> Result<Command, ()> {
    match arguments {
        [] => Ok(Command::Dashboard),
        [argument] if argument == "-h" || argument == "--help" => Ok(Command::Help),
        [argument] if argument == "-V" || argument == "--version" => Ok(Command::Version),
        [ingest, claude] if ingest == "ingest" && claude == "claude" => Ok(Command::IngestClaude),
        _ => Err(()),
    }
}

fn run() -> io::Result<()> {
    let mut terminal = TerminalSession::enter()?;
    let result = run_app(&mut terminal.terminal);
    result.and(terminal.restore())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let adapters = adapters::discover();
    let identities = adapters
        .iter()
        .map(|adapter| adapter.identity())
        .collect::<Vec<_>>();
    let worker = PlanWorker::spawn(adapters);
    let mut app = App::new(identities);
    let mut history = UsageHistory::load_default();
    let mut next_refresh = Instant::now();
    let mut redraw = true;

    loop {
        while let Ok(Some(event)) = worker.try_recv() {
            if let PlanEvent::Fetched {
                result: Ok(plan), ..
            } = &event
            {
                let _ = history.record(plan);
            }
            app.apply_event(event);
            redraw = true;
        }

        if Instant::now() >= next_refresh && !app.worker_disconnected() {
            request_refresh(&worker, &mut app);
            next_refresh = Instant::now() + REFRESH_INTERVAL;
            redraw = true;
        }

        if redraw {
            terminal.draw(|frame| ui::render(frame, &app, &history))?;
            redraw = false;
        }

        if !event::poll(EVENT_POLL_INTERVAL)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            redraw = true;
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Esc => {
                if app.close_detail() {
                    redraw = true;
                } else {
                    return Ok(());
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !app.is_detail_open() {
                    app.select_previous();
                    redraw = true;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !app.is_detail_open() {
                    app.select_next();
                    redraw = true;
                }
            }
            KeyCode::Enter => {
                app.toggle_detail();
                redraw = true;
            }
            KeyCode::Char('r') if !app.worker_disconnected() => {
                request_refresh(&worker, &mut app);
                next_refresh = Instant::now() + REFRESH_INTERVAL;
                redraw = true;
            }
            _ => {}
        }
    }
}

fn request_refresh(worker: &PlanWorker, app: &mut App) {
    match worker.request_refresh() {
        Ok(true) => app.start_refresh(),
        Ok(false) => {}
        Err(_) => app.mark_worker_disconnected(),
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;

        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        let terminal = match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(stdout(), LeaveAlternateScreen, Show);
                return Err(error);
            }
        };
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }

        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show)?;
        self.restored = true;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if !self.restored {
            let _ = disable_raw_mode();
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_standard_commands() {
        assert_eq!(parse_command(&[]), Ok(Command::Dashboard));
        assert_eq!(parse_command(&arguments(&["--help"])), Ok(Command::Help));
        assert_eq!(parse_command(&arguments(&["-h"])), Ok(Command::Help));
        assert_eq!(
            parse_command(&arguments(&["--version"])),
            Ok(Command::Version)
        );
        assert_eq!(parse_command(&arguments(&["-V"])), Ok(Command::Version));
        assert_eq!(
            parse_command(&arguments(&["ingest", "claude"])),
            Ok(Command::IngestClaude)
        );
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert_eq!(parse_command(&arguments(&["--unknown"])), Err(()));
        assert_eq!(parse_command(&arguments(&["ingest"])), Err(()));
    }
}
