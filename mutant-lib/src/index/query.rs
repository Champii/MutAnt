use crate::index::error::IndexError;
use crate::index::structure::{KeyInfo, MasterIndex, PadStatus, DEFAULT_SCRATCHPAD_SIZE};
use crate::types::{KeyDetails, StorageStats};
use autonomi::ScratchpadAddress;
use log::{debug, trace, warn};

// --- Internal Query & Modification Functions ---
// These functions operate directly on the MasterIndex state and are
// intended to be called while holding a lock (e.g., MutexGuard).

/// Retrieves information for a specific key.
pub(crate) fn get_key_info_internal<'a>(index: &'a MasterIndex, key: &str) -> Option<&'a KeyInfo> {
    trace!("Query: get_key_info_internal for key '{}'", key);
    index.index.get(key)
}

/// Inserts or updates information for a specific key.
pub(crate) fn insert_key_info_internal(
    index: &mut MasterIndex,
    key: String,
    info: KeyInfo,
) -> Result<(), IndexError> {
    trace!("Query: insert_key_info_internal for key '{}'", key);
    // TODO: Add validation? E.g., ensure pad list isn't empty if size > 0?
    index.index.insert(key, info);
    Ok(())
}

/// Removes information for a specific key, returning the old info if it existed.
pub(crate) fn remove_key_info_internal(index: &mut MasterIndex, key: &str) -> Option<KeyInfo> {
    trace!("Query: remove_key_info_internal for key '{}'", key);
    index.index.remove(key)
}

/// Lists all user keys currently stored in the index.
pub(crate) fn list_keys_internal(index: &MasterIndex) -> Vec<String> {
    trace!("Query: list_keys_internal");
    index.index.keys().cloned().collect()
    // Consider filtering out internal keys if any are added later
}

/// Retrieves detailed information for a specific key.
pub(crate) fn get_key_details_internal(index: &MasterIndex, key: &str) -> Option<KeyDetails> {
    trace!("Query: get_key_details_internal for key '{}'", key);
    index.index.get(key).map(|info| {
        let percentage = if !info.is_complete && !info.pads.is_empty() {
            let confirmed_count = info
                .pads
                .iter()
                .filter(|p| p.status == PadStatus::Confirmed)
                .count();
            Some((confirmed_count as f32 / info.pads.len() as f32) * 100.0)
        } else {
            None
        };
        KeyDetails {
            key: key.to_string(),
            size: info.data_size,
            modified: info.modified,
            is_finished: info.is_complete,
            completion_percentage: percentage,
        }
    })
}

/// Retrieves detailed information for all keys.
pub(crate) fn list_all_key_details_internal(index: &MasterIndex) -> Vec<KeyDetails> {
    trace!("Query: list_all_key_details_internal");
    index
        .index
        .iter()
        .map(|(key, info)| {
            let percentage = if !info.is_complete && !info.pads.is_empty() {
                let confirmed_count = info
                    .pads
                    .iter()
                    .filter(|p| p.status == PadStatus::Confirmed)
                    .count();
                Some((confirmed_count as f32 / info.pads.len() as f32) * 100.0)
            } else {
                None
            };
            KeyDetails {
                key: key.clone(),
                size: info.data_size,
                modified: info.modified,
                is_finished: info.is_complete,
                completion_percentage: percentage,
            }
        })
        .collect()
}

/// Calculates storage statistics based on the current index state.
pub(crate) fn get_stats_internal(index: &MasterIndex) -> Result<StorageStats, IndexError> {
    trace!("Query: get_stats_internal");
    let scratchpad_size = index.scratchpad_size;
    if scratchpad_size == 0 {
        warn!("Cannot calculate stats: Scratchpad size in index is zero.");
        return Err(IndexError::InconsistentState(
            "Scratchpad size in index is zero".to_string(),
        ));
    }

    let free_pads_count = index.free_pads.len();
    let pending_verification_pads_count = index.pending_verification_pads.len();

    let mut occupied_pads_count = 0; // Pads confirmed holding data
    let mut occupied_data_size_total: u64 = 0;
    let mut allocated_written_pads_count = 0; // Pads used by keys but not confirmed

    let mut incomplete_keys_count = 0;
    let mut incomplete_keys_data_bytes = 0;
    let mut incomplete_keys_total_pads = 0;
    let mut incomplete_keys_pads_generated = 0;
    let mut incomplete_keys_pads_allocated = 0; // Added for completeness
    let mut incomplete_keys_pads_written = 0;
    let mut incomplete_keys_pads_confirmed = 0;

    for key_info in index.index.values() {
        if key_info.is_complete {
            // For complete keys, all pads contribute to occupied count and data size
            occupied_pads_count += key_info.pads.len();
            occupied_data_size_total += key_info.data_size as u64;
        } else {
            // For incomplete keys, analyze each pad status
            incomplete_keys_count += 1;
            incomplete_keys_data_bytes += key_info.data_size as u64;
            incomplete_keys_total_pads += key_info.pads.len();

            for pad_info in &key_info.pads {
                match pad_info.status {
                    PadStatus::Generated => incomplete_keys_pads_generated += 1,
                    PadStatus::Allocated => {
                        incomplete_keys_pads_allocated += 1;
                        allocated_written_pads_count += 1; // Count as used but not confirmed
                    }
                    PadStatus::Written => {
                        incomplete_keys_pads_written += 1;
                        allocated_written_pads_count += 1; // Count as used but not confirmed
                    }
                    PadStatus::Confirmed => {
                        incomplete_keys_pads_confirmed += 1;
                        // Count confirmed pads for incomplete keys towards occupied total
                        occupied_pads_count += 1;
                    }
                }
            }
            // Data size for incomplete keys contributes to the total occupied data estimate
            occupied_data_size_total += key_info.data_size as u64;
        }
    }

    // Total pads managed by the index
    let total_pads_count = occupied_pads_count
        + allocated_written_pads_count
        + free_pads_count
        + pending_verification_pads_count;

    let scratchpad_size_u64 = scratchpad_size as u64;
    // Space calculation based only on confirmed occupied pads
    let occupied_pad_space_bytes = occupied_pads_count as u64 * scratchpad_size_u64;
    let free_pad_space_bytes = free_pads_count as u64 * scratchpad_size_u64;
    let total_space_bytes = total_pads_count as u64 * scratchpad_size_u64;

    // Wasted space compares confirmed pad space vs estimated data size
    let wasted_space_bytes = occupied_pad_space_bytes.saturating_sub(occupied_data_size_total);

    Ok(StorageStats {
        scratchpad_size,
        total_pads: total_pads_count,
        occupied_pads: occupied_pads_count, // Only Confirmed pads
        free_pads: free_pads_count,
        pending_verification_pads: pending_verification_pads_count,
        total_space_bytes,
        occupied_pad_space_bytes,
        free_pad_space_bytes,
        occupied_data_bytes: occupied_data_size_total,
        wasted_space_bytes,
        incomplete_keys_count,
        incomplete_keys_data_bytes,
        incomplete_keys_total_pads,
        incomplete_keys_pads_generated,
        incomplete_keys_pads_written, // Correctly calculated now
        incomplete_keys_pads_confirmed,
        // Note: allocated_written_pads_count is calculated but not part of StorageStats struct
        // Note: incomplete_keys_pads_allocated is calculated but not part of StorageStats struct
    })
}

/// Adds a pad (with counter) to the free list. Checks for duplicates.
pub(crate) fn add_free_pad_with_counter_internal(
    index: &mut MasterIndex,
    address: ScratchpadAddress,
    key_bytes: Vec<u8>,
    counter: u64,
) -> Result<(), IndexError> {
    trace!(
        "Query: add_free_pad_with_counter_internal for address '{}' (counter: {})",
        address,
        counter
    );
    if index.free_pads.iter().any(|(addr, _, _)| *addr == address) {
        warn!("Attempted to add duplicate pad to free list: {}", address);
        return Ok(());
    }
    index.free_pads.push((address, key_bytes, counter));
    Ok(())
}

/// Takes a single pad from the free list, if available, returning counter.
pub(crate) fn take_free_pad_internal(
    index: &mut MasterIndex,
) -> Option<(ScratchpadAddress, Vec<u8>, u64)> {
    // Return tuple includes counter
    trace!("Query: take_free_pad_internal");
    index.free_pads.pop()
}

/// Adds multiple pads (with counters) to the free list. Checks for duplicates.
pub(crate) fn add_free_pads_with_counters_internal(
    index: &mut MasterIndex,
    pads: Vec<(ScratchpadAddress, Vec<u8>, u64)>,
) -> Result<(), IndexError> {
    trace!(
        "Query: add_free_pads_with_counters_internal ({} pads)",
        pads.len()
    );
    for (address, key_bytes, counter) in pads {
        if !index.free_pads.iter().any(|(addr, _, _)| *addr == address) {
            index.free_pads.push((address, key_bytes, counter));
        } else {
            warn!(
                "Attempted to add duplicate pad to free list via batch: {}",
                address
            );
        }
    }
    Ok(())
}

/// Adds multiple pads to the pending verification list.
pub(crate) fn add_pending_verification_pads_internal(
    index: &mut MasterIndex,
    pads: Vec<(ScratchpadAddress, Vec<u8>)>,
) -> Result<(), IndexError> {
    trace!(
        "Query: add_pending_verification_pads_internal ({} pads)",
        pads.len()
    );
    for (address, key_bytes) in pads {
        if !index
            .pending_verification_pads
            .iter()
            .any(|(addr, _)| *addr == address)
        {
            index.pending_verification_pads.push((address, key_bytes));
        } else {
            warn!(
                "Attempted to add duplicate pad to pending list via batch: {}",
                address
            );
        }
    }
    Ok(())
}

/// Takes all pads from the pending verification list.
pub(crate) fn take_pending_pads_internal(
    index: &mut MasterIndex,
) -> Vec<(ScratchpadAddress, Vec<u8>)> {
    trace!("Query: take_pending_pads_internal");
    std::mem::take(&mut index.pending_verification_pads)
}

/// Removes a specific pad address from the pending verification list.
pub(crate) fn remove_from_pending_internal(
    index: &mut MasterIndex,
    address_to_remove: &ScratchpadAddress,
) -> Result<(), IndexError> {
    trace!(
        "Query: remove_from_pending_internal for address '{}'",
        address_to_remove
    );
    index
        .pending_verification_pads
        .retain(|(addr, _)| addr != address_to_remove);
    Ok(())
}

/// Updates the status of a specific pad within a key's info.
pub(crate) fn update_pad_status_internal(
    index: &mut MasterIndex,
    key: &str,
    pad_address: &ScratchpadAddress,
    new_status: PadStatus,
) -> Result<(), IndexError> {
    trace!(
        "Query: update_pad_status_internal for key '{}', pad '{}', status {:?}",
        key,
        pad_address,
        new_status
    );
    if let Some(key_info) = index.index.get_mut(key) {
        if let Some(pad_info) = key_info.pads.iter_mut().find(|p| p.address == *pad_address) {
            pad_info.status = new_status;
            Ok(())
        } else {
            // Pad address provided does not exist within this key's pad list.
            warn!(
                "Attempted to update status for pad {} which is not found in key '{}'",
                pad_address, key
            );
            Err(IndexError::InconsistentState(format!(
                "Pad {} not found in key {}",
                pad_address, key
            )))
        }
    } else {
        Err(IndexError::KeyNotFound(key.to_string()))
    }
}

/// Sets the is_complete flag for a specific key to true.
pub(crate) fn mark_key_complete_internal(
    index: &mut MasterIndex,
    key: &str,
) -> Result<(), IndexError> {
    trace!("Query: mark_key_complete_internal for key '{}'", key);
    if let Some(key_info) = index.index.get_mut(key) {
        key_info.is_complete = true;
        Ok(())
    } else {
        Err(IndexError::KeyNotFound(key.to_string()))
    }
}

/// Resets the index to a default state.
pub(crate) fn reset_index_internal(index: &mut MasterIndex) {
    trace!("Query: reset_index_internal");
    *index = MasterIndex {
        scratchpad_size: index.scratchpad_size.max(DEFAULT_SCRATCHPAD_SIZE), // Keep existing or default size
        ..Default::default()
    };
    debug!("Index reset to default state.");
}

/// Adds a list of pads (address and key bytes) to the pending verification list.
pub(crate) fn add_pending_pads_internal(
    index: &mut MasterIndex,
    pads: Vec<(ScratchpadAddress, Vec<u8>)>, // This is the correct type
) -> Result<(), IndexError> {
    trace!(
        "Query: add_pending_pads_internal adding {} pads",
        pads.len()
    );
    // Extend the existing list with the provided pads
    index.pending_verification_pads.extend(pads);
    Ok(())
}
