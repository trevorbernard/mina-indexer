use anyhow::anyhow;
use crossbeam_channel::{bounded, Receiver};
use glob::glob;
use mina_indexer::block::precomputed::{BlockLogContents, PrecomputedBlock};
use mina_indexer::display_duration;
use mina_indexer::state::ledger::genesis::{parse_file, GenesisLedger, GenesisRoot};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};
use serde_derive::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let genesis_ledger_file = "/tmp/mainnet.json";
    let watch_dir = "/tmp/watch_dir";
    let blocks_dir = "/Users/tbernard/.mina-indexer/blocks/trevor";
    let db_dir = "/tmp/db";

    std::fs::create_dir_all(watch_dir)?;
    std::fs::create_dir_all(db_dir)?;
    std::fs::create_dir_all(blocks_dir)?;

    // Parse genesis ledger
    let root_ledger = match parse_genesis_ledger(genesis_ledger_file) {
        Ok(ledger) => ledger,
        Err(_) => {
            eprintln!("Unable to parse genesis ledger. Exiting...");
            std::process::exit(123);
        }
    };
    // Create a channels for thread communication
    let (ingestion_tx, ingestion_rx) = bounded(16384);
    let blocks_dir_ingestion_tx = ingestion_tx.clone();

    let (parser_tx, parser_rx) = bounded(16384);

    let total_time = Instant::now();

    thread::spawn(move || {
        let _ = block_ingestion_from_dir(Path::new(blocks_dir), blocks_dir_ingestion_tx);
    });
    thread::spawn(move || {
        let _ = block_ingestion(watch_dir, ingestion_tx);
    });
    thread::spawn(move || {
        let _ = block_parser(ingestion_rx, parser_tx);
    });
    thread::spawn(move || {
        let _ = block_persistence(Path::new(db_dir), parser_rx, total_time);
    });
    loop {}
}

/// Parse a genesis ledger file
fn parse_genesis_ledger<P: AsRef<Path>>(path: P) -> anyhow::Result<GenesisRoot> {
    parse_file(path)
}

/// Listen for SIGINT signals and notify the system to safely shutdown
fn ctrl_channel() -> anyhow::Result<Receiver<()>> {
    let (sender, receiver) = bounded(100);
    ctrlc::set_handler(move || {
        let _ = sender.send(());
    })?;

    Ok(receiver)
}

/// Checks to see if a file is a valid precomputed block
fn is_valid_block_file(path: &Path) -> bool {
    if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
        let parts: Vec<&str> = file_name.split('-').collect();
        match parts.len() {
            2 => parts[1].ends_with(".json") && parts[1].starts_with("3N"),
            3 => {
                parts[1].parse::<u64>().is_ok()
                    && parts[2].ends_with(".json")
                    && parts[2].starts_with("3N")
            }
            _ => false,
        }
    } else {
        false
    }
}

/// Take a directory of json precomputed blocks and returning then in ascending block height order
fn find_and_sort_json_blocks(blocks_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths: Vec<(u64, PathBuf)> = glob(&format!("{}/*.json", blocks_dir.display()))?
        .filter_map(|x| x.ok())
        .filter_map(|path| {
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                let parts: Vec<&str> = file_name.split('-').collect();
                if parts.len() == 3 && parts[2].ends_with(".json") {
                    parts[1]
                        .parse::<u64>()
                        .ok()
                        .map(|block_height| (block_height, path))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    paths.sort_by_key(|k| k.0);
    Ok(paths.into_iter().map(|(_, path)| path).collect())
}

/// Ingest blocks from directory
fn block_ingestion_from_dir(
    blocks_dir: &Path,
    sender: crossbeam_channel::Sender<PathBuf>,
) -> anyhow::Result<()> {
    let paths = find_and_sort_json_blocks(blocks_dir)?;
    for path in paths {
        if let Err(e) = sender.send(path) {
            eprintln!(
                "[block_ingestion_dir] Unable to send path downstream. {}",
                e
            );
        }
    }
    Ok(())
}

/// Listen for BlockAdded events to construct the best tip and best canonical chain
fn canonical_chain_discovery() -> anyhow::Result<()> {
    Ok(())
}

/// Block Ingestions Logic
fn block_ingestion<P: AsRef<Path>>(
    watch_dir: P,
    sender: crossbeam_channel::Sender<PathBuf>,
) -> notify::Result<()> {
    let (tx, rx) = bounded(4096);
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

    watcher.watch(watch_dir.as_ref(), RecursiveMode::NonRecursive)?;

    for res in rx {
        match res {
            Ok(event) => {
                if let EventKind::Create(notify::event::CreateKind::File) = event.kind {
                    for path in event.paths {
                        // println!("[block_ingestion] File Created Signal");
                        if is_valid_block_file(&path) {
                            // println!("[block_ingestion] valided precomputed block file");
                            if let Err(e) = sender.send(path) {
                                eprintln!("Unable to send path downstream. {}", e);
                            }
                        } else {
                            println!("[block_ingestion] Invalid block file: {}", path.display());
                        }
                    }
                }
            }
            Err(error) => println!("Error: {error:?}"),
        }
    }
    Ok(())
}

/// Block Parsing Logic worker
fn block_parser(
    receiver: crossbeam_channel::Receiver<PathBuf>,
    sender: crossbeam_channel::Sender<PrecomputedBlock>,
) -> anyhow::Result<()> {
    for path in receiver {
        // println!("[block_parser] Received path: {}", path.display());
        match parse_block_file(path.as_path()) {
            Ok(precomputed_block) => {
                // println!(
                //     "[block_parser] Parsed Precomputed Block: {}",
                //     precomputed_block.state_hash
                // );
                sender.send(precomputed_block)?;
            }
            Err(_) => {
                // TODO: Move tombstone to a place for manual inspection
                // println!(
                //     "[block_parser] Unable to parse: {}, skipping",
                //     path.display()
                // );
            }
        }
    }
    Ok(())
}

/// Block persistence worker
fn block_persistence(
    db_dir: &Path,
    receiver: crossbeam_channel::Receiver<PrecomputedBlock>,
    total_time: Instant, // TODO: Remove later
) -> anyhow::Result<()> {
    let mut blockchain = Blockchain::new(db_dir)?;

    let mut block_count = 0;
    let mut adding_time = Duration::new(0, 0);
    let add = Instant::now();
    for block in receiver {
        // println!(
        //     "[block_persistence] Received Precomputed Block: {}",
        //     block.state_hash
        // );
        match blockchain.add_block(&block) {
            Ok(_) => {
                block_count += 1;
                adding_time += add.elapsed();

                if block_count % 500 == 0 {
                    let display_elapsed: String = display_duration(total_time.elapsed());
                    println!("\n~~~ General ~~~");
                    println!("Blocks:  {block_count}");
                    println!("Total:   {display_elapsed}");

                    let blocks_per_sec = block_count as f64 / total_time.elapsed().as_secs_f64();
                    println!("\n~~~ Block stats ~~~");
                    println!("Per sec: {blocks_per_sec:?} blocks");
                    println!("Per hr:  {:?} blocks", blocks_per_sec * 3600.);
                }
                // println!(
                //     "[block_persistence] Persisted Precomputed Block: {}",
                //     block.state_hash
                // );
                // println!("[block_persistence] Signal canonical chain discovery of new block");
            }
            Err(e) => {
                println!(
                    "[block_persistence] Unable to persist precomputed block: {}, {}",
                    block.state_hash, e
                );
                std::process::exit(666);
            }
        };
    }
    Ok(())
}

/// Drop file extension from string
fn drop_extension(file_name: &str) -> String {
    file_name
        .rsplit_once('.')
        .map(|x| x.0)
        .unwrap_or(file_name)
        .to_string()
}

/// Parse precomputed block metadata from filename in the two supported patterns
fn extract_block_metadata(path: &Path) -> Option<(String, Option<u32>, String)> {
    if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
        let parts: Vec<&str> = file_name.split('-').collect();
        match parts.len() {
            2 => Some((parts[0].to_owned(), None, drop_extension(parts[1]))),
            3 => Some((
                parts[0].to_owned(),
                Some(parts[1].parse::<u32>().expect("should be u64")),
                drop_extension(parts[2]),
            )),
            _ => None,
        }
    } else {
        None
    }
}

/// Parses a mainnet precomputed block file
fn parse_block_file(path: &Path) -> anyhow::Result<PrecomputedBlock> {
    let (_, blockchain_length, state_hash) = extract_block_metadata(path).unwrap();
    let log_file_contents = std::fs::read(path)?;
    let precomputed_block = PrecomputedBlock::from_log_contents(BlockLogContents {
        state_hash,
        blockchain_length,
        contents: log_file_contents,
    })?;
    Ok(precomputed_block)
}

struct Blockchain {
    db: rocksdb::DB,
    path: PathBuf,
    journal_seq: AtomicU64,
}

/// BlockchainEvents
#[derive(Debug, Deserialize, Serialize)]
enum BlockchainEvents {
    /// Emitted when we receive a new Block
    BlockAdded(String),
    /// Emitted when the best known canonical chain has been extended by 1
    CanonicalChainExtended(String),
    /// Emitted when a short-term fork was identified
    CanonicalChainReorg(u64, Vec<String>),
}

/// Custom comparator so we can maintain sequential ordering of events
fn compare_journal_keys(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let left = u64::from_le_bytes(a.try_into().unwrap_or([0; 8]));
    let right = u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
    left.cmp(&right)
}

// TODO: add a unique key to this journal
const JOURNAL_SEQ_KEY: &str = "journal_seq_key";

fn load_journal_seq(db: &DB) -> Result<AtomicU64, rocksdb::Error> {
    match db.get(JOURNAL_SEQ_KEY)? {
        Some(bytes) => {
            let seq = AtomicU64::new(u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])));
            println!("Loading last processed seq id for journal: {:?}", seq);
            Ok(seq)
        }
        None => Ok(AtomicU64::new(0)),
    }
}

impl Blockchain {
    fn new(path: &Path) -> anyhow::Result<Self> {
        // Use a custom comparator for keys so events are inserted and returned in order
        let mut journal_opts = Options::default();
        journal_opts.set_comparator("journal", Box::new(compare_journal_keys));

        let cf_blocks = ColumnFamilyDescriptor::new("blocks", Options::default());
        let cf_journal = ColumnFamilyDescriptor::new("journal", journal_opts);

        let mut db_opts = rocksdb::Options::default();
        db_opts.create_missing_column_families(true);
        db_opts.create_if_missing(true);

        let db = rocksdb::DBWithThreadMode::open_cf_descriptors(
            &db_opts,
            path,
            vec![cf_blocks, cf_journal],
        )?;
        let journal_seq = load_journal_seq(&db).unwrap_or(AtomicU64::new(0));
        Ok(Self {
            path: PathBuf::from(path),
            db,
            journal_seq,
        })
    }

    /// Add a block to the blockchain
    fn add_block(&mut self, block: &PrecomputedBlock) -> anyhow::Result<()> {
        let mut batch = WriteBatch::default();

        let seq_num = self.journal_seq.fetch_add(1, Ordering::SeqCst);
        let seq_bytes = seq_num.to_le_bytes();

        let blocks_cf_handle = self
            .db
            .cf_handle("blocks")
            .ok_or_else(|| anyhow!("Unable to get blocks CF"))?;
        let journal_cf_handle = self
            .db
            .cf_handle("journal")
            .ok_or_else(|| anyhow!("Unable to get journal CF"))?;

        let block_key = block.state_hash.as_bytes();
        let state_hash = block.state_hash.clone();

        let block_data = bcs::to_bytes(&block)?;
        let journal_data = bcs::to_bytes(&BlockchainEvents::BlockAdded(state_hash))?;

        batch.put_cf(blocks_cf_handle, block_key, block_data);
        batch.put_cf(journal_cf_handle, seq_bytes, journal_data);
        batch.put(JOURNAL_SEQ_KEY, seq_bytes);

        self.db.write(batch).map_err(anyhow::Error::msg)
    }
}
