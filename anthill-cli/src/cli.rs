use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Path to the wallet file.
    #[arg(
        short,
        long,
        value_name = "FILE",
        default_value = "anthill_wallet.json"
    )]
    pub wallet_path: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Puts a key-value pair onto the network. Reads value from stdin if omitted.
    /// Use --force to overwrite an existing key.
    Put {
        key: String,
        value: Option<String>,
        #[arg(short, long, default_value_t = false)]
        force: bool,
    },
    /// Gets the value for a given key from the network and prints it to stdout.
    Get { key: String },
    /// Deletes a key-value pair from the network
    Rm { key: String },
    /// Lists all keys stored on the network
    Ls {
        #[arg(short, long, default_value_t = false)]
        long: bool,
    },
    /// Get storage summary (allocator perspective)
    Stats,
}
