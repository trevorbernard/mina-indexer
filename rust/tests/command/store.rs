use crate::helpers::setup_new_db_dir;
use mina_indexer::{
    block::{parser::BlockParser, precomputed::PrecomputedBlock, store::BlockStore},
    command::{signed::SignedCommand, store::CommandStore},
    constants::*,
    ledger::genesis::parse_file,
    state::IndexerState,
    store::{
        user_commands_iterator, user_commands_iterator_global_slot, user_commands_iterator_network,
        user_commands_iterator_signed_command, user_commands_iterator_txn_hash, IndexerStore,
    },
};
use std::{path::PathBuf, sync::Arc};

#[tokio::test]
async fn add_and_get() -> anyhow::Result<()> {
    let store_dir = setup_new_db_dir("command-store")?;
    let blocks_dir = &PathBuf::from("./tests/data/non_sequential_blocks");
    let indexer_store = Arc::new(IndexerStore::new(store_dir.path())?);
    let genesis_ledger_path = &PathBuf::from("./tests/data/genesis_ledgers/mainnet.json");
    let genesis_root = parse_file(genesis_ledger_path)?;
    let indexer = IndexerState::new(
        genesis_root.into(),
        indexer_store.clone(),
        MAINNET_TRANSITION_FRONTIER_K,
    )?;

    let mut bp = BlockParser::new_with_canonical_chain_discovery(
        blocks_dir,
        MAINNET_CANONICAL_THRESHOLD,
        BLOCK_REPORTING_FREQ_NUM,
    )?;

    // add the first block to the store
    if let Some((block, _)) = bp.next_block()? {
        let block: PrecomputedBlock = block.into();
        indexer.add_block_to_store(&block)?;
    }

    let state_hash = "3NL4HLb7MQrxmAqVw8D4vEXCj2tdT8zgP9DFWGRoDxP72b4wxyUw";
    let (block, _) = bp.get_precomputed_block(state_hash).await?;
    let block_cmds = block.commands();
    let pks = block.all_command_public_keys();

    // add another block to the store
    indexer.add_block_to_store(&block)?;

    // check state hash key
    let result_cmds = indexer_store
        .as_ref()
        .get_commands_in_block(&state_hash.into())?;
    assert_eq!(result_cmds, block_cmds);

    // check each pk key
    for pk in pks {
        let pk_cmds: Vec<SignedCommand> = block_cmds
            .iter()
            .cloned()
            .map(SignedCommand::from)
            .filter(|x| x.contains_public_key(&pk))
            .collect();
        let result_pk_cmds: Vec<SignedCommand> = indexer_store
            .as_ref()
            .get_commands_for_public_key(&pk)?
            .into_iter()
            .map(SignedCommand::from)
            .collect();
        assert_eq!(result_pk_cmds, pk_cmds);
    }

    // check transaction hash key
    for cmd in SignedCommand::from_precomputed(&block) {
        let result_cmd: SignedCommand = indexer_store
            .get_command_by_hash(&cmd.hash_signed_command()?)?
            .unwrap()
            .into();
        assert_eq!(result_cmd, cmd);
    }

    // iterate over transactions
    let network = "mainnet";
    let mut curr_slot = 0;

    for entry in user_commands_iterator(network.to_string(), &indexer_store) {
        // no longer iterating over global slot prefixed keys
        if String::from_utf8(entry.to_owned()?.0[..network.as_bytes().len()].to_vec())
            == Ok(network.to_string())
        {
            // networks match
            assert_eq!(network.to_string(), user_commands_iterator_network(&entry)?);

            // txn hashes should match
            assert_eq!(
                user_commands_iterator_signed_command(&entry)?.tx_hash,
                user_commands_iterator_txn_hash(network, &entry)?,
            );

            // global slot numbers should match
            let cmd_slot = user_commands_iterator_global_slot(network, &entry);
            assert!(curr_slot <= cmd_slot);
            assert_eq!(
                cmd_slot,
                user_commands_iterator_signed_command(&entry)?.global_slot_since_genesis,
            );

            // blocks should be present
            let state_hash = user_commands_iterator_signed_command(&entry)?.state_hash;
            assert!(indexer_store.get_block(&state_hash)?.is_some());

            curr_slot = cmd_slot;
        }
    }
    Ok(())
}
