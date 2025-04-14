use crate::callbacks::StyledProgressBar;
use crate::callbacks::put::create_put_callback;
use indicatif::MultiProgress;
use log::{debug, warn};
use mutant_lib::error::Error;
use mutant_lib::mutant::MutAnt;
use std::io::{self, Read};
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn handle_put(
    mutant: MutAnt,
    key: String,
    value: Option<String>,
    force: bool,
    multi_progress: &MultiProgress,
    quiet: bool,
) -> ExitCode {
    debug!(
        "CLI: Handling Put command: key={}, value_is_some={}, force={}",
        key,
        value.is_some(),
        force
    );

    // Get data as Vec<u8>
    let data_vec: Vec<u8> = match value {
        Some(v) => {
            debug!("handle_put: Using value from argument");
            v.into_bytes()
        }
        None => {
            if !quiet && atty::is(atty::Stream::Stdin) {
                eprintln!("Reading value from stdin... (Ctrl+D to end)");
            }
            debug!("handle_put: Reading value from stdin");
            let mut buffer = Vec::new();
            if let Err(e) = io::stdin().read_to_end(&mut buffer) {
                eprintln!("Error reading value from stdin: {}", e);
                return ExitCode::FAILURE;
            }
            buffer
        }
    };

    // Conditionally create callbacks based on quiet flag
    let (res_pb_opt, upload_pb_opt, confirm_pb_opt, confirm_counter_arc, callback) =
        create_put_callback(multi_progress, quiet);

    // Pass data as slice &[u8]
    let result = if force {
        debug!("Forcing update for key: {}", key);
        mutant
            .update_with_progress(&key, &data_vec, Some(callback))
            .await
    } else {
        mutant
            .store_with_progress(&key, &data_vec, Some(callback), confirm_counter_arc.clone())
            .await
    };

    match result {
        Ok(_) => {
            debug!("Put operation successful for key: {}", key);
            clear_pb(&res_pb_opt);
            clear_pb(&upload_pb_opt);
            clear_pb(&confirm_pb_opt);
            ExitCode::SUCCESS
        }
        Err(e) => {
            let error_message = match e {
                Error::KeyAlreadyExists(ref k) if !force => {
                    format!("Key '{}' already exists. Use --force to overwrite.", k)
                }
                Error::KeyNotFound(ref k) if force => {
                    format!(
                        "Cannot force update non-existent key '{}'. Use put without --force.",
                        k
                    )
                }
                Error::OperationCancelled => "Operation cancelled.".to_string(),
                _ => format!(
                    "Error during {}: {}",
                    if force { "update" } else { "store" },
                    e
                ),
            };

            eprintln!("{}", error_message);
            abandon_pb(&res_pb_opt, error_message.clone());
            abandon_pb(&upload_pb_opt, error_message.clone());
            abandon_pb(&confirm_pb_opt, error_message);

            ExitCode::FAILURE
        }
    }
}

// Helper functions to clear or abandon progress bars - kept local to put.rs
fn clear_pb(pb_opt: &Arc<Mutex<Option<StyledProgressBar>>>) {
    // Use try_lock to avoid blocking if the lock is held (e.g., by the callback thread)
    if let Ok(mut guard) = pb_opt.try_lock() {
        if let Some(pb) = guard.take() {
            if !pb.is_finished() {
                pb.finish_and_clear();
            }
        }
    } else {
        warn!("clear_pb: Could not acquire lock to clear progress bar.");
    }
}

fn abandon_pb(pb_opt: &Arc<Mutex<Option<StyledProgressBar>>>, message: String) {
    if let Ok(mut guard) = pb_opt.try_lock() {
        if let Some(pb) = guard.take() {
            if !pb.is_finished() {
                pb.abandon_with_message(message);
            }
        }
    } else {
        warn!("abandon_pb: Could not acquire lock to abandon progress bar.");
    }
}
