//! Core application library for Hbox.
//!
//! The binary is responsible only for parsing command-line arguments and
//! returning the outcome produced here.

#![forbid(unsafe_code)]

use std::process::{ExitCode, Termination};

use anyhow::Result;

pub mod cli;

mod ai;
mod assets;
mod commands;
mod config;
mod models;
mod optimization;
mod rendering;
mod templates;
mod utils;
mod watch;
mod publish;
mod previews;

pub use commands::validate::ValidationReport;

/// Prints development-only diagnostics without exporting a public macro.
///
/// This is a transitional replacement for the current `#[macro_export]` macro
/// in `utils.rs`. Structured logging would be the natural next step if Hbox
/// eventually needs configurable verbosity.
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

pub(crate) use dev_print;

/// The semantic result of executing an Hbox command.
///
/// A validation report is not an application error. The validation command can
/// nevertheless translate an invalid report into a non-zero process status
/// when it is invoked explicitly.
#[derive(Debug)]
#[must_use = "command outcomes determine the process exit status"]
pub enum CommandOutcome {
    /// The command completed successfully.
    Success,

    /// The explicit `validate` command completed and produced a report.
    Validation(ValidationReport),
}

impl CommandOutcome {
    /// Returns whether the command should be considered successful by the
    /// invoking process.
    pub fn is_success(&self) -> bool {
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

/// Executes one parsed Hbox command.
///
/// Parsing belongs to the binary-facing CLI module. Command orchestration lives
/// here, while the individual command modules own their domain workflows.
pub async fn run(cli: cli::Cli) -> Result<CommandOutcome> {
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
