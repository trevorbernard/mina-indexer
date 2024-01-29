use crate::constants::{MAINNET_TRANSITION_FRONTIER_K, SOCKET_NAME};
use crate::servers::unix_domain_socket_server::UnixDomainSocketServer;
use crate::{
    block::{
        is_valid_block_file, parser::BlockParser, precomputed::PrecomputedBlock, BlockHash,
        BlockWithoutHeight,
    },
    ledger::genesis::GenesisRoot,
    state::IndexerState,
    store::IndexerStore,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::Arc,
    thread,
};

use crossbeam_channel::{bounded, Receiver};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct IndexerConfiguration {
    pub ledger: GenesisRoot,
    pub root_hash: BlockHash,
    pub startup_dir: PathBuf,
    pub watch_dir: PathBuf,
    pub prune_interval: u32,
    pub canonical_threshold: u32,
    pub canonical_update_threshold: u32,
    pub initialization_mode: InitializationMode,
    pub ledger_cadence: u32,
    pub database_dir: PathBuf,
}

pub struct MinaIndexer {
    config: IndexerConfiguration,
}

#[derive(Debug, Clone)]
pub enum InitializationMode {
    New,
    Replay,
    Sync,
}

impl MinaIndexer {
    pub fn new(config: IndexerConfiguration) -> Self {
        Self { config }
    }
}

#[instrument(skip_all)]
pub fn start(indexer: MinaIndexer) -> anyhow::Result<()> {
    info!("Starting Mina Indexer...");
    let config = indexer.config.clone();
    run(config);
    Ok(())
}

fn run(config: IndexerConfiguration) {
    let uds_config = config.clone();
    let database_dir = config.database_dir.clone();
    let block_watch_dir = config.watch_dir.clone();
    let (ingestion_tx, ingestion_rx) = bounded(16384);

    // RocksDB instances need to be created first before a secondary instance can be created
    let store = Arc::new(IndexerStore::new(&database_dir).unwrap());
    let state = initialize(config, store).unwrap();

    // Launch watch block directory thread
    let _ = thread::spawn(move || {
        let _ = watch_directory_for_blocks(block_watch_dir, ingestion_tx);
    });
    // Launch precomputed block deserializer and persistence thread
    let _ = thread::spawn(move || {
        let _ = block_persistence(state, ingestion_rx);
    });
    // Wait for signal
    let _ = tokio::spawn(async move {
        let _ = wait_for_signal().await;
    });
    // Launch Unix Domain Server
    let _ = tokio::spawn(async move {
        start_unix_domain_server(uds_config).await;
    });
}

#[instrument(skip_all)]
async fn start_unix_domain_server(config: IndexerConfiguration) {
    info!("Starting unix domain server...");
    let IndexerConfiguration {
        ledger,
        root_hash,
        prune_interval,
        canonical_update_threshold,
        ledger_cadence,
        database_dir,
        ..
    } = config;

    let primary_path = database_dir.clone();
    let mut secondary_path = primary_path.clone();
    secondary_path.push(Uuid::new_v4().to_string());

    // TODO: Ensure the IndexerStore exists before attempting to create a read only instance
    let store = Arc::new(IndexerStore::new_read_only(&primary_path, &secondary_path).unwrap());
    // create read only indexer state
    let foobar = IndexerState::new_without_genesis_events(
        &root_hash,
        ledger.ledger.clone(),
        store,
        MAINNET_TRANSITION_FRONTIER_K,
        prune_interval,
        canonical_update_threshold,
        ledger_cadence,
    );

    let state = match foobar {
        Ok(state) => state,
        Err(e) => panic!("OH SHIT: {:?}", e),
    };
    let socket_name = PathBuf::from(SOCKET_NAME);
    let server = UnixDomainSocketServer::new(socket_name, state);
    let _ = crate::servers::unix_domain_socket_server::start(server).await;
}
///
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("failed to register signal handler");
    let mut int = signal(SignalKind::interrupt()).expect("failed to register signal handler");
    tokio::select! {
        _ = term.recv() => {
            info!("Received SIGTERM");
            process::exit(100);
        },
        _ = int.recv() => {
            info!("Received SIGINT");
            process::exit(101);
        },
    }
}

/// Write Precomputed Block to disk
fn block_persistence(
    mut state: IndexerState,
    ingestion_rx: Receiver<PathBuf>,
) -> notify::Result<()> {
    info!("Starting block persisting thread..");

    for path_buf in ingestion_rx {
        let precomputed_block = PrecomputedBlock::parse_file(&path_buf.as_path()).unwrap();
        let block = BlockWithoutHeight::from_precomputed(&precomputed_block);
        debug!("Deserialized precomputed block {block:?}");
        state.add_block_to_witness_tree(&precomputed_block).unwrap();
    }
    Ok(())
}
/// Watches a directory listening for when valid precomputed blocks are created and signals downstream
fn watch_directory_for_blocks<P: AsRef<Path>>(
    watch_dir: P,
    sender: crossbeam_channel::Sender<PathBuf>,
) -> notify::Result<()> {
    info!("Starting block watcher thread..");
    let (tx, rx) = bounded(4096);
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

    watcher.watch(watch_dir.as_ref(), RecursiveMode::NonRecursive)?;
    info!(
        "Listening for blocks in directory: {:?}",
        watch_dir.as_ref()
    );
    for res in rx {
        match res {
            Ok(event) => {
                if let EventKind::Create(notify::event::CreateKind::File) = event.kind {
                    for path in event.paths {
                        if is_valid_block_file(&path) {
                            debug!("Valid precomputed block file");
                            if let Err(e) = sender.send(path) {
                                error!("Unable to send path downstream. {}", e);
                            }
                        } else {
                            warn!("Invalid precomputed block file: {}", path.display());
                        }
                    }
                }
            }
            Err(error) => error!("Error: {error:?}"),
        }
    }
    Ok(())
}

fn initialize(
    config: IndexerConfiguration,
    store: Arc<IndexerStore>,
) -> anyhow::Result<IndexerState> {
    info!("Starting mina-indexer server");
    let db_path = store.db_path.clone();
    let IndexerConfiguration {
        ledger,
        root_hash,
        startup_dir,
        prune_interval,
        canonical_threshold,
        canonical_update_threshold,
        initialization_mode,
        ledger_cadence,
        ..
    } = config;

    fs::create_dir_all(startup_dir.clone()).expect("startup_dir created");

    let state = {
        let mut state = match initialization_mode {
            InitializationMode::New => {
                info!(
                    "Initializing indexer state from blocks in {}",
                    startup_dir.display()
                );
                IndexerState::new(
                    &root_hash,
                    ledger.ledger.clone(),
                    store,
                    MAINNET_TRANSITION_FRONTIER_K,
                    prune_interval,
                    canonical_update_threshold,
                    ledger_cadence,
                )?
            }
            InitializationMode::Replay => {
                info!("Replaying indexer events from db at {}", db_path.display());
                IndexerState::new_without_genesis_events(
                    &root_hash,
                    ledger.ledger.clone(),
                    store,
                    MAINNET_TRANSITION_FRONTIER_K,
                    prune_interval,
                    canonical_update_threshold,
                    ledger_cadence,
                )?
            }
            InitializationMode::Sync => {
                info!("Syncing indexer state from db at {}", db_path.display());
                IndexerState::new_without_genesis_events(
                    &root_hash,
                    ledger.ledger.clone(),
                    store,
                    MAINNET_TRANSITION_FRONTIER_K,
                    prune_interval,
                    canonical_update_threshold,
                    ledger_cadence,
                )?
            }
        };

        debug!("Initialization mode: {:?}", initialization_mode);
        match initialization_mode {
            InitializationMode::New => {
                let mut block_parser = BlockParser::new(&startup_dir, canonical_threshold)?;
                state.initialize_with_canonical_chain_discovery(&mut block_parser)?;
            }
            InitializationMode::Replay => {
                state.replay_events()?;
            }
            InitializationMode::Sync => {
                state.sync_from_db()?;
            }
        }
        state
    };

    Ok(state)
}
