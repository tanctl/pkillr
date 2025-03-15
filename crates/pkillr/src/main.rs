mod app;
mod config;
mod process;
mod signals;
mod ui;

use anyhow::Result;
use clap::builder::styling::{Styles, Style};
use clap::{ColorChoice, CommandFactory, FromArgMatches, Parser};
use config::{Config, SortField, Theme};

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
    println!("{args:#?}");
    println!("{config:#?}");
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
