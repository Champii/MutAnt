use crate::network::error::NetworkError;
use crate::network::NetworkChoice;
use autonomi::{Network, SecretKey, Wallet};
use hex;
use log::info;
use sha2::{Digest, Sha256};

pub(crate) fn create_wallet(
    private_key_hex: &str,
    network_choice: NetworkChoice,
) -> Result<(Wallet, SecretKey), NetworkError> {
    info!(
        "Creating Autonomi wallet and key for network: {:?}",
        network_choice
    );
    let network = Network::new(network_choice == NetworkChoice::Devnet)
        .map_err(|e| NetworkError::NetworkInitError(format!("Network init failed: {}", e)))?;

    let hex_to_decode = if private_key_hex.starts_with("0x") {
        &private_key_hex[2..]
    } else {
        private_key_hex
    };

    let input_key_bytes = hex::decode(hex_to_decode).map_err(|e| {
        NetworkError::InvalidKeyInput(format!("Failed to decode private key hex: {}", e))
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&input_key_bytes);
    let hash_result = hasher.finalize();
    let key_array: [u8; 32] = hash_result.into();
    let secret_key = SecretKey::from_bytes(key_array).map_err(|e| {
        NetworkError::InvalidKeyInput(format!("Failed to create SecretKey from HASH: {:?}", e))
    })?;

    let wallet = Wallet::new_from_private_key(network, private_key_hex)
        .map_err(|e| NetworkError::WalletError(format!("Failed to create wallet: {}", e)))?;

    Ok((wallet, secret_key))
}
