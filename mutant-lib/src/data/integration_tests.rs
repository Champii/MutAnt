#![cfg(test)]

use crate::data::chunking::{chunk_data, reassemble_data};
use crate::data::error::DataError;
use crate::data::manager::DefaultDataManager;
use crate::index::manager::DefaultIndexManager;
use crate::network::adapter::AutonomiNetworkAdapter;
use crate::network::NetworkChoice;
use crate::pad_lifecycle::manager::DefaultPadLifecycleManager;
use crate::storage::manager::DefaultStorageManager;
use std::sync::Arc;

// Define constant locally
const DEV_TESTNET_PRIVATE_KEY_HEX: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

async fn setup_managers() -> (
    Arc<AutonomiNetworkAdapter>,
    Arc<DefaultStorageManager>,
    Arc<DefaultIndexManager>,
    Arc<DefaultPadLifecycleManager>,
    DefaultDataManager,
) {
    tokio::time::sleep(std::time::Duration::from_millis(50)).await; // Prevent rate limiting

    let network_adapter: Arc<AutonomiNetworkAdapter> = Arc::new(
        AutonomiNetworkAdapter::new(DEV_TESTNET_PRIVATE_KEY_HEX, NetworkChoice::Devnet)
            .expect("Test NetworkAdapter setup failed"),
    );

    let storage_manager: Arc<DefaultStorageManager> =
        Arc::new(DefaultStorageManager::new(network_adapter.clone()));
    let index_manager_impl = Arc::new(DefaultIndexManager::new(
        storage_manager.clone(),
        network_adapter.clone(),
    ));
    // Use load_or_initialize, assuming a dummy key/address is fine for data tests
    // If these tests require specific index state, this setup needs adjustment.
    let dummy_key = autonomi::SecretKey::random();
    let dummy_addr = autonomi::ScratchpadAddress::new(dummy_key.public_key());
    index_manager_impl
        .load_or_initialize(&dummy_addr, &dummy_key)
        .await
        .expect("Index initialization failed in data test setup");
    // Save might not be strictly needed here, but keeping for consistency
    index_manager_impl
        .save(&dummy_addr, &dummy_key)
        .await
        .expect("Initial index save failed in data test setup");

    let index_manager: Arc<DefaultIndexManager> = index_manager_impl.clone(); // Keep Arc for dependencies

    let pad_lifecycle_manager_impl = Arc::new(DefaultPadLifecycleManager::new(
        index_manager.clone(),
        network_adapter.clone(),
        storage_manager.clone(),
    ));
    let pad_lifecycle_manager: Arc<DefaultPadLifecycleManager> = pad_lifecycle_manager_impl; // Keep Arc

    let data_manager = DefaultDataManager::new(
        index_manager.clone(),
        pad_lifecycle_manager.clone(),
        storage_manager.clone(),
        network_adapter.clone(),
    );

    // Need to pass TEST_SCRATCHPAD_SIZE to store/fetch calls now, not constructor

    (
        network_adapter,
        storage_manager,
        index_manager,
        pad_lifecycle_manager,
        data_manager,
    )
}

#[test]
fn test_chunking_basic() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let chunk_size = 3;
    let chunks = chunk_data(&data, chunk_size).unwrap();
    assert_eq!(chunks.len(), 4);
    assert_eq!(chunks[0], vec![1, 2, 3]);
    assert_eq!(chunks[1], vec![4, 5, 6]);
    assert_eq!(chunks[2], vec![7, 8, 9]);
    assert_eq!(chunks[3], vec![10]);
}

#[test]
fn test_chunking_exact_multiple() {
    let data = vec![1, 2, 3, 4, 5, 6];
    let chunk_size = 3;
    let chunks = chunk_data(&data, chunk_size).unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0], vec![1, 2, 3]);
    assert_eq!(chunks[1], vec![4, 5, 6]);
}

#[test]
fn test_chunking_larger_than_data() {
    let data = vec![1, 2, 3];
    let chunk_size = 10;
    let chunks = chunk_data(&data, chunk_size).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], vec![1, 2, 3]);
}

#[test]
fn test_chunking_empty_data() {
    let data = vec![];
    let chunk_size = 10;
    let chunks = chunk_data(&data, chunk_size).unwrap();
    assert_eq!(chunks.len(), 0);
}

#[test]
fn test_chunking_zero_size() {
    let data = vec![1, 2, 3];
    let chunk_size = 0;
    let result = chunk_data(&data, chunk_size);
    assert!(result.is_err());
    match result.err().unwrap() {
        DataError::ChunkingError(msg) => assert_eq!(msg, "Chunk size cannot be zero"),
        _ => panic!("Unexpected error type"),
    }
}

#[test]
fn test_reassemble_basic() {
    let chunks = vec![
        Some(vec![1, 2, 3]),
        Some(vec![4, 5, 6]),
        Some(vec![7, 8, 9]),
        Some(vec![10]),
    ];
    let expected_size = 10;
    let data = reassemble_data(chunks, expected_size).unwrap();
    assert_eq!(data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
}

#[test]
fn test_reassemble_empty() {
    let chunks = vec![];
    let expected_size = 0;
    let data = reassemble_data(chunks, expected_size).unwrap();
    assert_eq!(data, Vec::<u8>::new());
}

#[test]
fn test_reassemble_size_mismatch_too_small() {
    let chunks = vec![Some(vec![1, 2]), Some(vec![3])];
    let expected_size = 4;
    let result = reassemble_data(chunks, expected_size);
    assert!(result.is_err());
    match result.err().unwrap() {
        DataError::ReassemblyError(msg) => {
            assert!(msg.contains("does not match expected size"))
        }
        _ => panic!("Unexpected error type"),
    }
}

#[test]
fn test_reassemble_size_mismatch_too_large() {
    let chunks = vec![Some(vec![1, 2]), Some(vec![3, 4, 5])];
    let expected_size = 3;
    let result = reassemble_data(chunks, expected_size);
    assert!(result.is_err());
    match result.err().unwrap() {
        DataError::ReassemblyError(msg) => {
            assert!(msg.contains("does not match expected size"))
        }
        _ => panic!("Unexpected error type"),
    }
}

#[test]
fn test_reassemble_missing_chunk() {
    let chunks = vec![Some(vec![1, 2]), None, Some(vec![5, 6])];
    let expected_size = 6;
    let result = reassemble_data(chunks, expected_size);
    assert!(result.is_err());
    match result.err().unwrap() {
        DataError::ReassemblyError(msg) => assert!(msg.contains("Missing chunk at index 1")),
        _ => panic!("Unexpected error type"),
    }
}

#[tokio::test]
async fn test_store_very_large_data_multi_pad() {
    let (_network_adapter, _storage_manager, index_manager, _pad_lifecycle_manager, data_manager) =
        setup_managers().await;

    let key = "very_large_data_key".to_string();

    let data_size = 16 * 1024 * 1024;
    let data = vec![1u8; data_size];

    let store_result = data_manager.store(key.clone(), &data, None).await;
    assert!(
        store_result.is_ok(),
        "Store very large data failed: {:?}",
        store_result.err()
    );

    let key_info_res = index_manager.get_key_info(&key).await;
    assert!(
        key_info_res.is_ok(),
        "get_key_info failed: {:?}",
        key_info_res.err()
    );
    let key_info_opt = key_info_res.unwrap();
    assert!(
        key_info_opt.is_some(),
        "Key info not found after store (very large)"
    );
    let key_info = key_info_opt.unwrap();
    let expected_pads = 5;
    assert_eq!(
        key_info.pads.len(),
        expected_pads,
        "Expected exactly {} pads for 16MiB data, found {}",
        expected_pads,
        key_info.pads.len()
    );
    assert_eq!(key_info.data_size, data.len());

    let fetch_result = data_manager.fetch(&key, None).await;
    assert!(
        fetch_result.is_ok(),
        "Fetch very large data failed: {:?}",
        fetch_result.err()
    );
    assert_eq!(
        fetch_result.unwrap(),
        data,
        "Fetched very large data mismatch"
    );

    let remove_result = data_manager.remove(&key).await;
    assert!(
        remove_result.is_ok(),
        "Remove very large data failed: {:?}",
        remove_result.err()
    );

    let key_info_after_remove_res = index_manager.get_key_info(&key).await;
    assert!(
        key_info_after_remove_res.is_ok(),
        "get_key_info after remove failed: {:?}",
        key_info_after_remove_res.err()
    );
    let key_info_after_remove_opt = key_info_after_remove_res.unwrap();
    assert!(
        key_info_after_remove_opt.is_none(),
        "Key info still present after remove (very large)"
    );
}

#[tokio::test]
async fn test_store_basic() {
    let (_network, _storage, index_manager, _pad_lifecycle, data_manager) = setup_managers().await;
    let key = "store_basic_key".to_string();
    let data = b"some simple data".to_vec();

    let store_result = data_manager.store(key.clone(), &data, None).await;
    assert!(
        store_result.is_ok(),
        "Store failed: {:?}",
        store_result.err()
    );

    // Verification: Check index
    let key_info_res = index_manager.get_key_info(&key).await;
    assert!(key_info_res.is_ok());
    let key_info = key_info_res.unwrap();
    assert!(key_info.is_some());
    let info = key_info.unwrap();
    assert_eq!(info.data_size as usize, data.len());
    assert!(!info.pads.is_empty());
}

#[tokio::test]
async fn test_fetch_basic() {
    let (_network, _storage, _index_manager, _pad_lifecycle, data_manager) = setup_managers().await;
    let key = "fetch_basic_key".to_string();
    let data = b"some data to fetch".to_vec();

    data_manager.store(key.clone(), &data, None).await.unwrap();

    let fetch_result = data_manager.fetch(&key, None).await;
    assert!(
        fetch_result.is_ok(),
        "Fetch failed: {:?}",
        fetch_result.err()
    );
    assert_eq!(fetch_result.unwrap(), data);
}
