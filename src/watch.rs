use anyhow::{Context, Result};

use notify_debouncer_full::{
    new_debouncer,
    notify::{
        event::{CreateKind, ModifyKind, RemoveKind},
        EventKind, RecursiveMode,
    },
};

use std::{
    time::Duration,
};

use crate::config::ResolvedSite;
use crate::commands::build::build_site;
use crate::commands::validate::{validate_site};

pub async fn watch_and_rebuild(site: ResolvedSite) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    //mpsc::channel::<DebounceEventResult>();

    let mut debouncer = new_debouncer(
        Duration::from_millis(300),
        None,
        move |result| {
            let _ = tx.send(result);
        },
    )
    .context("failed to create filesystem watcher")?;

    debouncer
        .watch(&site.source_dir(), RecursiveMode::Recursive)
        .with_context(|| {
            format!("failed to watch {}", &site.source_dir().display())
        })?;

    println!("Watching: {}", site);

    while let Some(result) = rx.recv().await {
        match result {
            Ok(events) => {
                let has_write_event = events
                    .iter()
                    .any(|event| is_write_event(&event.event.kind));

                if !has_write_event {
                    continue;
                }

                match build_site(&site) {
                    Err(error) => {
                        eprintln!("Rebuild failed:\n{error:?}");
                    }

                    Ok(build_report) => {
                        match validate_site(&build_report.output_dir, false).await {
                            Ok(report) if report.is_valid() => {
                                dev_print!("Rebuild complete.");
                            }

                            Ok(report) => {
                                eprintln!(
                                    "Rebuild completed, but validation found {} errors.",
                                    report.errors,
                                );
                            }

                            Err(error) => {
                                eprintln!("Validation failed:\n{error:?}");
                            }
                        }
                    }
                }
            }

            Err(errors) => {
                for error in errors {
                    eprintln!("Watch error: {error:?}");
                }
            }
        }
    }

    Ok(())
}

fn is_write_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::Any)
            | EventKind::Create(CreateKind::File)
            | EventKind::Create(CreateKind::Folder)
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Metadata(_))
            | EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Remove(RemoveKind::Any)
            | EventKind::Remove(RemoveKind::File)
            | EventKind::Remove(RemoveKind::Folder)
    )
}
