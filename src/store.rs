use crate::{
    block::{precomputed::PrecomputedBlock, store::BlockStore, BlockHash},
    canonical::store::CanonicityStore,
    event::{store::EventStore, Event, db::*},
    state::{
        ledger::{store::LedgerStore, Ledger},
        Canonicity,
    },
};
use rocksdb::{ColumnFamilyDescriptor, DB};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};
use tracing::trace;

#[derive(Debug)]
pub struct IndexerStore {
    pub db_path: PathBuf,
    pub database: DB,
}

impl IndexerStore {
    pub fn new_read_only(path: &Path, secondary: &Path) -> anyhow::Result<Self> {
        let database_opts = rocksdb::Options::default();
        let database = rocksdb::DBWithThreadMode::open_cf_as_secondary(
            &database_opts,
            path,
            secondary,
            vec!["blocks", "canonicity", "events", "ledgers"],
        )?;
        Ok(Self {
            db_path: PathBuf::from(secondary),
            database,
        })
    }

    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let mut cf_opts = rocksdb::Options::default();
        cf_opts.set_max_write_buffer_number(16);
        let blocks = ColumnFamilyDescriptor::new("blocks", cf_opts.clone());
        let canonicity = ColumnFamilyDescriptor::new("canonicity", cf_opts.clone());
        let events = ColumnFamilyDescriptor::new("events", cf_opts.clone());
        let ledgers = ColumnFamilyDescriptor::new("ledgers", cf_opts);

        let mut database_opts = rocksdb::Options::default();
        database_opts.create_missing_column_families(true);
        database_opts.create_if_missing(true);
        let database = rocksdb::DBWithThreadMode::open_cf_descriptors(
            &database_opts,
            path,
            vec![blocks, canonicity, events, ledgers],
        )?;
        Ok(Self {
            db_path: PathBuf::from(path),
            database,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn blocks_cf(&self) -> &rocksdb::ColumnFamily {
        self.database
            .cf_handle("blocks")
            .expect("blocks column family exists")
    }

    fn canonicity_cf(&self) -> &rocksdb::ColumnFamily {
        self.database
            .cf_handle("canonicity")
            .expect("canonicity column family exists")
    }

    fn ledgers_cf(&self) -> &rocksdb::ColumnFamily {
        self.database
            .cf_handle("ledgers")
            .expect("ledgers column family exists")
    }

    fn events_cf(&self) -> &rocksdb::ColumnFamily {
        self.database
            .cf_handle("events")
            .expect("events column family exists")
    }
}

impl BlockStore for IndexerStore {
    /// Add the specified block at its state hash
    fn add_block(&self, block: &PrecomputedBlock) -> anyhow::Result<()> {
        trace!(
            "Adding block with height {} and hash {}",
            block.blockchain_length,
            block.state_hash
        );
        self.database.try_catch_up_with_primary().unwrap_or(());

        // add block to db
        let key = block.state_hash.as_bytes();
        let value = serde_json::to_vec(&block)?;
        let blocks_cf = self.blocks_cf();
        self.database.put_cf(&blocks_cf, key, value)?;
        
        // add new block event
        self.add_event(&Event::Db(DbEvent::Block(DbBlockEvent::NewBlock { path: ".".into(), state_hash: block.state_hash.clone(), blockchain_length: block.blockchain_length })))?;

        Ok(())
    }

    /// Get the block with the specified hash
    fn get_block(&self, state_hash: &BlockHash) -> anyhow::Result<Option<PrecomputedBlock>> {
        trace!("Getting block with hash {}", state_hash.0);
        self.database.try_catch_up_with_primary().unwrap_or(());

        let key = state_hash.0.as_bytes();
        let blocks_cf = self.blocks_cf();
        match self
            .database
            .get_pinned_cf(&blocks_cf, key)?
            .map(|bytes| bytes.to_vec())
        {
            None => Ok(None),
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        }
    }

    /// Set the speicifed block's canonicity
    fn set_block_canonicity(
        &self,
        state_hash: &BlockHash,
        canonicity: Canonicity,
    ) -> anyhow::Result<()> {
        trace!("Setting canonicity of block with hash {}", state_hash.0);
        self.database.try_catch_up_with_primary().unwrap_or(());

        match self.get_block(state_hash)? {
            None => Ok(()),
            Some(precomputed_block) => {
                let with_canonicity = PrecomputedBlock {
                    canonicity: Some(canonicity),
                    ..precomputed_block
                };
                self.add_block(&with_canonicity)
            }
        }
    }

    /// Get the specified block's canonicity
    fn get_block_canonicity(&self, state_hash: &BlockHash) -> anyhow::Result<Option<Canonicity>> {
        trace!("Getting canonicity of block with hash {}", state_hash.0);
        self.database.try_catch_up_with_primary().unwrap_or(());

        match self.get_block(state_hash)? {
            Some(PrecomputedBlock {
                canonicity: Some(block_canonicity),
                ..
            }) => Ok(Some(block_canonicity)),
            _ => Ok(None),
        }
    }
}

impl CanonicityStore for IndexerStore {
    /// Add a canonical state hash at the specified blockchain_length
    fn add_canonical_block(&self, height: u32, state_hash: &BlockHash) -> anyhow::Result<()> {
        trace!(
            "Adding canonical block at height {height} with hash {}",
            state_hash.0
        );
        self.database.try_catch_up_with_primary().unwrap_or(());

        // add canonicity info
        let key = height.to_be_bytes();
        let value = serde_json::to_vec(&state_hash)?;
        let canonicity_cf = self.canonicity_cf();
        self.database.put_cf(&canonicity_cf, key, value)?;
        
        // record new canonical block event
        self.add_event(&Event::Db(DbEvent::Canonicity(DbCanonicityEvent::NewCanonicalBlock { state_hash: state_hash.0.clone(), blockchain_length: height })))?;
        
        Ok(())
    }

    /// Get the state hash of the canonical block with the specified blockchain_length
    fn get_canonical_hash_at_height(&self, height: u32) -> anyhow::Result<Option<BlockHash>> {
        trace!("Getting canonical hash at height {height}");
        self.database.try_catch_up_with_primary().unwrap_or(());

        let key = height.to_be_bytes();
        let canonicity_cf = self.canonicity_cf();
        match self
            .database
            .get_pinned_cf(&canonicity_cf, key)?
            .map(|bytes| bytes.to_vec())
        {
            None => Ok(None),
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        }
    }

    /// Get the length of the canonical chain
    fn get_max_canonical_blockchain_length(&self) -> anyhow::Result<Option<u32>> {
        trace!("Getting max canonical blockchain length");
        self.database.try_catch_up_with_primary().unwrap_or(());

        let canonicity_cf = self.canonicity_cf();
        match self
            .database
            .get_pinned_cf(&canonicity_cf, Self::MAX_CANONICAL_KEY)?
            .map(|bytes| bytes.to_vec())
        {
            None => Ok(None),
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        }
    }

    /// Set the length of the canonical chain
    fn set_max_canonical_blockchain_length(&self, height: u32) -> anyhow::Result<()> {
        trace!("Setting max canonical blockchain length to {height}");
        self.database.try_catch_up_with_primary().unwrap_or(());

        let canonicity_cf = self.canonicity_cf();
        let value = serde_json::to_vec(&height)?;
        self.database
            .put_cf(&canonicity_cf, Self::MAX_CANONICAL_KEY, value)?;
        Ok(())
    }
}

impl LedgerStore for IndexerStore {
    /// Add the specified ledger at the key `state_hash`
    fn add_ledger(&self, state_hash: &BlockHash, ledger: Ledger) -> anyhow::Result<()> {
        trace!("Adding ledger at {}", state_hash.0);
        self.database.try_catch_up_with_primary().unwrap_or(());

        // add ledger to db
        let key = state_hash.0.as_bytes();
        let value = bcs::to_bytes(&ledger.to_string())?;
        let ledgers_cf = self.ledgers_cf();
        self.database.put_cf(&ledgers_cf, key, value)?;

        // add new ledger event
        self.add_event(&Event::Db(DbEvent::Ledger(DbLedgerEvent::NewLedger { path: ".".into(), hash: state_hash.0.clone() })))?;
        Ok(())
    }

    /// Get the ledger at the specified state hash
    fn get_ledger(&self, state_hash: &BlockHash) -> anyhow::Result<Option<Ledger>> {
        trace!("Getting ledger at {}", state_hash.0);
        self.database.try_catch_up_with_primary().unwrap_or(());

        let ledgers_cf = self.ledgers_cf();
        let mut state_hash = state_hash.clone();
        let mut to_apply = vec![];

        // walk chain back to a stored ledger (canonical)
        // collect blocks to compute the current ledger
        while self
            .database
            .get_pinned_cf(&ledgers_cf, state_hash.0.as_bytes())?
            .is_none()
        {
            if let Some(block) = self.get_block(&state_hash)? {
                to_apply.push(block.clone());
                state_hash = block.previous_state_hash();
            } else {
                return Ok(None);
            }
        }

        to_apply.reverse();

        let key = state_hash.0.as_bytes();
        if let Some(mut ledger) = self
            .database
            .get_pinned_cf(&ledgers_cf, key)?
            .map(|bytes| bytes.to_vec())
            .map(|bytes| Ledger::from_str(bcs::from_bytes(&bytes).unwrap()).unwrap())
        {
            for block in to_apply {
                ledger.apply_post_balances(&block);
            }

            return Ok(Some(ledger));
        }

        Ok(None)
    }

    /// Get the canonical ledger at the specified height
    fn get_ledger_at_height(&self, height: u32) -> anyhow::Result<Option<Ledger>> {
        trace!("Getting ledger at height {height}");
        self.database.try_catch_up_with_primary().unwrap_or(());

        match self.get_canonical_hash_at_height(height)? {
            None => Ok(None),
            Some(state_hash) => self.get_ledger(&state_hash),
        }
    }
}

impl EventStore for IndexerStore {
    fn add_event(&self, event: &Event) -> anyhow::Result<u32> {
        let seq_num = self.get_next_seq_num()?;
        trace!("Adding event {seq_num}: {:?}", event);

        if let Event::State(_) = event {
            return Ok(seq_num);
        }
        self.database.try_catch_up_with_primary().unwrap_or(());

        // add event to db
        let key = seq_num.to_be_bytes();
        let value = serde_json::to_vec(&event)?;
        let events_cf = self.events_cf();
        self.database.put_cf(&events_cf, key, value)?;

        // increment event sequence number
        let next_seq_num = seq_num + 1;
        let value = serde_json::to_vec(&next_seq_num)?;
        self.database
            .put_cf(&events_cf, Self::NEXT_EVENT_SEQ_NUM_KEY, value)?;

        // return next event sequence number
        Ok(next_seq_num)
    }

    fn get_event(&self, seq_num: u32) -> anyhow::Result<Option<Event>> {
        trace!("Getting event {seq_num}");
        self.database.try_catch_up_with_primary().unwrap_or(());

        let key = seq_num.to_be_bytes();
        let events_cf = self.events_cf();
        let event = self.database.get_cf(&events_cf, key)?;
        Ok(event.map(|bytes| serde_json::from_slice(&bytes).unwrap()))
    }

    fn get_next_seq_num(&self) -> anyhow::Result<u32> {
        trace!("Getting next event sequence number");
        self.database.try_catch_up_with_primary().unwrap_or(());

        if let Some(bytes) = self
            .database
            .get_cf(&self.events_cf(), Self::NEXT_EVENT_SEQ_NUM_KEY)?
        {
            serde_json::from_slice(&bytes).map_err(anyhow::Error::from)
        } else {
            Ok(0)
        }
    }

    fn get_event_log(&self) -> anyhow::Result<Vec<Event>> {
        trace!("Getting event log");

        let mut events = vec![];

        for n in 0..self.get_next_seq_num()? {
            if let Some(event) = self.get_event(n)? {
                events.push(event);
            }
        }
        Ok(events)
    }
}

impl IndexerStore {
    const NEXT_EVENT_SEQ_NUM_KEY: &[u8] = "next_event_seq_num".as_bytes();
    const MAX_CANONICAL_KEY: &[u8] = "max_canonical_blockchain_length".as_bytes();

    pub fn db_stats(&self) -> String {
        self.database
            .property_value(rocksdb::properties::DBSTATS)
            .unwrap()
            .unwrap()
    }

    pub fn memtables_size(&self) -> String {
        self.database
            .property_value(rocksdb::properties::CUR_SIZE_ALL_MEM_TABLES)
            .unwrap()
            .unwrap()
    }

    pub fn estimate_live_data_size(&self) -> u64 {
        self.database
            .property_int_value(rocksdb::properties::ESTIMATE_LIVE_DATA_SIZE)
            .unwrap()
            .unwrap()
    }

    pub fn estimate_num_keys(&self) -> u64 {
        self.database
            .property_int_value(rocksdb::properties::ESTIMATE_NUM_KEYS)
            .unwrap()
            .unwrap()
    }

    pub fn cur_size_all_mem_tables(&self) -> u64 {
        self.database
            .property_int_value(rocksdb::properties::CUR_SIZE_ALL_MEM_TABLES)
            .unwrap()
            .unwrap()
    }
}
