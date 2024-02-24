use crate::{
    block::precomputed::PrecomputedBlock, proof_systems::signer::pubkey::CompressedPubKey,
    protocol::serialization_types::staged_ledger_diff::{self, UserCommandWithStatus},
};
use chrono::DateTime;

use serde::{Deserialize, Serialize};

/// RFC 2822 date format
fn millis_to_date_string(millis: i64) -> String {
    let date_time = DateTime::from_timestamp_millis(millis).unwrap();
    date_time.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

impl From<UserCommandWithStatus> for UserCommand {
    fn from(command: UserCommandWithStatus) -> Self {
        let uc = command
            .data
            .inner()
            .inner();
        let foo = match uc {
            staged_ledger_diff::UserCommand::SignedCommand(signed) => signed,
        };
        let signed_body = foo
            .inner()
            .inner()
            .payload
            .inner()
            .inner()
            .body
            .inner()
            .inner();
        let blah = match signed_body {
            staged_ledger_diff::SignedCommandPayloadBody::PaymentPayload(body) => {
                let ha = body
                    .inner()
                    .inner();
                    
            },
            staged_ledger_diff::SignedCommandPayloadBody::StakeDelegation(body) => {
                let ha = body
                    .inner();

            },
        };

        UserCommand {
            amount: 0,
            kind: "asdf".to_owned(),
        }
    }
}
impl From<PrecomputedBlock> for Block {
    fn from(block: PrecomputedBlock) -> Self {
        let state_hash = block.state_hash.clone();
        let winner_pk = block.block_creator().0;
        let block_height = block.blockchain_length;
        let canonical = true;
        let date_time = millis_to_date_string(block.timestamp().try_into().unwrap());
        let pk_creator = block.consensus_state().block_creator;
        let creator = CompressedPubKey::from(&pk_creator).into_address();
        let previous_state_hash = block.previous_state_hash().0;
        let received_time =
            millis_to_date_string(block.scheduled_time.clone().parse::<i64>().unwrap());
        let supercharged_coinbase = block.protocol_state.body
            .inner()
            .inner()
            .consensus_state
            .inner()
            .inner()
            .supercharge_coinbase;
        let coinbase = if supercharged_coinbase {
            720_000_000_000_u64
        } else {
            1440_000_000_000_u64
        };
        

        Block {
            state_hash,
            block_height,
            canonical,
            date_time,
            creator_account: CreatorAccount {
                public_key: creator.clone(),
            },
            creator,
            protocol_state: ProtocolState {
                previous_state_hash,
            },
            received_time,
            transactions: Transactions {
                coinbase,
                //user_commands,
            },
            winner_account: WinnerAccount {
                public_key: winner_pk,
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocks {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    state_hash: String,
    block_height: u32,
    canonical: bool,
    date_time: String,
    creator: String,
    creator_account: CreatorAccount,
    protocol_state: ProtocolState,
    received_time: String,
    // snark_jobs: Vec<SnarkJob>,
    transactions: Transactions,
    winner_account: WinnerAccount,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatorAccount {
    public_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolState {
    // blockchain_state: BlockchainState,
    // consensus_state: ConsensusState,
    previous_state_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockchainState {
    date: u64,
    snarked_ledger_hash: String,
    staged_ledger_hash: String,
    utc_date: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsensusState {
    block_height: u32,
    blockchain_length: u32,
    epoch: u32,
    epoch_count: u32,
    has_ancestor_in_same_checkpoint_window: bool,
    last_vrf_output: String,
    min_window_density: u32,
    next_epoch_data: EpochData,
    slot: u32,
    slot_since_genesis: u32,
    staking_epoch_data: EpochData,
    total_currency: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpochData {
    epoch_length: u32,
    ledger: Ledger,
    lock_checkpoint: String,
    seed: String,
    start_checkpoint: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    hash: String,
    total_currency: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnarkJob {
    block_height: u32,
    block_state_hash: String,
    date_time: String,
    fee: u64,
    prover: String,
    work_ids: Vec<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transactions {
    coinbase: u64,
    //coinbase_receiver_account: CreatorAccount,
    //fee_transfer: Vec<FeeTransfer>,
    //user_commands: Vec<UserCommand>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeTransfer {
    block_height: u32,
    block_state_hash: String,
    date_time: String,
    fee: u64,
    recipient: String,
    #[serde(rename = "type")]
    fee_transfer_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserCommand {
    amount: u64,
    kind: String,
    // block_height: u32,
    // nonce: u32,
    // block_state_hash: String,
    // date_time: String,
    // fee: u64,
    // fee_payer: FeePayer,
    // fee_token: u32,
    // from: String,
    // from_account: FeePayer,
    // hash: String,
    // id: String,
    // is_delegation: bool,
    // memo: String,
    // receiver: Receiver,
    // source: CreatorAccount,
    // to: String,
    // to_account: FeePayer,
    // token: u32,
    // delegatee: Option<CreatorAccount>,
    // delegator: Option<CreatorAccount>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeePayer {
    public_key: String,
    token: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receiver {
    public_key: String,
    token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinnerAccount {
    //balance: Balance,
    public_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    block_height: u32,
    liquid: u64,
    locked: u64,
    state_hash: String,
    total: u64,
    unknown: Option<String>,
}
