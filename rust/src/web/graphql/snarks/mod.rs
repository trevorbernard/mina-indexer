use super::gen::BlockQueryInput;
use crate::{
    block::{precomputed::PrecomputedBlock, store::BlockStore},
    constants::*,
    ledger::public_key::PublicKey,
    snark_work::{store::SnarkStore, SnarkWorkSummary, SnarkWorkSummaryWithStateHash},
    store::{
        blocks_global_slot_idx_iterator, blocks_global_slot_idx_state_hash_from_entry, IndexerStore,
    },
    web::graphql::{db, get_block_canonicity},
};
use async_graphql::{ComplexObject, Context, Enum, InputObject, Object, Result, SimpleObject};
use std::sync::Arc;

#[derive(SimpleObject, Debug)]
pub struct Snark {
    pub fee: u64,
    pub prover: String,
    pub block: SnarkBlock,
}

#[derive(SimpleObject, Debug)]
pub struct SnarkBlock {
    pub state_hash: String,
}

#[derive(SimpleObject, Debug)]
#[graphql(complex)]
pub struct SnarkWithCanonicity {
    /// Value canonicity
    pub canonical: bool,
    /// Value optional block
    #[graphql(skip)]
    pub pcb: PrecomputedBlock,
    /// Value snark
    #[graphql(flatten)]
    pub snark: Snark,
}

#[ComplexObject]
impl SnarkWithCanonicity {
    /// Value state hash
    async fn state_hash(&self) -> String {
        self.pcb.state_hash().0.to_owned()
    }
    /// Value block height
    async fn block_height(&self) -> u32 {
        self.pcb.blockchain_length()
    }
    /// Value date time
    async fn date_time(&self) -> String {
        millis_to_iso_date_string(self.pcb.timestamp() as i64)
    }
}

#[derive(InputObject)]
pub struct SnarkQueryInput {
    canonical: Option<bool>,
    prover: Option<String>,
    block_height: Option<u32>,
    block: Option<BlockQueryInput>,
    and: Option<Vec<SnarkQueryInput>>,
    or: Option<Vec<SnarkQueryInput>>,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum SnarkSortByInput {
    #[graphql(name = "BLOCKHEIGHT_ASC")]
    BlockHeightAsc,
    #[graphql(name = "BLOCKHEIGHT_DESC")]
    BlockHeightDesc,
}

#[derive(Default)]
pub struct SnarkQueryRoot;

#[Object]
impl SnarkQueryRoot {
    async fn snarks<'ctx>(
        &self,
        ctx: &Context<'ctx>,
        query: Option<SnarkQueryInput>,
        sort_by: Option<SnarkSortByInput>,
        #[graphql(default = 100)] limit: usize,
    ) -> Result<Vec<SnarkWithCanonicity>> {
        let db = db(ctx);
        let mut snarks = Vec::with_capacity(limit);
        let sort_by = sort_by.unwrap_or(SnarkSortByInput::BlockHeightDesc);

        // state hash
        if let Some(state_hash) = query
            .as_ref()
            .and_then(|q| q.block.as_ref())
            .and_then(|block| block.state_hash.clone())
        {
            let mut snarks: Vec<SnarkWithCanonicity> = db
                .get_block(&state_hash.into())?
                .into_iter()
                .flat_map(|block| {
                    SnarkWorkSummaryWithStateHash::from_precomputed(&block)
                        .into_iter()
                        .filter_map(|s| snark_summary_matches_query(db, &query, s).ok().flatten())
                        .collect::<Vec<SnarkWithCanonicity>>()
                })
                .collect();

            match sort_by {
                SnarkSortByInput::BlockHeightAsc => snarks.reverse(),
                SnarkSortByInput::BlockHeightDesc => (),
            }

            snarks.truncate(limit);
            return Ok(snarks);
        }

        // block height
        if let Some(block_height) = query.as_ref().and_then(|q| q.block_height) {
            let mut snarks: Vec<SnarkWithCanonicity> = db
                .get_blocks_at_height(block_height)?
                .into_iter()
                .flat_map(|block| {
                    SnarkWorkSummaryWithStateHash::from_precomputed(&block)
                        .into_iter()
                        .filter_map(|s| snark_summary_matches_query(db, &query, s).ok().flatten())
                        .collect::<Vec<SnarkWithCanonicity>>()
                })
                .collect();

            match sort_by {
                SnarkSortByInput::BlockHeightAsc => snarks.reverse(),
                SnarkSortByInput::BlockHeightDesc => (),
            }

            snarks.truncate(limit);
            return Ok(snarks);
        }

        // prover query
        if let Some(prover) = query.as_ref().and_then(|q| q.prover.clone()) {
            let mut snarks =
                db.get_snark_work_by_public_key(&prover.into())?
                    .map_or(vec![], |snarks| {
                        snarks
                            .into_iter()
                            .filter_map(|s| {
                                snark_summary_matches_query(db, &query, s).ok().flatten()
                            })
                            .collect()
                    });

            match sort_by {
                SnarkSortByInput::BlockHeightAsc => snarks.reverse(),
                SnarkSortByInput::BlockHeightDesc => (),
            }

            snarks.truncate(limit);
            return Ok(snarks);
        }

        // general query
        let mode = match sort_by {
            SnarkSortByInput::BlockHeightAsc => speedb::IteratorMode::Start,
            SnarkSortByInput::BlockHeightDesc => speedb::IteratorMode::End,
        };
        let mut capacity_reached = false;
        for entry in blocks_global_slot_idx_iterator(db, mode) {
            let state_hash = blocks_global_slot_idx_state_hash_from_entry(&entry)?;
            let block = db
                .get_block(&state_hash.clone().into())?
                .expect("block to be returned");
            let canonical = get_block_canonicity(db, &state_hash);
            let snark_work = db.get_snark_work_in_block(&state_hash.clone().into())?;
            let snarks_with_canonicity = snark_work.map_or(vec![], |summaries| {
                summaries
                    .into_iter()
                    .map(|snark| SnarkWithCanonicity {
                        canonical,
                        pcb: block.clone(),
                        snark: (snark, state_hash.clone()).into(),
                    })
                    .collect()
            });

            for sw in snarks_with_canonicity {
                if query.as_ref().map_or(true, |q| q.matches(&sw)) {
                    snarks.push(sw);
                }

                if snarks.len() == limit {
                    capacity_reached = true;
                    break;
                }
            }
            if capacity_reached {
                break;
            }
        }
        Ok(snarks)
    }
}

fn snark_summary_matches_query(
    db: &Arc<IndexerStore>,
    query: &Option<SnarkQueryInput>,
    snark: SnarkWorkSummaryWithStateHash,
) -> anyhow::Result<Option<SnarkWithCanonicity>> {
    let canonical = get_block_canonicity(db, &snark.state_hash);
    Ok(db
        .get_block(&snark.state_hash.clone().into())?
        .and_then(|block| {
            let snark_with_canonicity = SnarkWithCanonicity {
                pcb: block,
                canonical,
                snark: snark.into(),
            };
            if query
                .as_ref()
                .map_or(true, |q| q.matches(&snark_with_canonicity))
            {
                Some(snark_with_canonicity)
            } else {
                None
            }
        }))
}

impl From<(SnarkWorkSummary, String)> for Snark {
    fn from(snark: (SnarkWorkSummary, String)) -> Self {
        Snark {
            fee: snark.0.fee,
            prover: snark.0.prover.0,
            block: SnarkBlock {
                state_hash: snark.1,
            },
        }
    }
}

impl From<SnarkWorkSummaryWithStateHash> for Snark {
    fn from(snark: SnarkWorkSummaryWithStateHash) -> Self {
        Snark {
            fee: snark.fee,
            prover: snark.prover.0,
            block: SnarkBlock {
                state_hash: snark.state_hash.clone(),
            },
        }
    }
}

impl SnarkQueryInput {
    pub fn matches(&self, snark: &SnarkWithCanonicity) -> bool {
        let mut matches = true;
        let Self {
            block,
            canonical,
            prover,
            block_height,
            and,
            or,
        } = self;

        if let Some(block_query_input) = block {
            if let Some(state_hash) = &block_query_input.state_hash {
                matches &= snark.pcb.state_hash().0 == *state_hash;
            }
        }
        if let Some(block_height) = block_height {
            matches &= snark.pcb.blockchain_length() == *block_height;
        }
        if let Some(prover) = prover {
            matches &= snark
                .pcb
                .prover_keys()
                .contains(&<String as Into<PublicKey>>::into(prover.clone()));
        }
        if let Some(canonical) = canonical {
            matches &= snark.canonical == *canonical;
        }
        if let Some(query) = and {
            matches &= query.iter().all(|and| and.matches(snark));
        }
        if let Some(query) = or {
            if !query.is_empty() {
                matches &= query.iter().any(|or| or.matches(snark));
            }
        }
        matches
    }
}
