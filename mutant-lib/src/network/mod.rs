pub mod adapter;
pub mod client;
pub mod error;
pub mod wallet;

pub use adapter::AutonomiNetworkAdapter;
pub use error::NetworkError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NetworkChoice {
    Devnet,
    Mainnet,
}

impl Default for NetworkChoice {
    fn default() -> Self {
        NetworkChoice::Mainnet // Default to Mainnet
    }
}

#[cfg(test)]
pub mod integration_tests;
