use crate::data::chunking::reassemble_data;
use crate::data::error::DataError;
use crate::internal_events::{invoke_get_callback, GetCallback, GetEvent};

use autonomi::SecretKey;
use futures::stream::{FuturesUnordered, StreamExt};
use log::{debug, error, info, trace, warn};

use std::sync::Arc;

/// Fetches data associated with a user key from the Autonomi network.
///
/// This operation retrieves all chunks belonging to the key, decrypts them,
/// and reassembles them into the original data. It requires the key's
/// metadata to be complete (`is_complete` flag in `KeyInfo`).
///
/// Progress can be monitored via an optional callback function.
///
/// # Arguments
///
/// * `data_manager` - A reference to the `DefaultDataManager` instance.
/// * `user_key` - The key identifying the data to fetch.
/// * `callback` - An optional callback function to report progress events.
///
/// # Errors
///
/// Returns `DataError` if:
/// - The key is not found (`DataError::KeyNotFound`).
/// - The key's metadata is marked as incomplete (`DataError::InternalError`).
/// - The operation is cancelled via the callback (`DataError::OperationCancelled`).
/// - A network error occurs during fetching (`DataError::Network`).
/// - An internal error occurs (e.g., missing pad keys, decryption failure, index inconsistency) (`DataError::InternalError`).
/// - Chunk reassembly fails (`DataError::ChunkingError`).
/// - Callback invocation fails (`DataError::InternalError`).
pub(crate) async fn fetch_op(
    data_manager: &crate::data::manager::DefaultDataManager,
    user_key: &str,
    mut callback: Option<GetCallback>,
) -> Result<Vec<u8>, DataError> {
    info!("DataOps: Starting fetch operation for key '{}'", user_key);

    let key_info = data_manager
        .index_manager
        .get_key_info(user_key)
        .await?
        .ok_or_else(|| DataError::KeyNotFound(user_key.to_string()))?;

    let _index_copy = data_manager.index_manager.get_index_copy().await?;

    if !key_info.is_complete {
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

    if num_chunks == 0 {
        debug!("Fetching empty data for key '{}'", user_key);
        if key_info.data_size != 0 {
            warn!(
                "Index inconsistency: 0 pads but data_size is {}",
                key_info.data_size
            );
        }
        if !invoke_get_callback(&mut callback, GetEvent::Complete)
            .await
            .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
        {
            return Err(DataError::OperationCancelled);
        }
        return Ok(Vec::new());
    }

    let mut fetch_futures = FuturesUnordered::new();
    let mut sorted_pads = key_info.pads.clone();
    sorted_pads.sort_by_key(|p| p.chunk_index);

    let network_adapter = Arc::clone(&data_manager.network_adapter);
    for pad_info in sorted_pads.iter() {
        let adapter_clone = Arc::clone(&network_adapter);
        let address = pad_info.address;
        let index = pad_info.chunk_index;
        fetch_futures.push(async move {
            let result = adapter_clone.get_raw_scratchpad(&address).await;
            (index, address, result)
        });
    }

    let mut fetched_decrypted_chunks: Vec<Option<Vec<u8>>> = vec![None; num_chunks];
    let mut fetched_count = 0;

    while let Some((chunk_index, pad_address, result)) = fetch_futures.next().await {
        match result {
            Ok(scratchpad) => {
                trace!(
                    "Successfully fetched scratchpad for chunk {} from pad {}",
                    chunk_index,
                    pad_address
                );

                let key_bytes_vec = key_info.pad_keys.get(&pad_address).ok_or_else(|| {
                    error!("Secret key for pad {} not found in KeyInfo", pad_address);
                    DataError::InternalError(format!("Pad key missing for {}", pad_address))
                })?;

                let pad_secret_key = {
                    let key_array: [u8; 32] =
                        key_bytes_vec.as_slice().try_into().map_err(|_| {
                            error!(
                                "Secret key for pad {} has incorrect length (expected 32): {}",
                                pad_address,
                                key_bytes_vec.len()
                            );
                            DataError::InternalError(format!(
                                "Invalid key length for {}",
                                pad_address
                            ))
                        })?;
                    SecretKey::from_bytes(key_array).map_err(|e| {
                        error!(
                            "Failed to deserialize secret key for pad {}: {}",
                            pad_address, e
                        );
                        DataError::InternalError(format!(
                            "Pad key deserialization failed for {}",
                            pad_address
                        ))
                    })?
                };

                let decrypted_data = scratchpad.decrypt_data(&pad_secret_key).map_err(|e| {
                    error!(
                        "Failed to decrypt chunk {} from pad {}: {}",
                        chunk_index, pad_address, e
                    );

                    DataError::InternalError(format!(
                        "Chunk decryption failed for pad {}",
                        pad_address
                    ))
                })?;
                trace!("Decrypted chunk {} successfully", chunk_index);

                if chunk_index < fetched_decrypted_chunks.len() {
                    fetched_decrypted_chunks[chunk_index] = Some(decrypted_data.to_vec());
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
                    return Err(DataError::InternalError(format!(
                        "Invalid chunk index {} encountered",
                        chunk_index
                    )));
                }
            }
            Err(e) => {
                debug!("Failed to fetch chunk {}: {}", chunk_index, e);
                return Err(DataError::Network(e));
            }
        }
    }

    if fetched_count != num_chunks {
        error!(
            "Fetched {} chunks, but expected {}",
            fetched_count, num_chunks
        );

        return Err(DataError::InternalError(
            "Mismatch between expected and fetched chunk count".to_string(),
        ));
    }

    debug!("All {} chunks fetched and decrypted.", num_chunks);

    if !invoke_get_callback(&mut callback, GetEvent::Reassembling)
        .await
        .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }
    let reassembled_data = reassemble_data(fetched_decrypted_chunks, key_info.data_size)?;
    debug!("Decrypted data reassembled successfully.");

    if !invoke_get_callback(&mut callback, GetEvent::Complete)
        .await
        .map_err(|e| DataError::InternalError(format!("Callback invocation failed: {}", e)))?
    {
        return Err(DataError::OperationCancelled);
    }

    info!("DataOps: Fetch operation complete for key '{}'", user_key);
    Ok(reassembled_data)
}
