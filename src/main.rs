mod adapter;
mod adapters;
mod app;
mod domain;
mod theme;
mod ui;

use std::{
    env,
    io::{self, stdout, Stdout},
    process::ExitCode,
    time::{Duration, Instant},
};

use app::{App, PlanWorker};
use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments == ["ingest", "claude"] {
        let line = adapters::ingest_claude_statusline()
            .unwrap_or_else(|_| "Claude quota unavailable".to_owned());
        println!("{line}");
        return ExitCode::SUCCESS;
    }
    if !arguments.is_empty() {
        eprintln!("用法：limitdeck [ingest claude]");
        return ExitCode::FAILURE;
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("LimitDeck 无法启动：{error}");
            ExitCode::FAILURE
        }
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
    let mut next_refresh = Instant::now();
    let mut redraw = true;

    loop {
        while let Ok(Some(event)) = worker.try_recv() {
            app.apply_event(event);
            redraw = true;
        }

        if Instant::now() >= next_refresh && !app.worker_disconnected() {
            request_refresh(&worker, &mut app);
            next_refresh = Instant::now() + REFRESH_INTERVAL;
            redraw = true;
        }

        if redraw {
            terminal.draw(|frame| ui::render(frame, &app))?;
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
