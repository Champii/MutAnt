use log::{debug, error, info};
// Use new top-level re-exports
use mutant_lib::{Error as LibError, MutAnt};
use std::process::ExitCode;

/// Executes the core logic for resetting the master index.
/// Confirmation should be handled by the caller.
pub async fn handle_reset(mutant: MutAnt) -> Result<ExitCode, LibError> {
    // Update return type
    debug!("CLI: Executing core Reset logic...");
    match mutant.reset_master_index().await {
        Ok(_) => {
            info!("Master index reset successfully via MutAnt library call.");
            println!("Master index has been reset.");
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            error!(
                "Failed to reset master index via MutAnt library call: {}",
                e
            );
            eprintln!("Error resetting master index: {}", e);
            // Propagate the library error
            Err(e) // Return the specific LibError
        }
    }
}
