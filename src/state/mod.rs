use crate::{
    block::{
        genesis::GenesisBlock, parser::BlockParser, precomputed::PrecomputedBlock,
        store::BlockStore, Block, BlockHash, BlockWithoutHeight,
    },
    canonical::store::CanonicityStore,
    event::{block::*, db::*, ledger::*, store::*, witness_tree::*, IndexerEvent},
    state::{
        branch::Branch,
        ledger::{
            command::Command, diff::LedgerDiff, genesis::GenesisLedger, store::LedgerStore, Ledger,
        },
        summary::{
            DbStats, SummaryShort, SummaryVerbose, WitnessTreeSummaryShort,
            WitnessTreeSummaryVerbose,
        },
    },
    store::IndexerStore,
    BLOCK_REPORTING_FREQ_NUM, BLOCK_REPORTING_FREQ_SEC, CANONICAL_UPDATE_THRESHOLD,
    MAINNET_CANONICAL_THRESHOLD, MAINNET_TRANSITION_FRONTIER_K, PRUNE_INTERVAL_DEFAULT,
};
use anyhow::anyhow;
use id_tree::NodeId;
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, info, instrument, trace};
use uuid::Uuid;

pub mod branch;
pub mod ledger;
pub mod summary;

/// Rooted forest of precomputed block summaries aka the witness tree
/// `root_branch` - represents the tree of blocks connecting back to a known ledger state, e.g. genesis
/// `dangling_branches` - trees of blocks stemming from an unknown ledger state
#[derive(Debug)]
pub struct IndexerState {
    /// Indexer phase
    pub phase: IndexerPhase,
    /// Block representing the best tip of the root branch
    pub best_tip: Tip,
    /// Highest known canonical block
    pub canonical_tip: Tip,
    /// Map of ledger diffs following the canonical tip
    pub diffs_map: HashMap<BlockHash, LedgerDiff>,
    /// Append-only tree of blocks built from genesis, each containing a ledger
    pub root_branch: Branch,
    /// Dynamic, dangling branches eventually merged into the `root_branch`
    /// needed for the possibility of missing blocks
    pub dangling_branches: Vec<Branch>,
    /// Block database
    pub indexer_store: Option<Arc<IndexerStore>>,
    /// Threshold amount of confirmations to trigger a pruning event
    pub transition_frontier_length: u32,
    /// Interval to the prune the root branch
    pub prune_interval: u32,
    /// Threshold for updating the canonical tip and db ledger
    pub canonical_update_threshold: u32,
    /// Number of blocks added to the state
    pub blocks_processed: u32,
    /// Datetime the indexer started running
    pub init_time: Instant,
}

#[derive(Debug, Clone)]
pub struct Tip {
    pub state_hash: BlockHash,
    pub node_id: NodeId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexerPhase {
    InitializingFromBlockDir,
    SyncingFromDB,
    Replaying,
    Watching,
    Testing,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExtensionType {
    DanglingNew,
    DanglingSimpleForward,
    DanglingSimpleReverse,
    DanglingComplex,
    RootSimple,
    RootComplex,
    BlockNotAdded,
}

pub enum ExtensionDirection {
    Forward,
    Reverse,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum Canonicity {
    Canonical,
    Orphaned,
    Pending,
}

impl IndexerState {
    /// Creates a new indexer state from the genesis ledger
    pub fn new(
        root_hash: &BlockHash,
        genesis_ledger: GenesisLedger,
        indexer_store: Arc<IndexerStore>,
        transition_frontier_length: u32,
        prune_interval: u32,
        canonical_update_threshold: u32,
    ) -> anyhow::Result<Self> {
        let root_branch = Branch::new_genesis(root_hash)?;

        // add genesis block and ledger to indexer store
        let genesis_block = GenesisBlock::new()?.into();
        indexer_store.add_block(&genesis_block)?;
        info!("Genesis block added to indexer store");

        indexer_store
            .add_ledger(root_hash, genesis_ledger.into())
            .expect("add genesis ledger succeeds");
        info!("Genesis ledger added to indexer store");

        // update genesis canonicity
        indexer_store
            .set_max_canonical_blockchain_length(1)
            .expect("set genesis blockchain length succeeds");
        indexer_store
            .add_canonical_block(1, root_hash)
            .expect("add genesis canonical block succeeds");

        let tip = Tip {
            state_hash: root_branch.root_block().state_hash.clone(),
            node_id: root_branch.root.clone(),
        };

        Ok(Self {
            phase: IndexerPhase::InitializingFromBlockDir,
            canonical_tip: tip.clone(),
            diffs_map: HashMap::from([(
                genesis_block.state_hash.clone().into(),
                LedgerDiff::from_precomputed_block(&genesis_block),
            )]),
            best_tip: tip,
            root_branch,
            dangling_branches: Vec::new(),
            indexer_store: Some(indexer_store),
            transition_frontier_length,
            prune_interval,
            canonical_update_threshold,
            blocks_processed: 0,
            init_time: Instant::now(),
        })
    }

    /// Creates a new indexer state without genesis events
    pub fn new_without_genesis_events(
        root_hash: &BlockHash,
        indexer_store: Arc<IndexerStore>,
        transition_frontier_length: u32,
        prune_interval: u32,
        canonical_update_threshold: u32,
    ) -> anyhow::Result<Self> {
        let root_branch = Branch::new_genesis(root_hash)?;
        let tip = Tip {
            state_hash: root_branch.root_block().state_hash.clone(),
            node_id: root_branch.root.clone(),
        };

        Ok(Self {
            phase: IndexerPhase::SyncingFromDB,
            canonical_tip: tip.clone(),
            diffs_map: HashMap::new(),
            best_tip: tip,
            root_branch,
            dangling_branches: Vec::new(),
            indexer_store: Some(indexer_store),
            transition_frontier_length,
            prune_interval,
            canonical_update_threshold,
            blocks_processed: 0,
            init_time: Instant::now(),
        })
    }

    /// Creates a new indexer state for testing
    pub fn new_testing(
        root_block: &PrecomputedBlock,
        root_ledger: Option<Ledger>,
        rocksdb_path: Option<&std::path::Path>,
        transition_frontier_length: Option<u32>,
    ) -> anyhow::Result<Self> {
        let root_branch = Branch::new_testing(root_block);
        let indexer_store = rocksdb_path.map(|path| {
            let store = IndexerStore::new(path).unwrap();
            if let Some(ledger) = root_ledger {
                store
                    .add_ledger(&BlockHash(root_block.state_hash.clone()), ledger)
                    .expect("ledger add succeeds");
            }
            store
        });

        let tip = Tip {
            state_hash: root_branch.root_block().state_hash.clone(),
            node_id: root_branch.root.clone(),
        };

        Ok(Self {
            phase: IndexerPhase::Testing,
            canonical_tip: tip.clone(),
            diffs_map: HashMap::from([(
                root_block.state_hash.clone().into(),
                LedgerDiff::from_precomputed_block(root_block),
            )]),
            best_tip: tip,
            root_branch,
            dangling_branches: Vec::new(),
            indexer_store: indexer_store.map(Arc::new),
            transition_frontier_length: transition_frontier_length
                .unwrap_or(MAINNET_TRANSITION_FRONTIER_K),
            prune_interval: PRUNE_INTERVAL_DEFAULT,
            canonical_update_threshold: CANONICAL_UPDATE_THRESHOLD,
            blocks_processed: 0,
            init_time: Instant::now(),
        })
    }

    #[instrument(skip_all)]
    pub fn spawn_secondary_database(&self) -> anyhow::Result<IndexerStore> {
        let primary_path = self.indexer_store.as_ref().unwrap().db_path.clone();
        let mut secondary_path = primary_path.clone();
        secondary_path.push(Uuid::new_v4().to_string());

        debug!("Spawning secondary readonly RocksDB instance");
        let block_store_readonly = IndexerStore::new_read_only(&primary_path, &secondary_path)?;
        Ok(block_store_readonly)
    }

    /// Removes the lower portion of the root tree which is no longer needed
    fn prune_root_branch(&mut self) -> anyhow::Result<Option<WitnessTreeEvent>> {
        let k = self.transition_frontier_length;
        if let Some(witness_tree_event) = self.update_canonical()? {
            if self.root_branch.height() > self.prune_interval * k {
                let best_tip_block = self.best_tip_block().clone();
                debug!(
                    "Pruning transition frontier: k = {}, best tip length = {}, canonical tip length = {}",
                    k,
                    self.best_tip_block().blockchain_length,
                    self.canonical_tip_block().blockchain_length,
                );

                self.root_branch
                    .prune_transition_frontier(k, &best_tip_block);
            }
            return Ok(Some(witness_tree_event));
        }

        Ok(None)
    }

    /// The highest known canonical block
    pub fn canonical_tip_block(&self) -> &Block {
        self.get_block_from_id(&self.canonical_tip.node_id)
    }

    /// The highest block known to be a descendant of the root block
    pub fn best_tip_block(&self) -> &Block {
        self.get_block_from_id(&self.best_tip.node_id)
    }

    /// Only works with blocks in the root branch
    fn get_block_from_id(&self, node_id: &NodeId) -> &Block {
        self.root_branch.branches.get(node_id).unwrap().data()
    }

    /// Updates the canonical tip if the precondition is met
    pub fn update_canonical(&mut self) -> anyhow::Result<Option<WitnessTreeEvent>> {
        if self.is_canonical_updatable() {
            let old_canonical_tip_id = self.canonical_tip.node_id.clone();
            let old_canonical_tip_hash = self.canonical_tip_block().state_hash.clone();
            let new_canonical_blocks = self.get_new_canonical_blocks(&old_canonical_tip_id)?;

            self.update_ledger_store(&old_canonical_tip_hash, &new_canonical_blocks)?;
            self.clean_up_diffs_map(&old_canonical_tip_id)?;

            return Ok(Some(WitnessTreeEvent::UpdateCanonicalChain(
                new_canonical_blocks,
            )));
        }

        Ok(None)
    }

    fn is_canonical_updatable(&self) -> bool {
        self.best_tip_block().height - self.canonical_tip_block().height
            >= self.canonical_update_threshold
    }

    /// Initialize indexer state from a collection of contiguous canonical blocks
    ///
    /// Short-circuits adding canonical blocks to the witness tree
    pub async fn initialize_with_canonical_chain_discovery(
        &mut self,
        block_parser: &mut BlockParser,
    ) -> anyhow::Result<()> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            let mut ledger = indexer_store
                .get_ledger(&self.canonical_tip.state_hash)?
                .unwrap();
            let total_time = Instant::now();

            if block_parser.num_canonical > BLOCK_REPORTING_FREQ_NUM {
                info!("Adding blocks to the state, reporting every {BLOCK_REPORTING_FREQ_NUM}...");
            } else {
                info!("Adding blocks to the state...");
            }

            // process canonical blocks first
            while self.blocks_processed < block_parser.num_canonical {
                self.blocks_processed += 1;

                if should_report_from_block_count(self.blocks_processed) {
                    let rate = self.blocks_processed as f64 / total_time.elapsed().as_secs() as f64;

                    info!(
                        "{} blocks parsed and applied in {:?}",
                        self.blocks_processed,
                        total_time.elapsed(),
                    );
                    info!(
                        "Estimated time: {} min",
                        (block_parser.total_num_blocks - self.blocks_processed) as f64
                            / (rate * 60_f64)
                    );
                    debug!("Rate: {rate} blocks/s");
                }

                let block = block_parser.next_block()?.unwrap();
                let state_hash = block.state_hash.clone().into();

                // apply and add to db
                ledger.apply_post_balances(&block);
                indexer_store.add_block(&block)?;
                indexer_store.add_canonical_block(
                    block.blockchain_length,
                    &block.state_hash.clone().into(),
                )?;
                indexer_store.set_max_canonical_blockchain_length(block.blockchain_length)?;

                // store ledger at specified cadence, e.g. every 100 blocks
                if self.blocks_processed % 100 == 0 {
                    indexer_store.add_ledger(&state_hash, ledger.clone())?;
                }

                if self.blocks_processed == block_parser.num_canonical {
                    // update root branch
                    self.root_branch = Branch::new(&block)?;
                    self.best_tip = Tip {
                        state_hash: self.root_branch.root_block().state_hash.clone(),
                        node_id: self.root_branch.root.clone(),
                    };
                    self.canonical_tip = self.best_tip.clone();
                }
            }
            assert_eq!(self.blocks_processed, block_parser.num_canonical);
        }

        // now add the successive non-canonical blocks
        self.add_blocks(block_parser).await
    }

    /// Initialize indexer state without short-circuiting canonical blocks
    pub async fn initialize_without_canonical_chain_discovery(
        &mut self,
        block_parser: &mut BlockParser,
    ) -> anyhow::Result<()> {
        self.add_blocks(block_parser).await
    }

    /// Adds blocks to the state according to `block_parser` then changes phase to Watching
    pub async fn add_blocks(&mut self, block_parser: &mut BlockParser) -> anyhow::Result<()> {
        let total_time = Instant::now();
        let mut step_time = total_time;

        if self.blocks_processed == 0 && block_parser.total_num_blocks > 500 {
            info!(
                "Reporting every {BLOCK_REPORTING_FREQ_SEC}s or {BLOCK_REPORTING_FREQ_NUM} blocks"
            );
        }

        while let Some(block) = block_parser.next_block()? {
            self.report_progress(block_parser, step_time, total_time)?;
            step_time = Instant::now();

            // *** block pipeline ***
            // - add to db
            // - add to witness tree
            // - db processes canonical blocks
            if let Some(db_event) = self.add_block_to_store(&block)? {
                let new_canonical_blocks = if db_event.is_new_block_event() {
                    let (_, WitnessTreeEvent::UpdateCanonicalChain(blocks)) =
                        self.add_block_to_witness_tree(&block)?;
                    blocks
                } else {
                    vec![]
                };

                new_canonical_blocks
                    .iter()
                    .for_each(|block| self.add_canonical_block_to_store(block).unwrap());
            }
        }

        info!(
            "Ingested {} blocks in {:?}",
            self.blocks_processed,
            total_time.elapsed(),
        );

        debug!("Phase change: {} -> {}", self.phase, IndexerPhase::Watching);
        self.phase = IndexerPhase::Watching;
        Ok(())
    }

    /// Adds the block to the witness tree
    /// No store operations
    pub fn add_block_to_witness_tree(
        &mut self,
        precomputed_block: &PrecomputedBlock,
    ) -> anyhow::Result<(ExtensionType, WitnessTreeEvent)> {
        let incoming_length = precomputed_block.blockchain_length;
        if self.root_branch.root_block().blockchain_length > incoming_length {
            debug!(
                "Block with state hash {:?} has length {incoming_length} which is too low to add to the witness tree",
                precomputed_block.state_hash,
            );
            return Ok((ExtensionType::BlockNotAdded, WitnessTreeEvent::empty()));
        }

        self.blocks_processed += 1;
        self.diffs_map.insert(
            precomputed_block.state_hash.clone().into(),
            LedgerDiff::from_precomputed_block(precomputed_block),
        );

        // forward extension on root branch
        if self.is_length_within_root_bounds(precomputed_block) {
            if let Some(root_extension) = self.root_extension(precomputed_block)? {
                let event = self.prune_root_branch()?;
                return Ok((root_extension, event.unwrap_or(WitnessTreeEvent::empty())));
            }
        }

        // if a dangling branch has been extended (forward or reverse) check for new connections to other dangling branches
        if let Some((extended_branch_index, new_node_id, direction)) =
            self.dangling_extension(precomputed_block)?
        {
            return self
                .update_dangling(
                    precomputed_block,
                    extended_branch_index,
                    new_node_id,
                    direction,
                )
                .map(|ext| (ext, WitnessTreeEvent::empty()));
        }

        self.new_dangling(precomputed_block)
            .map(|ext| (ext, WitnessTreeEvent::empty()))
    }

    /// Extends the root branch forward, potentially causing dangling branches to be merged into it
    fn root_extension(
        &mut self,
        precomputed_block: &PrecomputedBlock,
    ) -> anyhow::Result<Option<ExtensionType>> {
        if let Some((new_node_id, new_block)) = self.root_branch.simple_extension(precomputed_block)
        {
            trace!(
                "root extension (length {}): {}",
                precomputed_block.blockchain_length,
                precomputed_block.state_hash
            );
            // check if new block connects to a dangling branch
            let mut merged_tip_id = None;
            let mut branches_to_remove = Vec::new();

            for (index, dangling_branch) in self.dangling_branches.iter_mut().enumerate() {
                // new block is the parent of the dangling branch root
                if is_reverse_extension(dangling_branch, precomputed_block) {
                    merged_tip_id = self.root_branch.merge_on(&new_node_id, dangling_branch);
                    branches_to_remove.push(index);
                }
            }

            if let Some(merged_tip_id) = merged_tip_id {
                let merged_tip_block = self
                    .root_branch
                    .branches
                    .get(&merged_tip_id)
                    .unwrap()
                    .data()
                    .clone();

                if merged_tip_block.blockchain_length > self.best_tip_block().blockchain_length
                    || merged_tip_block.state_hash.0 > self.best_tip_block().state_hash.0
                {
                    self.update_best_tip(&merged_tip_block, &merged_tip_id);
                }
            }

            self.update_best_tip(&new_block, &new_node_id);

            if !branches_to_remove.is_empty() {
                // the root branch is newly connected to dangling branches
                for (num_removed, index_to_remove) in branches_to_remove.iter().enumerate() {
                    self.dangling_branches.remove(index_to_remove - num_removed);
                }
                Ok(Some(ExtensionType::RootComplex))
            } else {
                // there aren't any branches that are connected
                Ok(Some(ExtensionType::RootSimple))
            }
        } else {
            Ok(None)
        }
    }

    /// Extends an existing dangling branch either forwards or backwards
    fn dangling_extension(
        &mut self,
        precomputed_block: &PrecomputedBlock,
    ) -> anyhow::Result<Option<(usize, NodeId, ExtensionDirection)>> {
        let mut extension = None;
        for (index, dangling_branch) in self.dangling_branches.iter_mut().enumerate() {
            let min_length = dangling_branch.root_block().blockchain_length;
            let max_length = dangling_branch.best_tip().unwrap().blockchain_length;

            // check incoming block is within the length bounds
            if max_length + 1 >= precomputed_block.blockchain_length
                && precomputed_block.blockchain_length + 1 >= min_length
            {
                // simple reverse
                if is_reverse_extension(dangling_branch, precomputed_block) {
                    dangling_branch.new_root(precomputed_block);
                    extension = Some((
                        index,
                        dangling_branch
                            .branches
                            .root_node_id()
                            .expect("has root")
                            .clone(),
                        ExtensionDirection::Reverse,
                    ));
                    break;
                }

                // simple forward
                if let Some((new_node_id, _)) = dangling_branch.simple_extension(precomputed_block)
                {
                    extension = Some((index, new_node_id, ExtensionDirection::Forward));
                    break;
                }
            }
        }

        Ok(extension)
    }

    /// Updates an existing dangling branch in the witness tree
    fn update_dangling(
        &mut self,
        precomputed_block: &PrecomputedBlock,
        extended_branch_index: usize,
        new_node_id: NodeId,
        direction: ExtensionDirection,
    ) -> anyhow::Result<ExtensionType> {
        let mut branches_to_update = Vec::new();
        for (index, dangling_branch) in self.dangling_branches.iter().enumerate() {
            if is_reverse_extension(dangling_branch, precomputed_block) {
                branches_to_update.push(index);
            }
        }

        if !branches_to_update.is_empty() {
            let mut extended_branch = self.dangling_branches.remove(extended_branch_index);
            for (n, dangling_branch_index) in branches_to_update.iter().enumerate() {
                let index = if extended_branch_index < *dangling_branch_index {
                    dangling_branch_index - n - 1
                } else {
                    *dangling_branch_index
                };
                let branch_to_update = self.dangling_branches.get_mut(index).unwrap();
                extended_branch.merge_on(&new_node_id, branch_to_update);

                // remove one for each index we see
                self.dangling_branches.remove(index);
            }

            self.dangling_branches.push(extended_branch);
            Ok(ExtensionType::DanglingComplex)
        } else {
            match direction {
                ExtensionDirection::Forward => Ok(ExtensionType::DanglingSimpleForward),
                ExtensionDirection::Reverse => Ok(ExtensionType::DanglingSimpleReverse),
            }
        }
    }

    /// Spawns a new dangling branch in the witness tree
    fn new_dangling(
        &mut self,
        precomputed_block: &PrecomputedBlock,
    ) -> anyhow::Result<ExtensionType> {
        self.dangling_branches.push(Branch::new(precomputed_block)?);
        Ok(ExtensionType::DanglingNew)
    }

    /// Checks if it's even possible to add block to the root branch
    fn is_length_within_root_bounds(&self, precomputed_block: &PrecomputedBlock) -> bool {
        self.best_tip_block().blockchain_length + 1 >= precomputed_block.blockchain_length
    }

    /// Update the best tip of the root branch
    fn update_best_tip(&mut self, incoming_block: &Block, node_id: &NodeId) {
        let best_tip_length = self.best_tip_block().blockchain_length;

        if incoming_block.blockchain_length == best_tip_length + 1
            || incoming_block.blockchain_length == best_tip_length
                && incoming_block > self.best_tip_block()
        {
            debug!(
                "Update best tip: length = {}, state_hash = {}",
                incoming_block.blockchain_length, incoming_block.state_hash.0
            );
            self.best_tip.node_id = node_id.clone();
            self.best_tip.state_hash = incoming_block.state_hash.clone();
        }

        let (id, block) = self.root_branch.best_tip_with_id().unwrap();
        self.best_tip.node_id = id;
        self.best_tip.state_hash = block.state_hash;
    }

    pub fn chain_commands(&self) -> Vec<Command> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            return self
                .root_branch
                .longest_chain()
                .iter()
                .flat_map(|state_hash| indexer_store.get_block(state_hash))
                .flatten()
                .flat_map(|precomputed_block| Command::from_precomputed_block(&precomputed_block))
                .collect();
        }
        vec![]
    }

    pub fn get_block_status(&self, state_hash: &BlockHash) -> anyhow::Result<Option<Canonicity>> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            if let Some(PrecomputedBlock {
                blockchain_length, ..
            }) = indexer_store.get_block(state_hash)?
            {
                if let Some(max_canonical_length) =
                    indexer_store.get_max_canonical_blockchain_length()?
                {
                    if blockchain_length > max_canonical_length {
                        return Ok(Some(Canonicity::Pending));
                    } else if indexer_store.get_canonical_hash_at_height(blockchain_length)?
                        == Some(state_hash.clone())
                    {
                        return Ok(Some(Canonicity::Canonical));
                    } else {
                        return Ok(Some(Canonicity::Orphaned));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Returns the ledger corresponding to the best tip
    pub fn best_ledger(&self) -> anyhow::Result<Option<Ledger>> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            let best_tip_hash = self.best_tip_block().state_hash.clone();
            return indexer_store.get_ledger(&best_tip_hash);
        }

        Ok(None)
    }

    /// Get the canonical block at the given height
    pub fn canonical_block_at_height(
        &self,
        height: u32,
    ) -> anyhow::Result<Option<PrecomputedBlock>> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            if let Some(state_hash) = indexer_store.get_canonical_hash_at_height(height)? {
                return indexer_store.get_block(&state_hash);
            }
        }

        Ok(None)
    }

    /// Get the ledger at the specified height
    pub fn ledger_at_height(&self, height: u32) -> anyhow::Result<Option<Ledger>> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            return indexer_store.get_ledger_at_height(height);
        }

        Ok(None)
    }

    pub fn len(&self) -> u32 {
        let mut len = self.root_branch.len();
        for dangling in &self.dangling_branches {
            len += dangling.len();
        }
        len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Add block to the underlying block store
    pub fn add_block_to_store(&self, block: &PrecomputedBlock) -> anyhow::Result<Option<DbEvent>> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            return Ok(Some(indexer_store.add_block(block)?));
        }
        Ok(None)
    }

    fn add_canonical_block_to_store(&self, block: &Block) -> anyhow::Result<()> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            indexer_store.add_canonical_block(block.blockchain_length, &block.state_hash)?;
        }
        Ok(())
    }

    /// Sync from an existing db
    ///
    /// Short-circuits adding blocks to the witness tree by starting at
    /// the most recent canonical block and only adding the succeeding blocks
    pub fn sync_from_db(&mut self) -> anyhow::Result<()> {
        let mut successive_blocks = vec![];
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            let event_log = indexer_store.get_event_log()?;
            let canonical_block_events = event_log.iter().filter(|e| e.is_canonical_block_event());
            if let Some(IndexerEvent::Db(DbEvent::Canonicity(
                DbCanonicityEvent::NewCanonicalBlock {
                    blockchain_length: canonical_length,
                    state_hash,
                },
            ))) = canonical_block_events.last()
            {
                // invariant
                assert_eq!(
                    Some(*canonical_length),
                    indexer_store.get_max_canonical_blockchain_length()?
                );

                // root branch root is canonical block
                // add all successive NewBlock's to the witness tree
                if let Some(block) = indexer_store.get_block(&state_hash.clone().into())? {
                    self.root_branch = Branch::new(&block)?;

                    let tip = Tip {
                        state_hash: self.root_branch.root_block().state_hash.clone(),
                        node_id: self.root_branch.root.clone(),
                    };
                    self.canonical_tip = tip.clone();
                    self.best_tip = tip;

                    for state_hash in event_log.iter().filter_map(|e| match e {
                        IndexerEvent::Db(DbEvent::Block(DbBlockWatcherEvent::NewBlock {
                            state_hash,
                            blockchain_length,
                        })) => {
                            if blockchain_length > canonical_length {
                                Some(state_hash.clone())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }) {
                        if let Some(block) = indexer_store.get_block(&state_hash.into())? {
                            successive_blocks.push(block);
                        } else {
                            panic!(
                                "Fatal sync error: block missing from db {}",
                                block.state_hash
                            )
                        }
                    }
                } else {
                    panic!("Fatal sync error: block missing from db {}", state_hash)
                }
            } else {
                // add all NewBlock's to the witness tree
                for state_hash in event_log.iter().filter_map(|e| match e {
                    IndexerEvent::Db(DbEvent::Block(DbBlockWatcherEvent::NewBlock {
                        state_hash,
                        ..
                    })) => Some(state_hash.clone()),
                    _ => None,
                }) {
                    if let Some(block) = indexer_store.get_block(&state_hash.into())? {
                        successive_blocks.push(block);
                    }
                }
            }
        } else {
            panic!("Fatal sync error: no indexer store");
        };

        for block in successive_blocks {
            trace!(
                "Sync: add block (height: {}): {}",
                block.blockchain_length,
                block.state_hash
            );
            self.add_block_to_witness_tree(&block)?;
        }

        Ok(())
    }

    /// Replay events on a mutable state
    pub fn replay_events(&mut self) -> anyhow::Result<()> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            let events = indexer_store.get_event_log()?;
            events
                .iter()
                .for_each(|event| self.replay_event(event).unwrap());
        }
        Ok(())
    }

    fn replay_event(&mut self, event: &IndexerEvent) -> anyhow::Result<()> {
        match event {
            IndexerEvent::BlockWatcher(block_event) => match block_event {
                BlockWatcherEvent::SawBlock { state_hash, .. } => {
                    info!("Replay block with hash {state_hash}");
                    todo!("replay {:?}", block_event);
                }
                BlockWatcherEvent::WatchDir(path) => {
                    info!("Replay watch dir {}", path.display());
                    todo!("set fs block watcher {:?}", block_event);
                }
            },
            IndexerEvent::LedgerWatcher(ledger_event) => match ledger_event {
                LedgerWatcherEvent::NewLedger { hash, path } => {
                    info!("Replay new ledger hash {hash} at {}", path.display());
                    todo!("replay {:?}", ledger_event);
                }
                LedgerWatcherEvent::WatchDir(path) => {
                    info!("Replay watch ledger dir {}", path.display());
                    todo!("set fs ledger watcher {:?}", ledger_event);
                }
            },
            IndexerEvent::Db(db_event) => {
                match db_event {
                    DbEvent::Block(db_block_event) => match db_block_event {
                        DbBlockWatcherEvent::AlreadySeenBlock {
                            state_hash,
                            blockchain_length,
                        } => {
                            info!("Replay already seen block in db (height {blockchain_length}, hash {state_hash})");
                            Ok(())
                        }
                        DbBlockWatcherEvent::NewBlock {
                            blockchain_length,
                            state_hash,
                        } => {
                            info!("Replay db new block (height: {blockchain_length}, hash: {state_hash})");
                            if let Some(indexer_store) = self.indexer_store.as_ref() {
                                if let Ok(Some(block)) =
                                    indexer_store.get_block(&state_hash.to_string().into())
                                {
                                    self.add_block_to_witness_tree(&block)?;
                                    Ok(())
                                } else {
                                    Err(anyhow!("Error: block missing (length {blockchain_length}): {state_hash}"))
                                }
                            } else {
                                Err(anyhow!("Fatal: no indexer store"))
                            }
                        }
                    },
                    DbEvent::Ledger(ledger_event) => match ledger_event {
                        DbLedgerWatcherEvent::AlreadySeenLedger(hash) => {
                            info!("replay already seen db ledger with hash {hash}");
                            Ok(())
                        }
                        DbLedgerWatcherEvent::NewLedger { hash } => {
                            info!("replay new db ledger hash {hash}");
                            Ok(())
                        }
                    },
                    DbEvent::Canonicity(canonicity_event) => match canonicity_event {
                        DbCanonicityEvent::NewCanonicalBlock {
                            blockchain_length,
                            state_hash,
                        } => {
                            info!("replay new canonical block (height: {blockchain_length}, hash: {state_hash})");
                            Ok(())
                        }
                    },
                }
            }
            IndexerEvent::WitnessTree(WitnessTreeEvent::UpdateCanonicalChain(blocks)) => {
                info!("replay update canonical chain {:?}", blocks);
                Ok(())
            }
        }
    }

    fn get_new_canonical_blocks(
        &mut self,
        old_canonical_tip_id: &NodeId,
    ) -> anyhow::Result<Vec<Block>> {
        let mut canonical_blocks = vec![];

        for ancestor_id in self
            .root_branch
            .branches
            .ancestor_ids(&self.best_tip.node_id)
            .unwrap()
            .skip(MAINNET_CANONICAL_THRESHOLD.saturating_sub(1) as usize)
        {
            // only add blocks between the old_canonical_tip and the new one
            if ancestor_id != old_canonical_tip_id {
                let ancestor_block = self.get_block_from_id(ancestor_id).clone();
                if canonical_blocks.is_empty() {
                    // update canonical tip
                    self.canonical_tip.node_id = ancestor_id.clone();
                    self.canonical_tip.state_hash = ancestor_block.state_hash.clone();
                }
                canonical_blocks.push(ancestor_block);
            } else {
                break;
            }
        }

        // sort lowest to highest
        canonical_blocks.reverse();

        Ok(canonical_blocks)
    }

    /// Add new canonical ledgers to the ledger store
    fn update_ledger_store(
        &self,
        old_canonical_tip_hash: &BlockHash,
        canonical_blocks: &Vec<Block>,
    ) -> anyhow::Result<()> {
        if let Some(indexer_store) = self.indexer_store.as_ref() {
            if let Some(mut ledger) = indexer_store.get_ledger(old_canonical_tip_hash)? {
                // apply the new canonical diffs and store each nth resulting ledger (n = 100)
                for canonical_block in canonical_blocks {
                    let diff = self
                        .diffs_map
                        .get(&canonical_block.state_hash)
                        .expect("block is in diffs_map");
                    ledger.apply_diff(diff)?;

                    if canonical_block.blockchain_length % 100 == 0 {
                        indexer_store.add_ledger(&canonical_block.state_hash, ledger.clone())?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Remove diffs corresponding to blocks at or beneath the height of the new canonical tip
    fn clean_up_diffs_map(&mut self, old_canonical_tip_id: &NodeId) -> anyhow::Result<()> {
        for node_id in self
            .root_branch
            .branches
            .traverse_level_order_ids(old_canonical_tip_id)
            .unwrap()
        {
            if self.get_block_from_id(&node_id).height <= self.canonical_tip_block().height {
                self.diffs_map
                    .remove(&self.get_block_from_id(&node_id).state_hash.clone());
            }
        }
        Ok(())
    }

    pub fn summary_short(&self) -> SummaryShort {
        let mut max_dangling_height = 0;
        let mut max_dangling_length = 0;

        for dangling in &self.dangling_branches {
            if dangling.height() > max_dangling_height {
                max_dangling_height = dangling.height();
            }
            if dangling.len() > max_dangling_length {
                max_dangling_length = dangling.len();
            }
        }

        let db_stats_str = self.indexer_store.as_ref().map(|db| db.db_stats());
        let mem = self
            .indexer_store
            .as_ref()
            .map(|db| db.memtables_size())
            .unwrap_or_default();
        let witness_tree = WitnessTreeSummaryShort {
            best_tip_hash: self.best_tip_block().state_hash.0.clone(),
            best_tip_length: self.best_tip_block().blockchain_length,
            canonical_tip_hash: self.canonical_tip_block().state_hash.0.clone(),
            canonical_tip_length: self.canonical_tip_block().blockchain_length,
            root_hash: self.root_branch.root_block().state_hash.0.clone(),
            root_height: self.root_branch.height(),
            root_length: self.root_branch.len(),
            num_leaves: self.root_branch.leaves().len() as u32,
            num_dangling: self.dangling_branches.len() as u32,
            max_dangling_height,
            max_dangling_length,
        };

        SummaryShort {
            uptime: Instant::now() - self.init_time,
            blocks_processed: self.blocks_processed,
            witness_tree,
            db_stats: db_stats_str.map(|s| DbStats::from_str(&format!("{mem}\n{s}")).unwrap()),
        }
    }

    pub fn summary_verbose(&self) -> SummaryVerbose {
        let mut max_dangling_height = 0;
        let mut max_dangling_length = 0;

        for dangling in &self.dangling_branches {
            if dangling.height() > max_dangling_height {
                max_dangling_height = dangling.height();
            }
            if dangling.len() > max_dangling_length {
                max_dangling_length = dangling.len();
            }
        }

        let db_stats_str = self.indexer_store.as_ref().map(|db| db.db_stats());
        let mem = self
            .indexer_store
            .as_ref()
            .map(|db| db.memtables_size())
            .unwrap_or_default();
        let witness_tree = WitnessTreeSummaryVerbose {
            best_tip_hash: self.best_tip_block().state_hash.0.clone(),
            best_tip_length: self.best_tip_block().blockchain_length,
            canonical_tip_hash: self.canonical_tip_block().state_hash.0.clone(),
            canonical_tip_length: self.canonical_tip_block().blockchain_length,
            root_hash: self.root_branch.root_block().state_hash.0.clone(),
            root_height: self.root_branch.height(),
            root_length: self.root_branch.len(),
            num_leaves: self.root_branch.leaves().len() as u32,
            num_dangling: self.dangling_branches.len() as u32,
            max_dangling_height,
            max_dangling_length,
            witness_tree: format!("{self}"),
        };

        SummaryVerbose {
            uptime: Instant::now() - self.init_time,
            blocks_processed: self.blocks_processed,
            witness_tree,
            db_stats: db_stats_str.map(|s| DbStats::from_str(&format!("{mem}\n{s}")).unwrap()),
        }
    }

    fn is_initializing(&self) -> bool {
        self.phase == IndexerPhase::InitializingFromBlockDir
    }

    fn should_report_from_time(&self, duration: Duration) -> bool {
        self.is_initializing() && duration.as_secs() > BLOCK_REPORTING_FREQ_SEC
    }

    fn report_progress(
        &self,
        block_parser: &BlockParser,
        step_time: Instant,
        total_time: Instant,
    ) -> anyhow::Result<()> {
        if should_report_from_block_count(self.blocks_processed)
            || self.should_report_from_time(step_time.elapsed())
        {
            let best_tip: BlockWithoutHeight = self.best_tip_block().clone().into();
            let canonical_tip: BlockWithoutHeight = self.canonical_tip_block().clone().into();
            let rate = self.blocks_processed as f64 / total_time.elapsed().as_secs() as f64;

            info!(
                "Parsed and added {} blocks to the witness tree in {:?}",
                self.blocks_processed,
                total_time.elapsed(),
            );

            debug!("Root height:       {}", self.root_branch.height());
            debug!("Root length:       {}", self.root_branch.len());
            debug!("Rate:              {rate} blocks/s");

            info!(
                "Estimate rem time: {} hr",
                (block_parser.total_num_blocks - self.blocks_processed) as f64 / (rate * 3600_f64)
            );
            info!("Best tip:          {best_tip:?}");
            info!("Canonical tip:     {canonical_tip:?}");
        }
        Ok(())
    }
}

/// Checks if the block is the parent of the branch's root
fn is_reverse_extension(branch: &Branch, precomputed_block: &PrecomputedBlock) -> bool {
    precomputed_block.state_hash == branch.root_block().parent_hash.0
}

fn should_report_from_block_count(block_count: u32) -> bool {
    block_count > 0 && block_count % BLOCK_REPORTING_FREQ_NUM == 0
}

impl std::fmt::Display for IndexerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Root branch ===")?;
        writeln!(f, "{}", self.root_branch)?;

        if !self.dangling_branches.is_empty() {
            writeln!(f, "=== Dangling branches ===")?;
            for (n, branch) in self.dangling_branches.iter().enumerate() {
                writeln!(f, "Dangling branch {n}:")?;
                writeln!(f, "{branch}")?;
            }
        }

        Ok(())
    }
}

impl std::fmt::Display for IndexerPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexerPhase::InitializingFromBlockDir => write!(f, "initializing"),
            IndexerPhase::SyncingFromDB => write!(f, "syncing"),
            IndexerPhase::Replaying => write!(f, "replaying"),
            IndexerPhase::Watching => write!(f, "watching"),
            IndexerPhase::Testing => write!(f, "testing"),
        }
    }
}
