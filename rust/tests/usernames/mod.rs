use crate::helpers::setup_new_db_dir;
use mina_indexer::{
    block::parser::BlockParser,
    constants::*,
    ledger::genesis::GenesisRoot,
    server::IndexerVersion,
    state::IndexerState,
    store::{username::UsernameStore, IndexerStore},
};
use std::{path::PathBuf, sync::Arc};

#[test]
fn set_usernames() -> anyhow::Result<()> {
    let store_dir = setup_new_db_dir("usernames-db")?;
    let blocks_dir = &PathBuf::from("./tests/data/non_sequential_blocks");
    let store = Arc::new(IndexerStore::new(store_dir.path())?);
    let genesis_contents = include_str!("../data/genesis_ledgers/mainnet.json");
    let genesis_root = serde_json::from_str::<GenesisRoot>(genesis_contents).unwrap();
    let mut bp = BlockParser::new_with_canonical_chain_discovery(
        blocks_dir,
        IndexerVersion::new_testing().version,
        MAINNET_CANONICAL_THRESHOLD,
        BLOCK_REPORTING_FREQ_NUM,
    )?;
    let mut state = IndexerState::new(
        genesis_root.clone().into(),
        IndexerVersion::new_testing(),
        store.clone(),
        MAINNET_TRANSITION_FRONTIER_K,
    )?;

    state.add_blocks(&mut bp)?;

    assert_eq!(
        "Betelgeuse",
        store
            .get_username(&"B62qkEtH1PxqjJPKitAmzfV2ozCuCcibBL4tLgpeXHvsaqVgrENjFhX".into())?
            .unwrap()
    );
    Ok(())
}
