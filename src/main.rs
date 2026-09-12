#![forbid(unsafe_code)]

use std::process::{ExitCode, Termination};

use anyhow::Result;
use clap::Parser;

macro_rules! dev_print {
    ($format:literal $(, $value:expr)* $(,)?) => {
        #[cfg(debug_assertions)]
        {
            eprintln!($format $(, $value)*);
        }
    };
    ($value:expr $(,)?) => {
        #[cfg(debug_assertions)]
        {
            eprintln!("{}", $value);
        }
    };
}

mod ai;
mod assets;
mod cli;
mod commands;
mod config;
mod models;
mod optimization;
mod previews;
mod publish;
mod rendering;
mod templates;
mod utils;
mod watch;

use cli::Cli;
use commands::validate::ValidationReport;

#[must_use = "command outcomes determine the process exit status"]
enum CommandOutcome {
    Success,
    Validation(ValidationReport),
}

impl CommandOutcome {
    fn is_success(&self) -> bool {
        match self {
            Self::Success => true,
            Self::Validation(report) => report.is_valid(),
        }
    }
}

impl Termination for CommandOutcome {
    fn report(self) -> ExitCode {
        if self.is_success() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<CommandOutcome> {
    match cli.command {
        cli::Command::Init(args) => {
            commands::init::run(args)?;
        }

        cli::Command::Build(args) => {
            commands::build::run(args).await?;
        }

        cli::Command::ImportDesign(args) => {
            commands::import::run(args).await?;
        }

        cli::Command::Update(args) => {
            commands::update::run(args).await?;
        }

        cli::Command::AcceptPreview(args) => {
            commands::accept::run(args).await?;
        }

        cli::Command::Optimize(args) => {
            commands::optimize::run(args).await?;
        }

        cli::Command::Preview(args) => {
            commands::preview::run(args).await?;
        }

        cli::Command::Deploy(args) => {
            commands::deploy::run(args).await?;
        }

        cli::Command::Validate(args) => {
            let report = commands::validate::run(args).await?;
            return Ok(CommandOutcome::Validation(report));
        }
    }

    Ok(CommandOutcome::Success)
}

#[tokio::main]
async fn main() -> Result<CommandOutcome> {
    run(Cli::parse()).await
}
