use eyre::Result;
use octos_tui::{cli::Cli, cmd, event_loop};

fn main() -> Result<()> {
    color_eyre::install()?;

    // Intercept `update`/`doctor` subcommands before the normal TUI launch.
    // A leading `update`/`doctor` positional dispatches to the command modules
    // and exits with their code; anything else falls through to the TUI.
    if let Some(code) = cmd::dispatch(std::env::args())? {
        std::process::exit(code);
    }

    let cli = Cli::parse()?;
    event_loop::run(cli)
}
