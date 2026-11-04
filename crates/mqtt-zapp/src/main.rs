mod cli;
mod headless;
mod tui;

use clap::Parser;
use cli::{Args, Cmd};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match &args.cmd {
        Some(Cmd::Profile(cmd)) => cli::run_profile(cmd),
        Some(Cmd::Sub(cmd)) => std::process::exit(headless::sub(cmd).await),
        Some(Cmd::Pub(cmd)) => std::process::exit(headless::publish(cmd).await),
        Some(Cmd::Tui) | None => {
            let (store, store_error) =
                cli::store_or_empty(&args, mqtt_zapp_core::config::ProfileStore::load())?;
            if let Some(w) = cli::insecure_warning(&args) {
                eprintln!("{w}");
            }
            let launch = cli::resolve_launch(&args, &store)?;
            tui::run(launch, store, store_error).await
        }
    }
}
