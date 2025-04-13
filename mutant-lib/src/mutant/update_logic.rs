use super::MutAnt;
use crate::error::Error;
use crate::events::{invoke_callback, PutCallback, PutEvent};
use log::{debug, error, info, trace, warn};

/// Updates an existing item with new data.
///
/// If the key does not exist, it returns a `KeyNotFound` error.
pub(super) async fn update_item(
    es: &MutAnt,
    key: &str,
    data_bytes: &[u8],
    mut callback: Option<PutCallback>,
) -> Result<(), Error> {
    let data_size = data_bytes.len();
    trace!(
        "MutAnt [{}]: Starting update_item for key '{}' ({} bytes) using PadManager",
        es.master_index_addr, // Assuming this is still relevant for logging
        key,
        data_size
    );

    {
        let mis_guard = es.master_index_storage.lock().await;
        if !mis_guard.index.contains_key(key) {
            debug!("UpdateItem[{}]: Key does not exist.", key);
            return Err(Error::KeyNotFound(key.to_string()));
        }
    }

    let mut needs_confirmation = false;
    let mut estimated_new_pads_needed = 0;

    match es.pad_manager.estimate_reservation(data_size).await {
        Ok(Some(pads)) => {
            if pads > 0 {
                needs_confirmation = true;
                estimated_new_pads_needed = pads;
                debug!(
                    "UpdateItem[{}]: Estimate indicates {} new pads needed. Confirmation required.",
                    key, pads
                );
                invoke_callback(
                    &mut callback,
                    PutEvent::ReservingScratchpads {
                        needed: pads as u64,
                    },
                )
                .await?;
            } else {
                warn!(
                    "UpdateItem[{}]: estimate_reservation returned Some(0), which is unexpected. Proceeding.",
                    key
                );
            }
        }
        Ok(None) => {
            debug!(
                "UpdateItem[{}]: Estimate indicates enough free/existing pads available. No confirmation needed.",
                key
            );
        }
        Err(e) => {
            error!(
                "UpdateItem[{}]: Failed to estimate reservation needs: {}",
                key, e
            );
            return Err(e);
        }
    }

    if needs_confirmation {
        let (total_space_bytes, free_space_bytes, current_scratchpads, _scratchpad_size) = {
            let mis_guard = es.master_index_storage.lock().await;
            let total_pads = mis_guard
                .index
                .values()
                .map(|v| v.pads.len())
                .sum::<usize>()
                + mis_guard.free_pads.len();
            let pad_size = mis_guard.scratchpad_size;
            let total_space = total_pads * pad_size;
            let free_space = mis_guard.free_pads.len() * pad_size;
            (total_space, free_space, total_pads, pad_size)
        };

        debug!(
            "UpdateItem[{}]: Invoking ConfirmReservation callback...",
            key
        );
        invoke_callback(
            &mut callback,
            PutEvent::ConfirmReservation {
                needed: estimated_new_pads_needed as u64,
                data_size: data_size as u64,
                total_space: total_space_bytes as u64,
                free_space: free_space_bytes as u64,
                current_scratchpads,
                estimated_cost: None,
            },
        )
        .await?;
        info!(
            "UpdateItem[{}]: Reservation confirmation received from user.",
            key
        );
    } else {
        info!(
            "UpdateItem[{}]: Reservation confirmation not required.",
            key
        );
    }

    debug!(
        "UpdateItem[{}]: Calling pad_manager.allocate_and_write...",
        key
    );

    es.pad_manager
        .allocate_and_write(key, data_bytes, callback)
        .await?;

    info!(
        "UpdateItem[{}]: PadManager allocate_and_write completed successfully for update.",
        key
    );

    info!("UpdateItem[{}]: Update operation fully completed.", key);
    Ok(())
}
