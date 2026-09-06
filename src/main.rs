use anyhow::Result;
use clap::Parser;

use hbox::{
    cli::Cli,
    CommandOutcome,
};

#[tokio::main]
async fn main() -> Result<CommandOutcome> {
    hbox::run(Cli::parse()).await
}
