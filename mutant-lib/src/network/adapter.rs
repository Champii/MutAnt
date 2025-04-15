use crate::network::client::create_client;
use crate::network::error::NetworkError;
use crate::network::wallet::create_wallet;
use crate::network::NetworkChoice;
use async_trait::async_trait;
use autonomi::client::payment::PaymentOption;
use autonomi::{Bytes, Client, Scratchpad, ScratchpadAddress, SecretKey, Wallet};
use log::{debug, error, info, trace};
use std::sync::Arc;

/// Trait defining the interface for low-level network operations related to scratchpads.
/// This abstracts the underlying network implementation (e.g., autonomi client).
#[async_trait]
pub trait NetworkAdapter: Send + Sync {
    /// Fetches the full Scratchpad object from a scratchpad address.
    /// Decryption must be handled by the caller using the appropriate key and the Scratchpad's decrypt_data method.
    async fn get_raw_scratchpad(
        &self,
        address: &ScratchpadAddress,
    ) -> Result<Scratchpad, NetworkError>;

    /// Puts raw data into a scratchpad associated with the given secret key.
    /// Creates the scratchpad if it doesn't exist, updates it otherwise.
    /// Returns the address of the scratchpad.
    async fn put_raw(
        &self,
        key: &SecretKey,
        data: &[u8],
    ) -> Result<ScratchpadAddress, NetworkError>;

    /// Checks if a scratchpad exists at the given address.
    async fn check_existence(&self, address: &ScratchpadAddress) -> Result<bool, NetworkError>;

    /// Returns the network choice (Devnet/Mainnet) the adapter is configured for.
    fn get_network_choice(&self) -> NetworkChoice;

    /// Provides access to the underlying wallet instance.
    /// Use with caution; prefer methods that abstract wallet interactions.
    fn wallet(&self) -> &Wallet;

    // Consider adding:
    // async fn get_client(&self) -> Result<autonomi::Client, NetworkError>; // If direct client access is truly needed, but ideally avoided.
}

// --- Implementation ---

/// Concrete implementation of NetworkAdapter using the autonomi crate.
#[derive(Clone)] // Clone is cheap due to Arc
pub struct AutonomiNetworkAdapter {
    wallet: Wallet, // Wallet is currently only used during client creation, might be removable later
    client: Arc<Client>,
    network_choice: NetworkChoice, // Store for quick access
    secret_key: SecretKey,         // Store the secret key
}

impl AutonomiNetworkAdapter {
    /// Creates a new AutonomiNetworkAdapter instance.
    pub async fn new(
        private_key_hex: &str,
        network_choice: NetworkChoice,
    ) -> Result<Self, NetworkError> {
        debug!(
            "Creating AutonomiNetworkAdapter for network: {:?}",
            network_choice
        );
        // create_wallet returns (Wallet, SecretKey). Store both.
        let (wallet, key) = create_wallet(private_key_hex, network_choice)?;
        // Pass network_choice to client creation instead of wallet
        let client = create_client(network_choice).await?;
        Ok(Self {
            wallet, // Keep wallet for now, needed for PaymentOption
            client: Arc::new(client),
            network_choice,
            secret_key: key, // Store the secret key
        })
    }
}

#[async_trait]
impl NetworkAdapter for AutonomiNetworkAdapter {
    async fn get_raw_scratchpad(
        &self,
        address: &ScratchpadAddress,
    ) -> Result<Scratchpad, NetworkError> {
        trace!(
            "NetworkAdapter::get_raw_scratchpad called for address: {}",
            address
        );
        // Fetch the Scratchpad object
        let scratchpad: Scratchpad =
            self.client.scratchpad_get(address).await.map_err(|e| {
                NetworkError::InternalError(format!("Failed to get scratchpad: {}", e))
            })?;

        // Return the whole Scratchpad object
        Ok(scratchpad)
    }

    async fn put_raw(
        &self,
        key: &SecretKey,
        data: &[u8],
    ) -> Result<ScratchpadAddress, NetworkError> {
        trace!("NetworkAdapter::put_raw called, data_len: {}", data.len());
        // The 'key' parameter is now used from the function arguments,
        // but we still derive the address from it.
        let public_key = key.public_key();
        let address = ScratchpadAddress::new(public_key);
        let data_bytes = Bytes::copy_from_slice(data); // Convert to autonomi::Bytes

        // Use default content type
        let content_type = 0u64;
        // Use PaymentOption::Wallet with the adapter's wallet
        let payment_option = PaymentOption::Wallet(self.wallet.clone());

        debug!("Checking existence of scratchpad at address: {}", address);

        match self.client.scratchpad_check_existance(&address).await {
            Ok(true) => {
                // Pad exists, update it
                info!("Scratchpad exists at {}, updating...", address);
                self.client
                    .scratchpad_update(key, content_type, &data_bytes)
                    .await
                    .map_err(|e| {
                        NetworkError::InternalError(format!("Failed to update scratchpad: {}", e))
                    })?;
                Ok(address) // Return address on success
            }
            Ok(false) => {
                // Pad does not exist, create it
                info!("Scratchpad does not exist at {}, creating...", address);
                let result_tuple = self
                    .client
                    .scratchpad_create(key, content_type, &data_bytes, payment_option)
                    .await
                    // Convert Autonomi error to NetworkError::InternalError
                    .map_err(|e| {
                        NetworkError::InternalError(format!("Failed to create scratchpad: {}", e))
                    })?;

                // Extract the address from the tuple result after handling the error
                let (_cost, created_addr) = result_tuple;
                Ok(created_addr)
            }
            Err(e) => {
                // Error during check existence
                error!("Error checking scratchpad existence at {}: {}", address, e);
                // Convert Autonomi error to NetworkError::InternalError
                Err(NetworkError::InternalError(format!(
                    "Failed to check scratchpad existence: {}",
                    e
                )))
            }
        }
    }

    async fn check_existence(&self, address: &ScratchpadAddress) -> Result<bool, NetworkError> {
        trace!(
            "NetworkAdapter::check_existence called for address: {}",
            address
        );
        self.client
            .scratchpad_check_existance(address)
            .await
            .map_err(|e| {
                NetworkError::InternalError(format!("Failed to check scratchpad existence: {}", e))
            })
    }

    fn get_network_choice(&self) -> NetworkChoice {
        self.network_choice
    }

    fn wallet(&self) -> &Wallet {
        &self.wallet
    }
}
