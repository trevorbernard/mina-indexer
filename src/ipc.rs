use crate::{
    block::{
        self, precomputed::PrecomputedBlockWithCanonicity, store::BlockStore, Block, BlockHash,
        BlockWithoutHeight,
    },
    canonicity::store::CanonicityStore,
    command::{signed, store::CommandStore, Command},
    ledger::{self, public_key, store::LedgerStore},
    server::{remove_domain_socket, IndexerConfiguration, IpcChannelUpdate},
    snark_work::store::SnarkStore,
    state::summary::{SummaryShort, SummaryVerbose},
    store::IndexerStore,
};
use futures_util::{io::BufReader, AsyncBufReadExt, AsyncWriteExt};
use interprocess::local_socket::tokio::{LocalSocketListener, LocalSocketStream};
use std::{path::PathBuf, process, sync::Arc};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, instrument, trace, warn};

#[derive(Debug)]
pub struct IpcActor {
    state_recv: IpcStateReceiver,
    listener: LocalSocketListener,
    best_tip: RwLock<Block>,
    summary: RwLock<Option<SummaryVerbose>>,
    store: RwLock<Arc<IndexerStore>>,
}

type IpcStateReceiver = mpsc::Receiver<IpcChannelUpdate>;

impl IpcActor {
    #[instrument(skip_all)]
    pub fn new(
        config: IndexerConfiguration,
        listener: LocalSocketListener,
        store: Arc<IndexerStore>,
        state_recv: IpcStateReceiver,
    ) -> Self {
        debug!("Creating new IPC actor");
        Self {
            state_recv,
            listener,
            best_tip: RwLock::new(Block {
                parent_hash: config.genesis_hash.clone(),
                state_hash: config.genesis_hash,
                height: 1,
                blockchain_length: 1,
                global_slot_since_genesis: 0,
            }),
            summary: RwLock::new(None),
            store: RwLock::new(store),
        }
    }

    #[instrument(skip(self))]
    pub async fn run(&mut self) -> () {
        loop {
            tokio::select! {
                state = self.state_recv.recv() => {
                    debug!("Received IPC state update");
                    match state {
                        None => panic!("IPC channel closed"),
                        Some(state) => {
                            debug!("Setting IPC state");
                            *self.best_tip.write().await = state.best_tip;
                            *self.summary.write().await = Some(*state.summary);
                            *self.store.write().await = state.store;
                        },
                    }
                }

                client = self.listener.accept() => {
                    let store = self.store.read().await.clone();
                    let best_tip = self.best_tip.read().await.clone();
                    let summary = self.summary.read().await.clone();
                    match client {
                        Err(e) => error!("Error accepting connection: {}", e.to_string()),
                        Ok(stream) => {
                            info!("Accepted client connection");
                            tokio::spawn(async move {
                                debug!("Handling client connection");
                                match handle_conn(stream,
                                    &store,
                                    &best_tip,
                                    summary.as_ref()
                                ).await {
                                    Err(e) => {
                                        error!("Error handling connection: {e}");
                                    },
                                    Ok(_) => { info!("handled connection"); },
                                };
                                debug!("Removing readonly instance at {}", store.db_path.clone().display());
                                tokio::fs::remove_dir_all(&store.db_path).await.ok();
                            });
                        }
                    }
                }
            }
        }
    }
}

#[instrument(skip_all)]
async fn handle_conn(
    conn: LocalSocketStream,
    db: &IndexerStore,
    best_tip: &Block,
    summary: Option<&SummaryVerbose>,
) -> Result<(), anyhow::Error> {
    use anyhow::anyhow;
    use helpers::*;

    let (reader, mut writer) = conn.into_split();
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::with_capacity(1024);
    let read_size = reader.read_until(0, &mut buffer).await?;

    if read_size == 0 {
        return Err(anyhow!("Unexpected EOF"));
    }

    let mut buffers = buffer.split(|byte| *byte == b' ');
    let command = buffers.next().unwrap();
    let command_string = String::from_utf8(command.to_vec()).unwrap();
    let response_json = match command_string.as_str() {
        "account" => {
            let pk_buffer = buffers.next().unwrap();
            let pk = String::from_utf8(pk_buffer.to_vec())?;
            let pk = pk.trim_end_matches('\0');
            info!("Received account command for {pk}");

            if let Some(ledger) = db.get_ledger_state_hash(&best_tip.state_hash)? {
                if !public_key::is_valid(pk) {
                    invalid_public_key(pk)
                } else {
                    let pk = pk.into();
                    let account = ledger.accounts.get(&pk);
                    if let Some(account) = account {
                        info!("Writing account {pk} to client");
                        Some(format!("{account}"))
                    } else {
                        warn!("Account {pk} does not exist");
                        Some(format!("Account {pk} does not exist"))
                    }
                }
            } else {
                error!(
                    "Best ledger not in database (length {}): {}",
                    best_tip.blockchain_length, best_tip.state_hash.0
                );
                Some(format!(
                    "Best ledger not in database (length {}): {}",
                    best_tip.blockchain_length, best_tip.state_hash.0
                ))
            }
        }
        "block-best-tip" => {
            info!("Received best-tip command");
            let verbose: bool = String::from_utf8(buffers.next().unwrap().to_vec())?.parse()?;
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');

            if let Ok(Some(ref block)) = db.get_block(&best_tip.state_hash) {
                let block_str = if let Some(canonicity) =
                    db.get_block_canonicity(&block.state_hash.clone().into(), &best_tip.state_hash)?
                {
                    if verbose {
                        serde_json::to_string_pretty(&block.with_canonicity(canonicity))?
                    } else {
                        let block = BlockWithoutHeight::with_canonicity(block, canonicity);
                        serde_json::to_string_pretty(&block)?
                    }
                } else {
                    block_missing_from_db(&block.state_hash)
                };
                if !path.is_empty() {
                    info!("Writing best tip block to {path}");
                    std::fs::write(path, block_str)?;
                    Some(format!("Best block written to {path}"))
                } else {
                    info!("Writing best tip block to stdout");
                    Some(block_str)
                }
            } else {
                error!(
                    "Best tip block is not in the store (length {}): {}",
                    best_tip.blockchain_length, best_tip.state_hash.0
                );
                Some(format!(
                    "Best tip block is not in the store (length {}): {}",
                    best_tip.blockchain_length, best_tip.state_hash.0
                ))
            }
        }
        "block-state-hash" => {
            info!("Received block-state-hash command");
            let state_hash = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let verbose: bool = String::from_utf8(buffers.next().unwrap().to_vec())?.parse()?;
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');

            if !block::is_valid_state_hash(&state_hash) {
                invalid_state_hash(&state_hash)
            } else if let Ok(Some(ref block)) = db.get_block(&state_hash.clone().into()) {
                let block_str = if let Some(canonicity) =
                    db.get_block_canonicity(&block.state_hash.clone().into(), &best_tip.state_hash)?
                {
                    if verbose {
                        serde_json::to_string_pretty(&block.with_canonicity(canonicity))?
                    } else {
                        let block = BlockWithoutHeight::with_canonicity(block, canonicity);
                        serde_json::to_string_pretty(&block)?
                    }
                } else {
                    block_missing_from_db(&block.state_hash)
                };
                if !path.is_empty() {
                    info!("Writing block {state_hash} to {path}");
                    std::fs::write(path, block_str)?;
                    Some(format!("Block {} written to {path}", block.state_hash))
                } else {
                    info!(
                        "Writing block to stdout (length {}): {}",
                        block.blockchain_length, block.state_hash
                    );
                    Some(block_str)
                }
            } else {
                error!("Block at state hash not present in store: {}", state_hash);
                Some(format!(
                    "Block at state hash not present in store: {}",
                    state_hash
                ))
            }
        }
        "best-chain" => {
            info!("Received best-chain command");
            let num = String::from_utf8(buffers.next().unwrap().to_vec())?.parse::<u32>()?;
            let verbose = String::from_utf8(buffers.next().unwrap().to_vec())?.parse()?;
            let start_state_hash: BlockHash =
                String::from_utf8(buffers.next().unwrap().to_vec())?.into();
            let end_state_hash = {
                let hash = String::from_utf8(buffers.next().unwrap().to_vec())?;
                let hash = hash.trim_end_matches('\0');
                if !block::is_valid_state_hash(hash) {
                    best_tip.state_hash.clone()
                } else {
                    hash.into()
                }
            };
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');

            if !block::is_valid_state_hash(&start_state_hash.0) {
                invalid_state_hash(&start_state_hash.0)
            } else if let (Some(end_block), Some(start_block)) = (
                db.get_block(&end_state_hash)?,
                db.get_block(&start_state_hash)?,
            ) {
                let start_height = start_block.blockchain_length;
                let end_height = end_block.blockchain_length;
                let mut parent_hash = end_block.previous_state_hash();
                let mut best_chain = vec![end_block];

                // constrain by num and state hash bound
                for _ in 1..num.min(end_height.saturating_sub(start_height) + 1) {
                    if let Some(parent_pcb) = db.get_block(&parent_hash)? {
                        let curr_hash: BlockHash = parent_pcb.state_hash.clone().into();
                        parent_hash = parent_pcb.previous_state_hash();
                        best_chain.push(parent_pcb);

                        if curr_hash == start_state_hash {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                let best_chain_str = if verbose {
                    let best_chain: Vec<PrecomputedBlockWithCanonicity> = best_chain
                        .iter()
                        .flat_map(|block| {
                            if let Ok(Some(canonicity)) = db.get_block_canonicity(
                                &block.state_hash.clone().into(),
                                &best_tip.state_hash,
                            ) {
                                Some(block.with_canonicity(canonicity))
                            } else {
                                None
                            }
                        })
                        .collect();
                    serde_json::to_string(&best_chain)?
                } else {
                    let best_chain: Vec<BlockWithoutHeight> = best_chain
                        .iter()
                        .flat_map(|block| {
                            if let Ok(Some(canonicity)) = db.get_block_canonicity(
                                &block.state_hash.clone().into(),
                                &best_tip.state_hash,
                            ) {
                                Some(BlockWithoutHeight::with_canonicity(block, canonicity))
                            } else {
                                None
                            }
                        })
                        .collect();
                    format_vec_jq_compatible(&best_chain)
                };

                if path.is_empty() {
                    info!("Writing best chain to stdout");
                    Some(best_chain_str)
                } else {
                    let path: PathBuf = path.into();
                    if !path.is_dir() {
                        info!("Writing best chain to {}", path.display());

                        std::fs::write(path.clone(), best_chain_str)?;
                        Some(format!("Best chain written to {}", path.display()))
                    } else {
                        file_must_not_be_a_directory(&path)
                    }
                }
            } else {
                None
            }
        }
        "checkpoint" => {
            info!("Received checkpoint command");
            let path: PathBuf = String::from_utf8(buffers.next().unwrap().to_vec())?
                .trim_end_matches('\0')
                .parse()?;
            if path.exists() {
                error!("Checkpoint directory already exists at {}", path.display());
                Some(format!(
                    "Checkpoint directory already exists at {}",
                    path.display()
                ))
            } else {
                debug!("Creating checkpoint at {}", path.display());
                db.create_checkpoint(&path)?;
                Some(format!(
                    "Checkpoint created and saved to {}",
                    path.display()
                ))
            }
        }
        "best-ledger" => {
            info!("Received best-ledger command");

            if let Some(ledger) = db.get_ledger_state_hash(&best_tip.state_hash)? {
                let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
                let path = path.trim_end_matches('\0');
                let ledger = ledger.to_string_pretty();

                if path.is_empty() {
                    debug!("Writing best ledger to stdout");
                    Some(ledger)
                } else {
                    let path = path.parse::<PathBuf>()?;
                    if path.is_dir() {
                        file_must_not_be_a_directory(&path)
                    } else {
                        debug!("Writing best ledger to {}", path.display());

                        std::fs::write(path.clone(), ledger)?;
                        Some(format!("Best ledger written to {}", path.display()))
                    }
                }
            } else {
                error!(
                    "Best ledger cannot be calculated (length {}): {}",
                    best_tip.blockchain_length, best_tip.state_hash.0
                );
                Some(format!(
                    "Best ledger cannot be calculated (length {}): {}",
                    best_tip.blockchain_length, best_tip.state_hash.0
                ))
            }
        }
        "ledger" => {
            let hash = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');
            info!("Received ledger command for {hash}");

            // check if ledger or state hash and use appropriate getter
            if block::is_valid_state_hash(&hash) {
                trace!("{hash} is a state hash");

                if let Some(ledger) = db.get_ledger_state_hash(&hash.clone().into())? {
                    let ledger = ledger.to_string_pretty();
                    if path.is_empty() {
                        debug!("Writing ledger at state hash {hash} to stdout");
                        Some(ledger)
                    } else {
                        let path: PathBuf = path.into();
                        if !path.is_dir() {
                            debug!("Writing ledger at {hash} to {}", path.display());

                            std::fs::write(path.clone(), ledger)?;
                            Some(format!(
                                "Ledger at state hash {hash} written to {}",
                                path.display()
                            ))
                        } else {
                            file_must_not_be_a_directory(&path)
                        }
                    }
                } else {
                    error!("Ledger at state hash {hash} is not in the store");
                    Some(format!("Ledger at state hash {hash} is not in the store"))
                }
            } else if ledger::is_valid_hash(&hash) {
                trace!("{hash} is a ledger hash");

                if let Some(ledger) = db.get_ledger(&hash)? {
                    let ledger = ledger.to_string_pretty();
                    match buffers.next() {
                        None => {
                            debug!("Writing ledger at {hash} to stdout");
                            Some(ledger)
                        }
                        Some(path_buffer) => {
                            let path = String::from_utf8(path_buffer.to_vec())?
                                .trim_end_matches('\0')
                                .parse::<PathBuf>()?;
                            if !path.is_dir() {
                                debug!("Writing ledger at {hash} to {}", path.display());

                                std::fs::write(path.clone(), ledger)?;
                                Some(format!("Ledger at {hash} written to {}", path.display()))
                            } else {
                                file_must_not_be_a_directory(&path)
                            }
                        }
                    }
                } else {
                    error!("Ledger at {hash} is not in the store");
                    Some(format!("Ledger at {hash} is not in the store"))
                }
            } else {
                error!("Invalid ledger or state hash: {hash}");
                Some(format!("Invalid ledger or state hash: {hash}"))
            }
        }
        "ledger-at-height" => {
            let height = String::from_utf8(buffers.next().unwrap().to_vec())?.parse::<u32>()?;
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');
            info!("Received ledger-at-height {height} command");

            if height > best_tip.blockchain_length {
                // ahead of witness tree - cannot compute
                Some(format!("Invalid query: ledger at height {height} cannot be determined from a chain of length {}", best_tip.blockchain_length))
            } else {
                let ledger_str = if Some(height) > db.get_max_canonical_blockchain_length()? {
                    // follow best chain back from tip to given height block and get the ledger
                    if let Some(mut curr_block) = db.get_block(&best_tip.state_hash)? {
                        while curr_block.blockchain_length > height {
                            if let Some(parent) = db.get_block(&curr_block.previous_state_hash())? {
                                curr_block = parent;
                            } else {
                                break;
                            }
                        }

                        let state_hash = curr_block.state_hash.clone().into();
                        if let Some(ledger) = db.get_ledger_state_hash(&state_hash)? {
                            ledger.to_string_pretty()
                        } else {
                            block_missing_from_db(&curr_block.state_hash)
                        }
                    } else {
                        best_tip_missing_from_db()
                    }
                } else if let Some(ledger) = db.get_ledger_at_height(height)? {
                    ledger.to_string_pretty()
                } else {
                    error!("Invalid ledger query. Ledger at height {height} not available");
                    format!("Invalid ledger query. Ledger at height {height} not available")
                };

                if path.is_empty() {
                    debug!("Writing ledger at height {height} to stdout");
                    Some(ledger_str)
                } else {
                    let path: PathBuf = path.into();
                    if !path.is_dir() {
                        debug!("Writing ledger at height {height} to {}", path.display());

                        std::fs::write(&path, ledger_str)?;
                        Some(format!(
                            "Ledger at height {height} written to {}",
                            path.display()
                        ))
                    } else {
                        file_must_not_be_a_directory(&path)
                    }
                }
            }
        }
        "snark-pk" => {
            let pk = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');
            info!("Received SNARK work command for public key {pk}");

            if !public_key::is_valid(&pk) {
                invalid_public_key(&pk)
            } else {
                let snarks = db
                    .get_snark_work_by_public_key(&pk.clone().into())?
                    .unwrap_or(vec![]);
                let snarks_str = format_vec_jq_compatible(&snarks);

                if path.is_empty() {
                    debug!("Writing SNARK work for public key {pk} to stdout");
                    Some(snarks_str)
                } else {
                    let path: PathBuf = path.into();
                    if !path.is_dir() {
                        debug!(
                            "Writing SNARK work for public key {pk} to {}",
                            path.display()
                        );

                        std::fs::write(&path, snarks_str)?;
                        Some(format!(
                            "SNARK work for public key {pk} written to {}",
                            path.display()
                        ))
                    } else {
                        file_must_not_be_a_directory(&path)
                    }
                }
            }
        }
        "snark-state-hash" => {
            let state_hash = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');
            info!("Received SNARK work command for state hash {state_hash}");

            if !block::is_valid_state_hash(&state_hash) {
                invalid_state_hash(&state_hash)
            } else {
                db.get_snark_work_in_block(&state_hash.clone().into())?
                    .and_then(|snarks| {
                        let snarks_str = format_vec_jq_compatible(&snarks);
                        if path.is_empty() {
                            debug!("Writing SNARK work for block {state_hash} to stdout");
                            Some(snarks_str)
                        } else {
                            let path: PathBuf = path.into();
                            if !path.is_dir() {
                                debug!(
                                    "Writing SNARK work for block {state_hash} to {}",
                                    path.display()
                                );

                                std::fs::write(&path, snarks_str).unwrap();
                                Some(format!(
                                    "SNARK work for block {state_hash} written to {}",
                                    path.display()
                                ))
                            } else {
                                file_must_not_be_a_directory(&path)
                            }
                        }
                    })
            }
        }
        "shutdown" => {
            info!("Received shutdown command");
            writer
                .write_all(b"Shutting down the Mina Indexer daemon...")
                .await?;

            remove_domain_socket().unwrap_or(());
            process::exit(0);
        }
        "summary" => {
            info!("Received summary command");
            let verbose = String::from_utf8(buffers.next().unwrap().to_vec())?.parse::<bool>()?;
            let json = String::from_utf8(buffers.next().unwrap().to_vec())?.parse::<bool>()?;
            let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let path = path.trim_end_matches('\0');

            if let Some(summary) = summary {
                let summary_str = if verbose {
                    format_json(&summary, json)
                } else {
                    let summary: SummaryShort = summary.clone().into();
                    format_json(&summary, json)
                };

                if path.is_empty() {
                    info!("Writing summary to stdout");
                    Some(summary_str)
                } else {
                    let path: PathBuf = path.into();
                    if !path.is_dir() {
                        info!("Writing summary to {}", path.display());

                        std::fs::write(&path, summary_str).unwrap();
                        Some(format!("Summary written to {}", path.display()))
                    } else {
                        file_must_not_be_a_directory(&path)
                    }
                }
            } else {
                Some("Summary not available".into())
            }
        }
        "tx-pk" => {
            let pk = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let verbose = String::from_utf8(buffers.next().unwrap().to_vec())?.parse::<bool>()?;
            let start_state_hash: BlockHash =
                String::from_utf8(buffers.next().unwrap().to_vec())?.into();
            let end_state_hash: BlockHash = {
                let raw = String::from_utf8(buffers.next().unwrap().to_vec())?;
                if &raw == "x" {
                    // dummy value
                    best_tip.state_hash.clone()
                } else {
                    raw.into()
                }
            };
            info!("Received tx-public-key command for {pk}");

            if !public_key::is_valid(&pk) {
                invalid_public_key(&pk)
            } else if !block::is_valid_state_hash(&start_state_hash.0) {
                invalid_state_hash(&start_state_hash.0)
            } else if !block::is_valid_state_hash(&end_state_hash.0) {
                invalid_state_hash(&end_state_hash.0)
            } else {
                let path = String::from_utf8(buffers.next().unwrap().to_vec())?;
                let path = path.trim_end_matches('\0');
                let transactions = db
                    .get_commands_for_public_key(&pk.clone().into())?
                    .unwrap_or(vec![]);
                let transaction_str = if verbose {
                    format_vec_jq_compatible(&transactions)
                } else {
                    let txs: Vec<Command> = transactions.into_iter().map(Command::from).collect();
                    format_vec_jq_compatible(&txs)
                };

                if path.is_empty() {
                    debug!("Writing transactions for {pk} to stdout");
                    Some(transaction_str)
                } else {
                    let path: PathBuf = path.into();
                    if !path.is_dir() {
                        debug!("Writing transactions for {pk} to {}", path.display());

                        std::fs::write(&path, transaction_str)?;
                        Some(format!(
                            "Transactions for {pk} written to {}",
                            path.display()
                        ))
                    } else {
                        file_must_not_be_a_directory(&path)
                    }
                }
            }
        }
        "tx-hash" => {
            let tx_hash = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let verbose = String::from_utf8(buffers.next().unwrap().to_vec())?
                .trim_end_matches('\0')
                .parse()?;

            info!("Received tx-hash command for {tx_hash}");
            if !signed::is_valid_tx_hash(&tx_hash) {
                invalid_tx_hash(&tx_hash)
            } else {
                db.get_command_by_hash(&tx_hash)?.map(|cmd| {
                    if verbose {
                        format!("{cmd:?}")
                    } else {
                        let cmd: Command = cmd.command.into();
                        format!("{cmd:?}")
                    }
                })
            }
        }
        "tx-state-hash" => {
            let state_hash = String::from_utf8(buffers.next().unwrap().to_vec())?;
            let verbose = String::from_utf8(buffers.next().unwrap().to_vec())?
                .trim_end_matches('\0')
                .parse()?;

            info!("Received tx-state-hash command for {state_hash}");
            if !block::is_valid_state_hash(&state_hash) {
                invalid_state_hash(&state_hash)
            } else {
                db.get_commands_in_block(&state_hash.into())?.map(|cmds| {
                    if verbose {
                        format_vec_jq_compatible(&cmds)
                    } else {
                        let cmds: Vec<Command> = cmds.into_iter().map(Command::from).collect();
                        format_vec_jq_compatible(&cmds)
                    }
                })
            }
        }
        bad_request => {
            return Err(anyhow!("Malformed request: {bad_request}"));
        }
    };

    if let Some(response_json) = response_json {
        writer.write_all(response_json.as_bytes()).await?;
    } else {
        writer
            .write_all(serde_json::to_string("no response 404")?.as_bytes())
            .await?;
    }

    Ok(())
}

fn file_must_not_be_a_directory(path: &std::path::Path) -> Option<String> {
    Some(format!(
        "The path provided must not be a directory: {}",
        path.display()
    ))
}

mod helpers {
    use super::*;

    pub fn invalid_public_key(input: &str) -> Option<String> {
        warn!("Invalid public key: {input}");
        Some(format!("Invalid public key: {input}"))
    }

    pub fn invalid_tx_hash(input: &str) -> Option<String> {
        warn!("Invalid transaction hash: {input}");
        Some(format!("Invalid transaction hash: {input}"))
    }

    pub fn invalid_state_hash(input: &str) -> Option<String> {
        warn!("Invalid state hash: {input}");
        Some(format!("Invalid state hash: {input}"))
    }

    pub fn block_missing_from_db(state_hash: &str) -> String {
        error!("Block missing from store: {state_hash}");
        format!("Block missing from store: {state_hash}")
    }

    pub fn best_tip_missing_from_db() -> String {
        error!("Best tip block missing from store");
        "Best tip block missing from store".to_string()
    }

    pub fn format_vec_jq_compatible<T>(vec: &Vec<T>) -> String
    where
        T: std::fmt::Debug,
    {
        let pp = format!("{vec:#?}");
        pp.replace(",\n]", "\n]")
    }

    pub fn format_json<T>(input: &T, json: bool) -> String
    where
        T: ?Sized + serde::Serialize + std::fmt::Display,
    {
        if json {
            serde_json::to_string(input).unwrap()
        } else {
            format!("{input}")
        }
    }
}
