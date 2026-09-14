mod agent;
mod app;
mod config;
mod integration;
mod penguin;
mod providers;
mod safety;
mod session;
mod setup_tui;
mod skills;
mod term;
mod theme;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pc", version, about = "penguin command — a terminal with a built-in AI agent")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Shell to run (defaults to $SHELL)
    #[arg(long, global = true)]
    shell: Option<String>,

    /// Workspace directory for the agent (default: current dir)
    #[arg(short, long, global = true)]
    workspace: Option<PathBuf>,

    /// Start in agent mode with this first prompt
    #[arg(short = 'p', long, global = true)]
    prompt: Option<String>,

    /// Resume the most recent agent session
    #[arg(short = 'c', long = "continue", global = true)]
    cont: bool,

    /// YOLO mode: auto-approve every tool action (catastrophic hard-blocks
    /// and sudo password prompts still apply)
    #[arg(long, global = true)]
    yolo: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Configure providers (TUI wizard)
    Setup,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::Setup) => {
            let cfg = config::Config::load().unwrap_or_default();
            setup_tui::run_setup(&cfg)?;
            println!("saved {}", config::Config::path().display());
        }
        None => app::run(app::Options {
            shell: cli.shell,
            workspace: cli.workspace,
            prompt: cli.prompt,
            cont_last: cli.cont,
            yolo: cli.yolo,
        })?,
    }
    Ok(())
}
