mod cli;
mod tui;

use clap::Parser;
use cli::{Args, Cmd};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match &args.cmd {
        Some(Cmd::Profile(cmd)) => cli::run_profile(cmd),
        Some(Cmd::Tui) | None => {
            let store = mqtt_zapp_core::config::ProfileStore::load()?;
            tui::run(cli::resolve_launch(&args, &store)?).await
        }
    }
}
