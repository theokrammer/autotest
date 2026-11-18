use clap::Parser;
use autotest_cli::{run_cli, Cli};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    run_cli(cli)
}
