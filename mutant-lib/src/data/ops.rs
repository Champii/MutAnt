use crate::data::chunking::{chunk_data, reassemble_data};
use crate::data::error::DataError;
use crate::events::{
    invoke_get_callback, invoke_put_callback, GetCallback, GetEvent, PutCallback, PutEvent,
};
use crate::index::{IndexManager, KeyInfo, PadInfo};
use crate::pad_lifecycle::PadLifecycleManager;
use crate::storage::StorageManager;
use autonomi::{ScratchpadAddress, SecretKey};
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use std::sync::Arc;

// Helper structure to pass down dependencies to operation functions
// Using Arcs for shared ownership across potential concurrent tasks
pub(crate) struct DataManagerDependencies {
    pub index_manager: Arc<dyn IndexManager>,
    pub pad_lifecycle_manager: Arc<dyn PadLifecycleManager>,
    pub storage_manager: Arc<dyn StorageManager>,
    // Add master index address/key if needed for saving index directly?
    // No, IndexManager::save should encapsulate that.
}

// --- Store Operation ---

pub(crate) async fn store_op(
    deps: &DataManagerDependencies,
    user_key: String, // Take ownership
    data_bytes: &[u8],
    mut callback: Option<PutCallback>,
) -> Result<(), DataError> {
    info!("DataOps: Starting store operation for key '{}'", user_key);
    let data_size = data_bytes.len();

    // 1. Get chunk size and chunk data
    let chunk_size = deps.index_manager.get_scratchpad_size().await?;
    let chunks = chunk_data(data_bytes, chunk_size)?;
    let num_chunks = chunks.len();
    debug!("Data chunked into {} pieces.", num_chunks);

    if !invoke_put_callback(
        &mut callback,
        PutEvent::Starting {
            total_chunks: num_chunks,
        },
    )
    .await
    .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }

    // Handle empty data case: store metadata but no pads
    if num_chunks == 0 {
        debug!("Storing empty data for key '{}'", user_key);
        let key_info = KeyInfo {
            pads: Vec::new(),
            data_size,
            modified: Utc::now(),
            is_complete: true,
            populated_pads_count: 0,
        };
        deps.index_manager
            .insert_key_info(user_key, key_info)
            .await?;
        // Save index immediately for empty data? Yes, seems consistent.
        // Need master index address/key here... This suggests IndexManager::save needs adjustment
        // or these details need to be passed down. Let's assume IndexManager handles it for now.
        // deps.index_manager.save(???, ???).await?; // How to get address/key?
        // TODO: Revisit index saving strategy. For now, skip explicit save here. API layer will save.
        if !invoke_put_callback(&mut callback, PutEvent::Complete)
            .await
            .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
        {
            return Err(DataError::OperationCancelled);
        }
        return Ok(());
    }

    // 2. Acquire necessary pads
    debug!("Acquiring {} pads...", num_chunks);
    let acquired_pads = deps.pad_lifecycle_manager.acquire_pads(num_chunks).await?;
    if acquired_pads.len() < num_chunks {
        // Should not happen if acquire_pads works correctly, but check defensively
        error!(
            "Acquired {} pads, but {} were needed. Releasing acquired pads.",
            acquired_pads.len(),
            num_chunks
        );
        // Release the partially acquired pads - requires keys map
        let keys_map: HashMap<_, _> = acquired_pads
            .iter()
            .map(|(a, k)| (*a, k.to_bytes().to_vec()))
            .collect();
        let pad_infos_to_release = acquired_pads
            .iter()
            .map(|(a, _)| PadInfo {
                address: *a,
                chunk_index: 0,
            })
            .collect(); // chunk_index doesn't matter here
        if let Err(e) = deps
            .pad_lifecycle_manager
            .release_pads(pad_infos_to_release, &keys_map)
            .await
        {
            warn!(
                "Failed to release partially acquired pads during store failure: {}",
                e
            );
        }
        return Err(DataError::InsufficientFreePads(format!(
            "Needed {} pads, but only {} were available/acquired",
            num_chunks,
            acquired_pads.len()
        )));
    }
    debug!("Successfully acquired {} pads.", acquired_pads.len());

    // 3. Write chunks concurrently
    let mut write_futures = FuturesUnordered::new();
    let mut pad_info_list = Vec::with_capacity(num_chunks);
    let mut populated_count = 0;

    for (i, chunk) in chunks.into_iter().enumerate() {
        let (pad_address, pad_key) = acquired_pads[i].clone(); // Clone Arc'd key/address
        let storage_manager = Arc::clone(&deps.storage_manager);
        pad_info_list.push(PadInfo {
            address: pad_address,
            chunk_index: i,
        });

        write_futures.push(async move {
            let result = storage_manager.write_pad_data(&pad_address, &chunk).await;
            (i, pad_address, result) // Return index and result
        });
    }

    while let Some((chunk_index, _pad_address, result)) = write_futures.next().await {
        match result {
            Ok(_) => {
                populated_count += 1;
                trace!(
                    "Successfully wrote chunk {} to pad {}",
                    chunk_index,
                    _pad_address
                );
                if !invoke_put_callback(&mut callback, PutEvent::ChunkWritten { chunk_index })
                    .await
                    .map_err(|e| {
                        DataError::InternalError(format!("Callback invocation failed: {}", e))
                    })?
                {
                    // TODO: Handle cancellation during concurrent writes (abort others, release pads?)
                    error!("Store operation cancelled by callback during chunk writing.");
                    // Release ALL acquired pads if cancelled
                    let keys_map: HashMap<_, _> = acquired_pads
                        .iter()
                        .map(|(a, k)| (*a, k.to_bytes().to_vec()))
                        .collect();
                    if let Err(e) = deps
                        .pad_lifecycle_manager
                        .release_pads(pad_info_list, &keys_map)
                        .await
                    {
                        warn!("Failed to release pads after store cancellation: {}", e);
                    }
                    return Err(DataError::OperationCancelled);
                }
            }
            Err(e) => {
                error!(
                    "Failed to write chunk {} to pad {}: {}",
                    chunk_index, _pad_address, e
                );
                // TODO: Handle partial write failure (release pads, mark as incomplete?)
                // Release ALL acquired pads on failure
                let keys_map: HashMap<_, _> = acquired_pads
                    .iter()
                    .map(|(a, k)| (*a, k.to_bytes().to_vec()))
                    .collect();
                if let Err(rel_e) = deps
                    .pad_lifecycle_manager
                    .release_pads(pad_info_list, &keys_map)
                    .await
                {
                    warn!(
                        "Failed to release pads after store write failure: {}",
                        rel_e
                    );
                }
                return Err(DataError::Storage(e));
            }
        }
    }

    debug!("All {} chunks written successfully.", num_chunks);

    // 4. Update index
    let key_info = KeyInfo {
        pads: pad_info_list, // Already ordered by chunk index
        data_size,
        modified: Utc::now(),
        is_complete: true, // Assuming all writes succeeded if we reached here
        populated_pads_count: populated_count,
    };

    deps.index_manager
        .insert_key_info(user_key.clone(), key_info)
        .await?;
    debug!("Index updated for key '{}'", user_key);

    // 5. Save index (explicitly triggered by API layer, not here)
    // if !invoke_put_callback(&mut callback, PutEvent::SavingIndex).await? {
    //     return Err(DataError::OperationCancelled);
    // }
    // deps.index_manager.save(???, ???).await?; // How to get address/key?

    if !invoke_put_callback(&mut callback, PutEvent::Complete)
        .await
        .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }

    info!("DataOps: Store operation complete for key '{}'", user_key);
    Ok(())
}

// --- Fetch Operation ---

pub(crate) async fn fetch_op(
    deps: &DataManagerDependencies,
    user_key: &str,
    mut callback: Option<GetCallback>,
) -> Result<Vec<u8>, DataError> {
    info!("DataOps: Starting fetch operation for key '{}'", user_key);

    // 1. Get KeyInfo from index
    let key_info = deps
        .index_manager
        .get_key_info(user_key)
        .await?
        .ok_or_else(|| DataError::KeyNotFound(user_key.to_string()))?;

    if !key_info.is_complete {
        // Handle incomplete data - return error or partial data? Error for now.
        warn!("Attempting to fetch incomplete data for key '{}'", user_key);
        return Err(DataError::InternalError(format!(
            "Data for key '{}' is marked as incomplete",
            user_key
        )));
    }

    let num_chunks = key_info.pads.len();
    debug!("Found {} chunks for key '{}'", num_chunks, user_key);

    if !invoke_get_callback(
        &mut callback,
        GetEvent::Starting {
            total_chunks: num_chunks,
        },
    )
    .await
    .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }

    // Handle empty data case
    if num_chunks == 0 {
        debug!("Fetching empty data for key '{}'", user_key);
        if key_info.data_size != 0 {
            warn!(
                "Index inconsistency: 0 pads but data_size is {}",
                key_info.data_size
            );
            // Return empty vec anyway? Or error? Let's return empty vec.
        }
        if !invoke_get_callback(&mut callback, GetEvent::Complete)
            .await
            .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
        {
            return Err(DataError::OperationCancelled);
        }
        return Ok(Vec::new());
    }

    // 2. Fetch chunks concurrently
    let mut fetch_futures = FuturesUnordered::new();
    // Sort PadInfo by chunk_index to ensure correct order for reassembly
    let mut sorted_pads = key_info.pads;
    sorted_pads.sort_by_key(|p| p.chunk_index);

    for pad_info in sorted_pads.iter() {
        let storage_manager = Arc::clone(&deps.storage_manager);
        let address = pad_info.address; // Copy address
        let index = pad_info.chunk_index;

        fetch_futures.push(async move {
            let result = storage_manager.read_pad_data(&address).await;
            (index, result) // Return index and result
        });
    }

    // Collect fetched chunks, placing them in a Vec<Option<Vec<u8>>> based on index
    let mut fetched_chunks: Vec<Option<Vec<u8>>> = vec![None; num_chunks];
    let mut fetched_count = 0;

    while let Some((chunk_index, result)) = fetch_futures.next().await {
        match result {
            Ok(data) => {
                trace!(
                    "Successfully fetched chunk {} ({} bytes)",
                    chunk_index,
                    data.len()
                );
                if chunk_index < fetched_chunks.len() {
                    fetched_chunks[chunk_index] = Some(data);
                    fetched_count += 1;
                    if !invoke_get_callback(&mut callback, GetEvent::ChunkFetched { chunk_index })
                        .await
                        .map_err(|e| {
                            DataError::InternalError(format!("Callback invocation failed: {}", e))
                        })?
                    {
                        error!("Fetch operation cancelled by callback during chunk fetching.");
                        return Err(DataError::OperationCancelled);
                    }
                } else {
                    error!(
                        "Invalid chunk index {} returned during fetch (max expected {})",
                        chunk_index,
                        num_chunks - 1
                    );
                    // This indicates an index inconsistency
                    return Err(DataError::InternalError(format!(
                        "Invalid chunk index {} encountered",
                        chunk_index
                    )));
                }
            }
            Err(e) => {
                error!("Failed to fetch chunk {}: {}", chunk_index, e);
                // Don't immediately fail, allow reassembly to detect missing chunk later?
                // Or fail fast? Fail fast seems safer.
                return Err(DataError::Storage(e));
            }
        }
    }

    if fetched_count != num_chunks {
        error!(
            "Fetched {} chunks, but expected {}",
            fetched_count, num_chunks
        );
        // This implies some futures didn't complete or returned invalid indices, should not happen without error above.
        return Err(DataError::InternalError(
            "Mismatch between expected and fetched chunk count".to_string(),
        ));
    }

    debug!("All {} chunks fetched.", num_chunks);

    // 3. Reassemble data
    if !invoke_get_callback(&mut callback, GetEvent::Reassembling)
        .await
        .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }
    let reassembled_data = reassemble_data(fetched_chunks, key_info.data_size)?;
    debug!("Data reassembled successfully.");

    if !invoke_get_callback(&mut callback, GetEvent::Complete)
        .await
        .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }

    info!("DataOps: Fetch operation complete for key '{}'", user_key);
    Ok(reassembled_data)
}

// --- Remove Operation ---

pub(crate) async fn remove_op(
    deps: &DataManagerDependencies,
    user_key: &str,
) -> Result<(), DataError> {
    info!("DataOps: Starting remove operation for key '{}'", user_key);

    // 1. Remove key info from index, getting the old info
    let removed_info = deps.index_manager.remove_key_info(user_key).await?;

    match removed_info {
        Some(key_info) => {
            debug!("Removed key info for '{}' from index.", user_key);
            // 2. Release associated pads
            if !key_info.pads.is_empty() {
                debug!("Releasing {} associated pads...", key_info.pads.len());
                // Need the keys for the pads to release them! Where do we get them?
                // The KeyInfo only stores addresses. The keys were originally in the free_pads list.
                // This implies we cannot *actually* release pads without storing keys alongside addresses
                // or having a global map.
                // TODO: Revisit pad release strategy. For now, we can only remove from index.
                warn!("Pad release during remove is not fully implemented - keys are not stored with KeyInfo.");
                // Placeholder: If keys were available (e.g., in a HashMap passed down)
                // let keys_map: HashMap<_, _> = ...;
                // deps.pad_lifecycle_manager.release_pads(key_info.pads, &keys_map).await?;
            } else {
                debug!("No pads associated with key '{}' to release.", user_key);
            }

            // 3. Save index (explicitly triggered by API layer)
            // deps.index_manager.save(???, ???).await?;

            info!("DataOps: Remove operation complete for key '{}'", user_key);
            Ok(())
        }
        None => {
            warn!("Attempted to remove non-existent key '{}'", user_key);
            // Return Ok or KeyNotFound? Original returned Ok. Let's stick with that.
            Ok(())
            // Err(DataError::KeyNotFound(user_key.to_string()))
        }
    }
}

// --- Update Operation ---
// TODO: Implement update_op. This is complex:
// 1. Fetch existing KeyInfo. Error if not found.
// 2. Chunk new data.
// 3. Compare new chunk count with old pad count.
// 4. If counts differ:
//    - Acquire new pads if needed.
//    - Identify pads to release if shrinking. Need keys for release!
// 5. Write new/updated chunks concurrently (potentially overwriting existing pads).
// 6. Release any now-unused pads.
// 7. Update KeyInfo (new size, timestamp, potentially new pad list).
// 8. Update index.
// 9. Save index (via API layer).
// Requires careful handling of partial failures and pad key management.

pub(crate) async fn update_op(
    deps: &DataManagerDependencies,
    user_key: String,
    data_bytes: &[u8],
    mut callback: Option<PutCallback>,
) -> Result<(), DataError> {
    info!("DataOps: Starting update operation for key '{}'", user_key);
    warn!("DataOps: Update operation is complex and potentially incomplete regarding pad key management for release.");

    // 1. Get existing KeyInfo
    let old_key_info = deps
        .index_manager
        .get_key_info(&user_key)
        .await?
        .ok_or_else(|| DataError::KeyNotFound(user_key.to_string()))?;

    // TODO: Check if old_key_info.is_complete? What if updating incomplete data?

    // 2. Chunk new data
    let new_data_size = data_bytes.len();
    let chunk_size = deps.index_manager.get_scratchpad_size().await?;
    let new_chunks = chunk_data(data_bytes, chunk_size)?;
    let new_num_chunks = new_chunks.len();
    debug!("New data chunked into {} pieces.", new_num_chunks);

    if !invoke_put_callback(
        &mut callback,
        PutEvent::Starting {
            total_chunks: new_num_chunks,
        },
    )
    .await
    .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }

    // --- Pad Management ---
    let old_num_chunks = old_key_info.pads.len();
    let mut acquired_pads = Vec::new(); // Tuples of (Address, Key)
    let mut pads_to_use = old_key_info.pads.clone(); // Start with old pads
    let mut pads_to_release_info = Vec::new(); // PadInfo of pads to release

    if new_num_chunks > old_num_chunks {
        // Need more pads
        let needed = new_num_chunks - old_num_chunks;
        debug!("Acquiring {} additional pads for update...", needed);
        acquired_pads = deps.pad_lifecycle_manager.acquire_pads(needed).await?;
        // Extend pads_to_use with info for the newly acquired pads
        for (i, (addr, _key)) in acquired_pads.iter().enumerate() {
            pads_to_use.push(PadInfo {
                address: *addr,
                chunk_index: old_num_chunks + i,
            });
        }
    } else if new_num_chunks < old_num_chunks {
        // Need fewer pads, mark some for release
        debug!(
            "{} pads no longer needed, marking for release.",
            old_num_chunks - new_num_chunks
        );
        pads_to_release_info = pads_to_use.split_off(new_num_chunks); // Remove excess pads from the end
    }
    // Ensure pads_to_use now has exactly new_num_chunks elements
    assert_eq!(pads_to_use.len(), new_num_chunks);

    // --- Write Chunks Concurrently ---
    let mut write_futures = FuturesUnordered::new();
    let mut populated_count = 0;
    // We need the keys for *all* pads we are writing to (old and new)
    // Combine old pad keys (how to get?) and new acquired pad keys
    let mut all_pad_keys: HashMap<ScratchpadAddress, SecretKey> = HashMap::new();
    for (addr, key) in acquired_pads.iter() {
        all_pad_keys.insert(*addr, key.clone());
    }
    // TODO: How to get keys for the old pads being reused (pads_to_use[0..old_num_chunks])?
    // This is a major gap - keys aren't stored in KeyInfo. Assume failure for now.
    if old_num_chunks > 0 && new_num_chunks > 0 && all_pad_keys.len() < new_num_chunks {
        error!("Cannot perform update: Missing secret keys for reused pads.");
        // Release newly acquired pads if any
        if !acquired_pads.is_empty() {
            let keys_map: HashMap<_, _> = acquired_pads
                .iter()
                .map(|(a, k)| (*a, k.to_bytes().to_vec()))
                .collect();
            let pad_infos_to_release = acquired_pads
                .iter()
                .map(|(a, _)| PadInfo {
                    address: *a,
                    chunk_index: 0,
                })
                .collect();
            if let Err(e) = deps
                .pad_lifecycle_manager
                .release_pads(pad_infos_to_release, &keys_map)
                .await
            {
                warn!(
                    "Failed to release newly acquired pads during update failure: {}",
                    e
                );
            }
        }
        return Err(DataError::InternalError(
            "Missing keys for reused pads during update".to_string(),
        ));
    }

    for (i, chunk) in new_chunks.into_iter().enumerate() {
        let pad_info = &pads_to_use[i]; // Address comes from here
        let pad_address = pad_info.address;
        // Get the key from our combined map
        let pad_key = all_pad_keys
            .get(&pad_address)
            .ok_or_else(|| {
                DataError::InternalError(format!(
                    "Missing key for pad {} during update write",
                    pad_address
                ))
            })?
            .clone();
        let storage_manager = Arc::clone(&deps.storage_manager);

        write_futures.push(async move {
            let result = storage_manager.write_pad_data(&pad_address, &chunk).await;
            (i, pad_address, result)
        });
    }

    while let Some((chunk_index, _pad_address, result)) = write_futures.next().await {
        match result {
            Ok(_) => {
                populated_count += 1;
                trace!(
                    "Successfully wrote chunk {} to pad {}",
                    chunk_index,
                    _pad_address
                );
                if !invoke_put_callback(&mut callback, PutEvent::ChunkWritten { chunk_index })
                    .await
                    .map_err(|e| {
                        DataError::InternalError(format!("Callback invocation failed: {}", e))
                    })?
                {
                    error!("Update operation cancelled by callback during chunk writing.");
                    // TODO: Release acquired pads and potentially revert index changes? Complex.
                    return Err(DataError::OperationCancelled);
                }
            }
            Err(e) => {
                error!(
                    "Failed to write chunk {} to pad {}: {}",
                    chunk_index, _pad_address, e
                );
                // TODO: Handle partial write failure during update. Release acquired pads?
                return Err(DataError::Storage(e));
            }
        }
    }
    debug!(
        "All {} new chunks written successfully for update.",
        new_num_chunks
    );

    // --- Release Unused Pads ---
    if !pads_to_release_info.is_empty() {
        debug!("Releasing {} unused pads...", pads_to_release_info.len());
        // TODO: Need keys for pads_to_release! Cannot proceed without them.
        warn!("Cannot release unused pads during update: Missing secret keys.");
        // Placeholder: If keys were available
        // let keys_map_to_release: HashMap<_, _> = ...;
        // if let Err(e) = deps.pad_lifecycle_manager.release_pads(pads_to_release_info, &keys_map_to_release).await {
        //     warn!("Failed to release unused pads during update: {}", e);
        //     // Continue with index update despite release failure? Or return error?
        // }
    }

    // --- Update Index ---
    let new_key_info = KeyInfo {
        pads: pads_to_use, // Contains only the pads used for the new data
        data_size: new_data_size,
        modified: Utc::now(),
        is_complete: true, // Assuming all writes succeeded
        populated_pads_count: populated_count,
    };

    deps.index_manager
        .insert_key_info(user_key.clone(), new_key_info)
        .await?;
    debug!(
        "Index updated for key '{}' after update operation.",
        user_key
    );

    // --- Save Index (via API layer) ---
    // if !invoke_put_callback(&mut callback, PutEvent::SavingIndex).await? {
    //     return Err(DataError::OperationCancelled);
    // }
    // deps.index_manager.save(???, ???).await?;

    if !invoke_put_callback(&mut callback, PutEvent::Complete)
        .await
        .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }

    info!("DataOps: Update operation complete for key '{}'", user_key);
    Ok(())
}
