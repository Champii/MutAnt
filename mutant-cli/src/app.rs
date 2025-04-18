use clap::Parser;
use dialoguer::{Select, theme::ColorfulTheme};
use directories::{BaseDirs, ProjectDirs};
use indicatif::{MultiProgress, ProgressDrawTarget};
use log::{debug, error, info, warn};

use mutant_lib::config::MutAntConfig;
use mutant_lib::error::Error as LibError;
use mutant_lib::{MutAnt, config::NetworkChoice};

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tokio::task::JoinHandle;

use crate::callbacks::create_init_callback;
use crate::cli::{Cli, Commands};

#[derive(Serialize, Deserialize, Debug, Default)]
struct MutantCliConfig {
    wallet_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum CliError {
    WalletRead(io::Error, PathBuf),
    MutAntInit(String),
    ConfigDirNotFound,
    ConfigRead(io::Error, PathBuf),
    ConfigParse(serde_json::Error, PathBuf),
    ConfigWrite(io::Error, PathBuf),
    WalletDirNotFound,
    WalletDirRead(io::Error, PathBuf),
    NoWalletsFound(PathBuf),
    UserSelectionFailed(dialoguer::Error),
    WalletNotSet,
    UserInputAborted(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::WalletRead(e, path) => {
                write!(f, "Error reading private key from {:?}: {}", path, e)
            }
            CliError::MutAntInit(e) => write!(f, "Error during MutAnt initialization: {}", e),
            CliError::ConfigDirNotFound => write!(f, "Could not find configuration directory."),
            CliError::ConfigRead(e, path) => {
                write!(f, "Error reading config file {:?}: {}", path, e)
            }
            CliError::ConfigParse(e, path) => {
                write!(f, "Error parsing config file {:?}: {}", path, e)
            }
            CliError::ConfigWrite(e, path) => {
                write!(f, "Error writing config file {:?}: {}", path, e)
            }
            CliError::WalletDirNotFound => write!(f, "Could not find Autonomi wallet directory."),
            CliError::WalletDirRead(e, path) => {
                write!(f, "Error reading wallet directory {:?}: {}", path, e)
            }
            CliError::NoWalletsFound(path) => write!(f, "No wallet files found in {:?}", path),
            CliError::UserSelectionFailed(e) => {
                write!(f, "Failed to get user wallet selection: {}", e)
            }
            CliError::WalletNotSet => write!(f, "No wallet configured or selected."),
            CliError::UserInputAborted(msg) => write!(f, "Operation aborted by user: {}", msg),
        }
    }
}

impl std::error::Error for CliError {}

impl From<LibError> for CliError {
    fn from(lib_err: LibError) -> Self {
        CliError::MutAntInit(lib_err.to_string())
    }
}

fn get_config_path() -> Result<PathBuf, CliError> {
    let proj_dirs =
        ProjectDirs::from("com", "Mutant", "MutantCli").ok_or(CliError::ConfigDirNotFound)?;
    let config_dir = proj_dirs.config_dir();
    if !config_dir.exists() {
        fs::create_dir_all(config_dir)
            .map_err(|e| CliError::ConfigWrite(e, config_dir.to_path_buf()))?;
    }
    Ok(config_dir.join("mutant.json"))
}

fn load_config(config_path: &Path) -> Result<MutantCliConfig, CliError> {
    if !config_path.exists() {
        info!("Config file {:?} not found, using default.", config_path);
        return Ok(MutantCliConfig::default());
    }
    let content = fs::read_to_string(config_path)
        .map_err(|e| CliError::ConfigRead(e, config_path.to_path_buf()))?;
    serde_json::from_str(&content).map_err(|e| CliError::ConfigParse(e, config_path.to_path_buf()))
}

fn save_config(config_path: &Path, config: &MutantCliConfig) -> Result<(), CliError> {
    let content = serde_json::to_string_pretty(config)
        .map_err(|e| CliError::ConfigParse(e, config_path.to_path_buf()))?;
    fs::write(config_path, content).map_err(|e| CliError::ConfigWrite(e, config_path.to_path_buf()))
}

fn get_autonomi_wallet_dir() -> Result<PathBuf, CliError> {
    let base_dirs = BaseDirs::new().ok_or(CliError::WalletDirNotFound)?;
    let data_dir = base_dirs.data_dir();
    let wallet_dir = data_dir.join("autonomi/client/wallets");
    if wallet_dir.is_dir() {
        Ok(wallet_dir)
    } else {
        warn!(
            "Standard Autonomi wallet directory not found at {:?}",
            wallet_dir
        );
        Err(CliError::WalletDirNotFound)
    }
}

fn scan_wallet_dir(wallet_dir: &Path) -> Result<Vec<PathBuf>, CliError> {
    let entries = fs::read_dir(wallet_dir)
        .map_err(|e| CliError::WalletDirRead(e, wallet_dir.to_path_buf()))?;
    let mut wallets = Vec::new();
    for entry_result in entries {
        let entry =
            entry_result.map_err(|e| CliError::WalletDirRead(e, wallet_dir.to_path_buf()))?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("0x") && name.len() > 40 {
                    wallets.push(path);
                }
            }
        }
    }
    if wallets.is_empty() {
        Err(CliError::NoWalletsFound(wallet_dir.to_path_buf()))
    } else {
        Ok(wallets)
    }
}

fn prompt_user_for_wallet(wallets: &[PathBuf]) -> Result<PathBuf, CliError> {
    if wallets.is_empty() {
        return Err(CliError::WalletNotSet);
    }
    if wallets.len() == 1 {
        info!("Only one wallet found, using it: {:?}", wallets[0]);
        return Ok(wallets[0].clone());
    }

    let items: Vec<String> = wallets
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();

    info!("Multiple wallets found. Please select one to use:");
    let selection = Select::with_theme(&ColorfulTheme::default())
        .items(&items)
        .default(0)
        .interact_opt()
        .map_err(CliError::UserSelectionFailed)?;

    match selection {
        Some(index) => Ok(wallets[index].clone()),
        None => {
            error!("No wallet selected by the user.");
            Err(CliError::WalletNotSet)
        }
    }
}

async fn initialize_wallet() -> Result<String, CliError> {
    let config_path = get_config_path()?;
    let mut config = load_config(&config_path)?;

    let wallet_path = if let Some(ref path) = config.wallet_path {
        if path.exists() {
            info!("Using wallet from config: {:?}", path);
            path.clone()
        } else {
            warn!(
                "Wallet path from config {:?} does not exist. Rescanning.",
                path
            );
            config.wallet_path = None;
            info!("No valid wallet in config, scanning Autonomi wallet directory...");
            let wallet_dir = get_autonomi_wallet_dir()?;
            let available_wallets = scan_wallet_dir(&wallet_dir)?;
            let selected_wallet = prompt_user_for_wallet(&available_wallets)?;
            info!("Selected wallet: {:?}", selected_wallet);
            config.wallet_path = Some(selected_wallet.clone());
            save_config(&config_path, &config)?;
            info!("Saved selected wallet path to config: {:?}", config_path);
            selected_wallet
        }
    } else {
        info!("No valid wallet in config, scanning Autonomi wallet directory...");
        let wallet_dir = get_autonomi_wallet_dir()?;
        let available_wallets = scan_wallet_dir(&wallet_dir)?;
        let selected_wallet = prompt_user_for_wallet(&available_wallets)?;
        info!("Selected wallet: {:?}", selected_wallet);
        config.wallet_path = Some(selected_wallet.clone());
        save_config(&config_path, &config)?;
        info!("Saved selected wallet path to config: {:?}", config_path);
        selected_wallet
    };

    let private_key_hex = {
        let content = fs::read_to_string(&wallet_path)
            .map_err(|e| CliError::WalletRead(e, wallet_path.clone()))?;
        debug!("Raw content read from wallet file: '{}'", content.trim());
        content.trim().to_string()
    };
    debug!("Using private key hex from file: '{}'", private_key_hex);

    Ok(private_key_hex)
}

async fn cleanup_background_tasks(
    mp_join_handle: JoinHandle<()>,
    mutant_init_handle: Option<JoinHandle<()>>,
) {
    if !mp_join_handle.is_finished() {
        debug!("Aborting and awaiting MultiProgress drawing task...");
        mp_join_handle.abort();
        if let Err(e) = mp_join_handle.await {
            if !e.is_cancelled() {
                error!("MultiProgress join handle error after abort: {}", e);
            }
        }
        debug!("MultiProgress drawing task finished.");
    }

    if let Some(handle) = mutant_init_handle {
        info!("Waiting for background MutAnt/Storage task to complete...");
        match handle.await {
            Ok(_) => {
                info!("Background MutAnt/Storage task finished successfully.");
            }
            Err(e) => {
                if e.is_panic() {
                    error!("Background MutAnt/Storage task panicked: {}", e);
                } else if e.is_cancelled() {
                    info!("Background MutAnt/Storage task was cancelled.");
                } else {
                    error!("Background MutAnt/Storage task failed to join: {}", e);
                }
            }
        }
    }
}

pub async fn run_cli() -> Result<ExitCode, CliError> {
    info!("MutAnt CLI started processing.");

    let cli = Cli::parse();
    debug!("Parsed CLI arguments: {:?}", cli);

    let private_key_hex = if cli.local {
        info!("Using hardcoded local/devnet secret key for testing.");
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string()
    } else {
        initialize_wallet().await?
    };
    debug!("Wallet initialization complete or local key used.");

    let multi_progress = MultiProgress::new();
    let draw_target = if cli.quiet {
        ProgressDrawTarget::hidden()
    } else {
        ProgressDrawTarget::stderr()
    };
    multi_progress.set_draw_target(draw_target);

    let (init_pb_opt_arc, init_cb) = create_init_callback(&multi_progress, cli.quiet);

    let network_choice = if cli.local {
        NetworkChoice::Devnet
    } else {
        NetworkChoice::Mainnet
    };

    let mut config = MutAntConfig::default();
    config.network = network_choice;

    let mutant = MutAnt::init_with_progress(private_key_hex.clone(), config, None).await?;

    let mut pb_to_finish = init_pb_opt_arc.lock().await;

    if let Some(pb) = pb_to_finish.as_mut() {
        if !pb.is_finished() {
            debug!("Clearing initialization progress bar.");
            pb.finish_and_clear();
        }
    }

    debug!("MutAnt core initialized.");

    let mp_join_handle = {
        let mp_clone = multi_progress.clone();
        tokio::spawn(async move {
            let _keep_alive = mp_clone;
            std::future::pending::<()>().await;
        })
    };

    let exit_code = match cli.command {
        Commands::Put {
            key,
            value,
            force,
            public,
        } => {
            crate::commands::put::handle_put(
                mutant,
                key,
                value,
                force,
                public,
                &multi_progress,
                cli.quiet,
            )
            .await
        }
        Commands::Get { key, public } => {
            crate::commands::get::handle_get(mutant, key, public, &multi_progress, cli.quiet).await
        }
        Commands::Rm { key } => crate::commands::remove::handle_rm(mutant, key).await,
        Commands::Ls { long } => crate::commands::ls::handle_ls(mutant, long).await,
        Commands::Stats => crate::commands::stats::handle_stats(mutant).await,
        Commands::Reset => crate::commands::reset::handle_reset(mutant).await,
        Commands::Import { private_key } => {
            crate::commands::import::handle_import(mutant, private_key).await
        }
        Commands::Sync { push_force } => {
            match crate::commands::sync::handle_sync(mutant, push_force).await {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    error!("Sync command failed: {}", e);
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Purge => {
            match crate::commands::purge::run(
                crate::commands::purge::PurgeArgs {},
                mutant,
                &multi_progress,
                cli.quiet,
            )
            .await
            {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    error!("Purge command failed: {}", e);
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Reserve(reserve_cmd) => {
            info!("Executing Reserve command...");
            match reserve_cmd.run(&mutant, &multi_progress).await {
                Ok(_) => {
                    info!("Reserve command completed successfully.");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    error!("Reserve command failed: {}", e);
                    ExitCode::FAILURE
                }
            }
        }
    };

    cleanup_background_tasks(mp_join_handle, None).await;

    debug!("CLI exiting with code: {:?}", exit_code);
    Ok(exit_code)
}
