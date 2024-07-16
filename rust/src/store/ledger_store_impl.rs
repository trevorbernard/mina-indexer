use super::column_families::ColumnFamilyHelpers;
use crate::{
    block::{store::BlockStore, BlockHash},
    canonicity::store::CanonicityStore,
    constants::*,
    event::{db::*, store::EventStore, IndexerEvent},
    ledger::{
        diff::LedgerDiff,
        public_key::PublicKey,
        staking::{AggregatedEpochStakeDelegations, StakingLedger},
        store::LedgerStore,
        Ledger, LedgerHash,
    },
    store::{account::AccountStore, from_be_bytes, to_be_bytes, IndexerStore},
};
use log::{error, trace};

impl LedgerStore for IndexerStore {
    ////////////////////
    // Staged ledgers //
    ////////////////////

    fn add_ledger(&self, ledger_hash: &LedgerHash, state_hash: &BlockHash) -> anyhow::Result<bool> {
        trace!("Adding staged ledger\nstate_hash: {state_hash}\nledger_hash: {ledger_hash}");
        let is_new = self
            .database
            .get_cf(self.ledgers_cf(), ledger_hash.0.as_bytes())?
            .is_none();
        self.database.put_cf(
            self.ledgers_cf(),
            ledger_hash.0.as_bytes(),
            state_hash.0.as_bytes(),
        )?;
        Ok(is_new)
    }

    fn get_best_ledger(&self) -> anyhow::Result<Option<Ledger>> {
        trace!("Getting best ledger");
        self.get_ledger_state_hash(&self.get_best_block_hash()?.expect("best block"), true)
    }

    fn add_ledger_state_hash(&self, state_hash: &BlockHash, ledger: Ledger) -> anyhow::Result<()> {
        trace!("Adding staged ledger state hash {state_hash}");

        // add ledger to db
        self.database.put_cf(
            self.ledgers_cf(),
            state_hash.0.as_bytes(),
            ledger.to_string(),
        )?;

        // index on state hash & add new ledger event
        if self
            .get_known_genesis_prev_state_hashes()?
            .contains(state_hash)
        {
            if self
                .add_ledger(&LedgerHash(MAINNET_GENESIS_LEDGER_HASH.into()), state_hash)
                .unwrap_or(false)
            {
                self.add_event(&IndexerEvent::Db(DbEvent::Ledger(
                    DbLedgerEvent::NewLedger {
                        blockchain_length: 0,
                        state_hash: state_hash.clone(),
                        ledger_hash: LedgerHash(MAINNET_GENESIS_LEDGER_HASH.into()),
                    },
                )))?;
            }
        } else {
            match self.get_block(state_hash)? {
                Some(block) => {
                    let ledger_hash = block.staged_ledger_hash();
                    if self.add_ledger(&ledger_hash, state_hash).unwrap_or(false) {
                        self.add_event(&IndexerEvent::Db(DbEvent::Ledger(
                            DbLedgerEvent::NewLedger {
                                ledger_hash,
                                state_hash: block.state_hash(),
                                blockchain_length: block.blockchain_length(),
                            },
                        )))?;
                    }
                }
                None => {
                    if state_hash.0 != MAINNET_GENESIS_PREV_STATE_HASH {
                        error!("Block missing from store: {state_hash}");
                    }
                }
            }
        }
        Ok(())
    }

    fn add_genesis_ledger(
        &self,
        state_hash: &BlockHash,
        genesis_ledger: Ledger,
    ) -> anyhow::Result<()> {
        // initialize account balances for sorting
        for (pk, acct) in &genesis_ledger.accounts {
            self.update_account_balance(pk, Some(acct.balance.0))?;
        }

        // add the ledger to the db
        self.add_ledger_state_hash(state_hash, genesis_ledger)?;
        Ok(())
    }

    fn get_ledger_state_hash(
        &self,
        state_hash: &BlockHash,
        memoize: bool,
    ) -> anyhow::Result<Option<Ledger>> {
        trace!("Getting staged ledger state hash {state_hash}");

        let mut curr_state_hash = state_hash.clone();
        let mut diffs = vec![];

        // walk chain back to a stored ledger
        // collect diffs to compute the current ledger
        while self
            .database
            .get_pinned_cf(self.ledgers_cf(), curr_state_hash.0.as_bytes())?
            .is_none()
        {
            trace!("No staged ledger found for state hash {curr_state_hash}");
            if let Some(diff) = self.get_block_ledger_diff(&curr_state_hash)? {
                diffs.push(diff);
                if let Ok(Some(parent_hash)) = self.get_block_parent_hash(&curr_state_hash) {
                    trace!("Checking for staged ledger state hash {parent_hash}");
                    curr_state_hash = parent_hash;
                }
            } else {
                if curr_state_hash.0 != MAINNET_GENESIS_PREV_STATE_HASH {
                    error!("Block missing from store: {curr_state_hash}");
                }
                return Ok(None);
            }
        }

        trace!("Found staged ledger state hash {curr_state_hash}");
        if let Some(mut ledger) = self
            .database
            .get_pinned_cf(self.ledgers_cf(), curr_state_hash.0.as_bytes())?
            .and_then(|bytes| Ledger::from_bytes(bytes.to_vec()).ok())
        {
            // apply diffs
            diffs.reverse();
            let diff = LedgerDiff::append_vec(diffs);
            ledger._apply_diff(&diff)?;

            if memoize {
                trace!("Memoizing ledger for block {state_hash}");
                self.add_ledger_state_hash(state_hash, ledger.clone())?;
            }
            return Ok(Some(ledger));
        }
        Ok(None)
    }

    fn get_ledger(&self, ledger_hash: &LedgerHash) -> anyhow::Result<Option<Ledger>> {
        trace!("Getting staged ledger hash {ledger_hash}");
        let key = ledger_hash.0.as_bytes();
        if let Some(state_hash) = self
            .database
            .get_pinned_cf(self.ledgers_cf(), key)?
            .and_then(|bytes| BlockHash::from_bytes(&bytes).ok())
        {
            if let Some(ledger) = self
                .database
                .get_pinned_cf(self.ledgers_cf(), state_hash.0.as_bytes())?
                .and_then(|bytes| Ledger::from_bytes(bytes.to_vec()).ok())
            {
                return Ok(Some(ledger));
            }
        }
        Ok(None)
    }

    fn get_ledger_at_height(&self, height: u32, memoize: bool) -> anyhow::Result<Option<Ledger>> {
        trace!("Getting staged ledger at height {height}");
        self.get_canonical_hash_at_height(height)?
            .map_or(Ok(None), |state_hash| {
                self.get_ledger_state_hash(&state_hash, memoize)
            })
    }

    fn set_block_ledger_diff(
        &self,
        state_hash: &BlockHash,
        ledger_diff: LedgerDiff,
    ) -> anyhow::Result<()> {
        trace!("Setting block ledger diff {state_hash}: {ledger_diff:?}");
        Ok(self.database.put_cf(
            self.block_ledger_diff_cf(),
            state_hash.0.as_bytes(),
            serde_json::to_vec(&ledger_diff)?,
        )?)
    }

    fn get_block_ledger_diff(&self, state_hash: &BlockHash) -> anyhow::Result<Option<LedgerDiff>> {
        trace!("Getting block ledger diff {state_hash}");
        Ok(self
            .database
            .get_pinned_cf(self.block_ledger_diff_cf(), state_hash.0.as_bytes())?
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
    }

    fn set_block_staged_ledger_hash(
        &self,
        state_hash: &BlockHash,
        staged_ledger_hash: &LedgerHash,
    ) -> anyhow::Result<()> {
        trace!("Setting block staged ledger hash {state_hash}: {staged_ledger_hash}");
        Ok(self.database.put_cf(
            self.block_staged_ledger_hash_cf(),
            state_hash.0.as_bytes(),
            staged_ledger_hash.0.as_bytes(),
        )?)
    }

    fn get_block_staged_ledger_hash(
        &self,
        state_hash: &BlockHash,
    ) -> anyhow::Result<Option<LedgerHash>> {
        trace!("Getting block staged ledger hash {state_hash}");
        Ok(self
            .database
            .get_cf(self.block_staged_ledger_hash_cf(), state_hash.0.as_bytes())?
            .and_then(|bytes| LedgerHash::from_bytes(bytes).ok()))
    }

    /////////////////////
    // Staking ledgers //
    /////////////////////

    fn get_staking_ledger_at_epoch(
        &self,
        epoch: u32,
        genesis_state_hash: Option<BlockHash>,
    ) -> anyhow::Result<Option<StakingLedger>> {
        trace!("Getting staking ledger epoch {epoch}");
        let genesis_state_hash = genesis_state_hash
            .clone()
            .unwrap_or_else(|| self.get_best_block_genesis_hash().ok().flatten().unwrap());
        if let Some(ledger_hash) = self.get_ledger_hash(epoch)? {
            if let Some(ledger) = self
                .database
                .get_pinned_cf(
                    self.staking_ledgers_cf(),
                    staking_ledger_key(genesis_state_hash, epoch, &ledger_hash),
                )?
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            {
                return Ok(Some(ledger));
            }
        }
        Ok(None)
    }

    /// If some epoch is given, use it over the ledger hash,
    /// else get the epoch from the ledger hash
    fn get_staking_ledger_hash(
        &self,
        ledger_hash: &LedgerHash,
        epoch: Option<u32>,
        genesis_state_hash: Option<BlockHash>,
    ) -> anyhow::Result<Option<StakingLedger>> {
        trace!("Getting staking ledger hash {ledger_hash}");
        match epoch {
            None => {
                if let (Ok(Some(epoch)), Some(genesis_state_hash)) = (
                    self.get_epoch(ledger_hash),
                    genesis_state_hash
                        .or_else(|| self.get_best_block_genesis_hash().ok().flatten()),
                ) {
                    if let Ok(Some(bytes)) = self.database.get_pinned_cf(
                        self.staking_ledgers_cf(),
                        staking_ledger_key(genesis_state_hash, epoch, ledger_hash),
                    ) {
                        return Ok(Some(serde_json::from_slice(&bytes)?));
                    }
                }
                Ok(None)
            }
            Some(epoch) => {
                if let Ok(Some(staking_ledger)) =
                    self.get_staking_ledger_at_epoch(epoch, genesis_state_hash)
                {
                    if staking_ledger.ledger_hash == *ledger_hash {
                        return Ok(Some(staking_ledger));
                    }
                }
                Ok(None)
            }
        }
    }

    fn add_staking_ledger(
        &self,
        staking_ledger: StakingLedger,
        genesis_state_hash: &BlockHash,
    ) -> anyhow::Result<()> {
        let epoch = staking_ledger.epoch;
        trace!("Adding staking ledger {}", staking_ledger.summary());

        // add ledger at ledger hash
        let key = staking_ledger_key(
            genesis_state_hash.clone(),
            staking_ledger.epoch,
            &staking_ledger.ledger_hash,
        );
        let is_new = self
            .database
            .get_pinned_cf(self.staking_ledgers_cf(), key.clone())?
            .is_none();

        // add staking ledger
        self.database.put_cf(
            self.staking_ledgers_cf(),
            key,
            serde_json::to_vec(&staking_ledger)?,
        )?;

        // add (ledger hash, epoch) index
        self.set_ledger_hash_epoch_pair(&staking_ledger.ledger_hash, epoch)?;

        // add (ledger hash, genesis state hash) index
        self.set_ledger_hash_genesis_pair(&staking_ledger.ledger_hash, genesis_state_hash)?;

        // add aggregated delegations
        trace!("Aggregating staking delegations epoch {epoch}");
        let aggregated_delegations = staking_ledger.aggregate_delegations()?;
        self.database.put_cf(
            self.staking_delegations_cf(),
            staking_ledger_epoch_key(genesis_state_hash.clone(), epoch),
            serde_json::to_vec(&aggregated_delegations)?,
        )?;

        // add per epoch, balance-sorted & delegation-sorted
        for (pk, account) in staking_ledger.staking_ledger.iter() {
            // balance-sort
            self.database.put_cf(
                self.staking_ledger_balance_cf(),
                staking_ledger_sort_key(epoch, account.balance, &pk.0),
                serde_json::to_vec(account)?,
            )?;

            // stake-sort
            let stake = aggregated_delegations
                .delegations
                .get(pk)
                .cloned()
                .unwrap_or_default()
                .total_delegated
                .unwrap_or_default();
            self.database.put_cf(
                self.staking_ledger_stake_cf(),
                staking_ledger_sort_key(epoch, stake, &pk.0),
                serde_json::to_vec(account)?,
            )?;
        }
        // add staking ledger count at epoch
        let count = staking_ledger.staking_ledger.values().count();
        self.set_staking_ledger_accounts_count_epoch(
            epoch,
            genesis_state_hash.clone(),
            count as u32,
        )?;
        if is_new {
            // add new ledger event
            self.add_event(&IndexerEvent::Db(DbEvent::StakingLedger(
                DbStakingLedgerEvent::NewStakingLedger {
                    epoch,
                    ledger_hash: staking_ledger.ledger_hash.clone(),
                    genesis_state_hash: genesis_state_hash.clone(),
                },
            )))?;

            // add new aggregated delegation event
            self.add_event(&IndexerEvent::Db(DbEvent::StakingLedger(
                DbStakingLedgerEvent::AggregateDelegations {
                    epoch: staking_ledger.epoch,
                    genesis_state_hash: genesis_state_hash.clone(),
                },
            )))?;
        }

        Ok(())
    }

    fn get_delegations_epoch(
        &self,
        epoch: u32,
        genesis_state_hash: &Option<BlockHash>,
    ) -> anyhow::Result<Option<AggregatedEpochStakeDelegations>> {
        trace!("Getting staking delegations for epoch {epoch}");
        let genesis_state_hash = genesis_state_hash
            .clone()
            .unwrap_or_else(|| self.get_best_block_genesis_hash().ok().flatten().unwrap());
        if let Some(bytes) = self.database.get_pinned_cf(
            self.staking_delegations_cf(),
            staking_ledger_epoch_key(genesis_state_hash, epoch),
        )? {
            return Ok(Some(serde_json::from_slice(&bytes)?));
        }
        Ok(None)
    }

    fn get_epoch(&self, ledger_hash: &LedgerHash) -> anyhow::Result<Option<u32>> {
        trace!("Getting epoch for ledger {ledger_hash}");
        Ok(self
            .database
            .get_cf(
                self.staking_ledger_hash_to_epoch_cf(),
                ledger_hash.0.as_bytes(),
            )?
            .map(from_be_bytes))
    }

    fn get_ledger_hash(&self, epoch: u32) -> anyhow::Result<Option<LedgerHash>> {
        trace!("Getting ledger hash for epoch {epoch}");
        Ok(self
            .database
            .get_cf(self.staking_ledger_epoch_to_hash_cf(), to_be_bytes(epoch))?
            .and_then(|bytes| LedgerHash::from_bytes(bytes).ok()))
    }

    fn set_ledger_hash_epoch_pair(
        &self,
        ledger_hash: &LedgerHash,
        epoch: u32,
    ) -> anyhow::Result<()> {
        trace!("Setting epoch {epoch} for ledger {ledger_hash}");
        self.database.put_cf(
            self.staking_ledger_epoch_to_hash_cf(),
            to_be_bytes(epoch),
            ledger_hash.0.as_bytes(),
        )?;
        Ok(self.database.put_cf(
            self.staking_ledger_hash_to_epoch_cf(),
            ledger_hash.0.as_bytes(),
            to_be_bytes(epoch),
        )?)
    }

    fn set_ledger_hash_genesis_pair(
        &self,
        ledger_hash: &LedgerHash,
        genesis_state_hash: &BlockHash,
    ) -> anyhow::Result<()> {
        trace!("Setting genesis state hash {genesis_state_hash} for ledger {ledger_hash}");
        Ok(self.database.put_cf(
            self.staking_ledger_genesis_hash_cf(),
            ledger_hash.0.as_bytes(),
            genesis_state_hash.0.as_bytes(),
        )?)
    }

    fn get_genesis_state_hash(
        &self,
        ledger_hash: &LedgerHash,
    ) -> anyhow::Result<Option<BlockHash>> {
        trace!("Getting genesis state hash for ledger {ledger_hash}");
        Ok(self
            .database
            .get_cf(
                self.staking_ledger_genesis_hash_cf(),
                ledger_hash.0.as_bytes(),
            )?
            .and_then(|bytes| BlockHash::from_bytes(&bytes).ok()))
    }

    ///////////////
    // Iterators //
    ///////////////

    fn staking_ledger_balance_iterator(
        &self,
        mode: speedb::IteratorMode,
    ) -> speedb::DBIterator<'_> {
        self.database
            .iterator_cf(self.staking_ledger_balance_cf(), mode)
    }

    fn staking_ledger_stake_iterator(&self, mode: speedb::IteratorMode) -> speedb::DBIterator<'_> {
        self.database
            .iterator_cf(self.staking_ledger_stake_cf(), mode)
    }

    ////////////////////////////
    // Staking ledger counts //
    ///////////////////////////

    fn get_staking_ledger_accounts_count_epoch(
        &self,
        epoch: u32,
        genesis_state_hash: BlockHash,
    ) -> anyhow::Result<u32> {
        trace!("Getting staking ledger accounts count for epoch {epoch} {genesis_state_hash:?}");
        Ok(self
            .database
            .get_cf(
                self.staking_ledger_accounts_epoch_cf(),
                staking_ledger_epoch_key(genesis_state_hash, epoch),
            )?
            .map_or(0, from_be_bytes))
    }

    fn set_staking_ledger_accounts_count_epoch(
        &self,
        epoch: u32,
        genesis_state_hash: BlockHash,
        count: u32,
    ) -> anyhow::Result<()> {
        trace!("Setting staking ledger accounts count for epoc {epoch} {genesis_state_hash:?}: {count}");
        Ok(self.database.put_cf(
            self.staking_ledger_accounts_epoch_cf(),
            staking_ledger_epoch_key(genesis_state_hash, epoch),
            to_be_bytes(count),
        )?)
    }
}

/// `{epoch BE}{amount BE}{suffix}`
pub fn staking_ledger_sort_key(epoch: u32, amount: u64, suffix: &str) -> Vec<u8> {
    let mut key = to_be_bytes(epoch);
    key.append(&mut amount.to_be_bytes().to_vec());
    key.append(&mut suffix.as_bytes().to_vec());
    key
}

/// 4 BE bytes for epoch (u32)
pub fn staking_ledger_sort_key_epoch(key: &[u8]) -> u32 {
    from_be_bytes(key[..4].to_vec())
}

/// 8 BE bytes for amount (u64)
pub fn staking_ledger_sort_key_amount(key: &[u8]) -> u32 {
    from_be_bytes(key[4..12].to_vec())
}

/// Remaining bytes for public key
pub fn staking_ledger_sort_key_pk(key: &[u8]) -> PublicKey {
    PublicKey::from_bytes(&key[12..]).expect("public key from bytes")
}

/// `{genesis_state_hash}{epoch BE}{ledger_hash}`
fn staking_ledger_key(
    genesis_state_hash: BlockHash,
    epoch: u32,
    ledger_hash: &LedgerHash,
) -> Vec<u8> {
    let mut key = staking_ledger_epoch_key(genesis_state_hash, epoch);
    key.append(&mut ledger_hash.0.clone().into_bytes());
    key
}

/// `{genesis_state_hash}{epoch BE}`
fn staking_ledger_epoch_key(genesis_state_hash: BlockHash, epoch: u32) -> Vec<u8> {
    let mut key = genesis_state_hash.to_bytes();
    key.append(&mut to_be_bytes(epoch));
    key
}
