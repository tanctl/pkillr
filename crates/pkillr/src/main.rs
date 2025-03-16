mod app;
mod config;
mod process;
mod signals;
mod ui;

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::builder::styling::{Style, Styles};
use clap::{ColorChoice, CommandFactory, FromArgMatches, Parser};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use app::App;
use config::{Config, SortField, Theme};
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Parser)]
#[command(name = "pkillr", about = "Interactive TUI process killer", version)]
pub struct Cli {
    /// optional process name filter applied on startup.
    #[arg(value_name = "FILTER")]
    pub filter: Option<String>,

    /// show system processes in addition to user processes.
    #[arg(short = 'a', long = "all")]
    pub all: bool,

    /// default column used to sort the process table.
    #[arg(long = "sort-by", value_enum, default_value_t = SortField::Cpu)]
    pub sort_by: SortField,

    /// theme selection for the tui.
    #[arg(long = "theme", value_enum, default_value_t = Theme::Pink)]
    pub theme: Theme,

    /// refresh interval in milliseconds.
    #[arg(long = "refresh-rate", value_name = "ms", default_value_t = 800)]
    pub refresh_rate: u64,
}

fn main() -> Result<()> {
    let matches = Cli::command()
        .color(ColorChoice::Always)
        .styles(clap_styles())
        .get_matches();
    let args = Cli::from_arg_matches(&matches).expect("cli parse failure");
    let config = Config {
        theme: args.theme,
        show_all_processes: args.all,
        refresh_rate_ms: args.refresh_rate,
        initial_filter: args.filter.clone(),
        initial_sort: args.sort_by,
        sort_descending: true,
    };

    let mut app = App::new(config);
    let mut terminal = setup_terminal().context("failed to initialize terminal")?;
    let _guard = TerminalGuard::new();

    ctrlc::set_handler(|| {
        cleanup_terminal();
        std::process::exit(0);
    })
    .context("failed to install ctrl+c handler")?;

    run_app(&mut terminal, &mut app)?;
    Ok(())
}

fn clap_styles() -> Styles {
    const HOT_PINK: (u8, u8, u8) = (255, 20, 147);

    let style = Style::new().fg_color(Some(HOT_PINK.into()));

    Styles::styled()
        .header(style.bold())
        .usage(style.bold())
        .literal(style.bold())
        .placeholder(style)
        .valid(style.bold())
        .invalid(style.bold())
        .context(style)
        .context_value(style)
        .error(style.bold())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("failed to create terminal")
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    terminal.hide_cursor()?;
    let mut refresh_timer = Instant::now();
    let refresh_interval = Duration::from_millis(app.refresh_rate_ms());

    loop {
        app.tick(Instant::now());
        if app.needs_refresh() {
            terminal.draw(|frame| ui::render(frame, app))?;
            app.clear_refresh_flag();
        }

        if event::poll(INPUT_POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                    {
                        break;
                    }
                    if app.handle_input(key)? {
                        break;
                    }
                }
                Event::Resize(_, _) => app.request_redraw(),
                _ => {}
            }
        }

        if !app.is_paused() && refresh_timer.elapsed() >= refresh_interval {
            app.update_processes();
            refresh_timer = Instant::now();
        }
    }

    terminal.show_cursor()?;
    Ok(())
}

fn cleanup_terminal() {
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, LeaveAlternateScreen, Show);
}

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> Self {
        TerminalGuard
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        cleanup_terminal();
    }
}
