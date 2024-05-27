use crate::helpers::setup_new_db_dir;
use mina_indexer::{
    block::{
        parser::BlockParser,
        precomputed::{PcbVersion, PrecomputedBlock},
        store::BlockStore,
    },
    canonicity::store::CanonicityStore,
    constants::*,
    event::{store::EventStore, witness_tree::*},
    ledger::genesis::GenesisRoot,
    server::IndexerVersion,
    state::IndexerState,
    store::IndexerStore,
};
use std::{path::PathBuf, sync::Arc};

#[tokio::test]
async fn test() {
    let blocks_dir = PathBuf::from("./tests/data/canonical_chain_discovery/contiguous");

    let store_dir0 = setup_new_db_dir("event-log-store0").unwrap();
    let mut block_parser0 = BlockParser::new_with_canonical_chain_discovery(
        &blocks_dir,
        PcbVersion::V1,
        MAINNET_CANONICAL_THRESHOLD,
        BLOCK_REPORTING_FREQ_NUM,
    )
    .unwrap();

    let store_dir1 = setup_new_db_dir("event-log-store1").unwrap();
    let mut block_parser1 = BlockParser::new_with_canonical_chain_discovery(
        &blocks_dir,
        PcbVersion::V1,
        MAINNET_CANONICAL_THRESHOLD,
        BLOCK_REPORTING_FREQ_NUM,
    )
    .unwrap();

    let indexer_store0 = Arc::new(IndexerStore::new(store_dir0.path()).unwrap());
    let indexer_store1 = Arc::new(IndexerStore::new(store_dir1.path()).unwrap());

    let genesis_contents = include_str!("../data/genesis_ledgers/mainnet.json");
    let genesis_root = serde_json::from_str::<GenesisRoot>(genesis_contents).unwrap();

    let mut state0 = IndexerState::new(
        genesis_root.clone().into(),
        IndexerVersion::new_testing(),
        indexer_store0.clone(),
        MAINNET_TRANSITION_FRONTIER_K,
    )
    .unwrap();
    let mut state1 = IndexerState::new(
        genesis_root.into(),
        IndexerVersion::new_testing(),
        indexer_store1.clone(),
        MAINNET_TRANSITION_FRONTIER_K,
    )
    .unwrap();

    // add parser0 blocks to state0
    state0.add_blocks(&mut block_parser0).unwrap();

    // add parser1 blocks to state1
    // - add block to db
    // - add block to witness tree
    // - update best tip
    // - update canonicities
    while let Some((block, _)) = block_parser1.next_block().unwrap() {
        let block: PrecomputedBlock = block.into();
        if let Some(db_event) = state1
            .indexer_store
            .as_ref()
            .map(|store| store.add_block(&block).unwrap())
        {
            if db_event.map(|db| db.is_new_block_event()).unwrap_or(false) {
                if let Some(wt_event) = state1.add_block_to_witness_tree(&block).unwrap().1 {
                    let (best_tip, new_canonical_blocks) = match wt_event {
                        WitnessTreeEvent::UpdateBestTip {
                            best_tip,
                            canonical_blocks,
                        } => (best_tip, canonical_blocks),
                    };

                    state1
                        .update_best_block_in_store(&best_tip.state_hash)
                        .unwrap();
                    new_canonical_blocks.iter().for_each(|block| {
                        if let Some(store) = state1.indexer_store.as_ref() {
                            store
                                .add_canonical_block(
                                    block.blockchain_length,
                                    &block.state_hash,
                                    &block.genesis_state_hash,
                                    None,
                                )
                                .unwrap()
                        }
                    });
                    state1.add_block_to_witness_tree(&block).unwrap();
                }
            }
        }
    }

    // check event logs match
    let event_log0 = if let Some(store0) = state0.indexer_store {
        store0.get_event_log().unwrap()
    } else {
        vec![]
    };
    let event_log1 = if let Some(store1) = state1.indexer_store {
        store1.get_event_log().unwrap()
    } else {
        vec![]
    };

    println!("----- Event log 0 -----");
    println!("{:?}", event_log0);
    println!("----- Event log 1 -----");
    println!("{:?}", event_log1);

    assert_eq!(event_log0, event_log1);
}
