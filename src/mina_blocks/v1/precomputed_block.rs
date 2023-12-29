use std::path::Path;

use serde_derive::{Serialize, Deserialize};
use serde_json::Value;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecomputedBlock {
    pub scheduled_time: String,
    pub protocol_state: ProtocolState,
    pub protocol_state_proof: String,
    pub staged_ledger_diff: StagedLedgerDiff,
    pub delta_transition_chain_proof: (String, Vec<String>),
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolState {
    pub previous_state_hash: String,
    pub body: Body,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub genesis_state_hash: String,
    pub blockchain_state: BlockchainState,
    pub consensus_state: ConsensusState,
    pub constants: Constants,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockchainState {
    pub staged_ledger_hash: StagedLedgerHash,
    pub snarked_ledger_hash: String,
    pub genesis_ledger_hash: String,
    pub snarked_next_available_token: String,
    pub timestamp: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagedLedgerHash {
    pub non_snark: NonSnark,
    pub pending_coinbase_hash: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NonSnark {
    pub ledger_hash: String,
    pub aux_hash: String,
    pub pending_coinbase_aux: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsensusState {
    pub blockchain_length: String,
    pub epoch_count: String,
    pub min_window_density: String,
    pub sub_window_densities: Vec<String>,
    pub last_vrf_output: String,
    pub total_currency: String,
    pub curr_global_slot: CurrGlobalSlot,
    pub global_slot_since_genesis: String,
    pub staking_epoch_data: StakingEpochData,
    pub next_epoch_data: StakingEpochData,
    pub has_ancestor_in_same_checkpoint_window: bool,
    pub block_stake_winner: String,
    pub block_creator: String,
    pub coinbase_receiver: String,
    pub supercharge_coinbase: bool,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrGlobalSlot {
    pub slot_number: String,
    pub slots_per_epoch: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StakingEpochData {
    pub ledger: Ledger,
    pub seed: String,
    pub start_checkpoint: String,
    pub lock_checkpoint: String,
    pub epoch_length: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    pub hash: String,
    pub total_currency: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constants {
    pub k: String,
    pub slots_per_epoch: String,
    pub slots_per_sub_window: String,
    pub delta: String,
    pub genesis_state_timestamp: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagedLedgerDiff {
    pub diff: Vec<Option<Diff>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    pub completed_works: Vec<CompletedWork>,
    pub commands: Vec<Value>,
    pub coinbase: Vec<Option<Value>>,
    pub internal_command_balances: Vec<(String, InternalCommandBalances)>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletedWork {
    pub fee: String,
    pub proofs: Option<Value>, //(String, Proofs, Proofs),
    pub prover: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proofs {
    pub statement: Statement,
    pub proof: Proof,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub source: String,
    pub target: String,
    pub supply_increase: String,
    pub pending_coinbase_stack_state: PendingCoinbaseStackState,
    pub fee_excess: Vec<FeeExcess>,
    pub next_available_token_before: String,
    pub next_available_token_after: String,
    pub sok_digest: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingCoinbaseStackState {
    pub source: Source,
    pub target: Source,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub data: String,
    pub state: State,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub init: String,
    pub curr: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeExcess {
    pub token: String,
    pub amount: Amount,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Amount {
    pub magnitude: String,
    pub sgn: Vec<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proof {
    pub statement: Statement2,
    pub prev_evals: Vec<PrevEval>,
    pub prev_x_hat: Vec<String>,
    pub proof: Proof2,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement2 {
    pub proof_state: ProofState,
    pub pass_through: PassThrough,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofState {
    pub deferred_values: DeferredValues,
    pub sponge_digest_before_evaluations: Vec<i64>,
    pub me_only: MeOnly,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeferredValues {
    pub plonk: Plonk,
    pub combined_inner_product: Vec<String>,
    pub b: Vec<String>,
    pub xi: (String, Vec<i64>),
    pub bulletproof_challenges: Vec<BulletproofChallenges>,
    pub which_branch: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plonk {
    pub alpha: (String, Vec<i64>),
    pub beta: Vec<i64>,
    pub gamma: Vec<i64>,
    pub zeta: (String, Vec<i64>),
}







#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BulletproofChallenges {
    pub prechallenge: (String, Vec<i64>),
}



#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeOnly {
    pub sg: Vec<String>,
    pub old_bulletproof_challenges: Vec<Vec<BulletproofChallenges>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassThrough {
    pub app_state: Value,
    pub sg: Vec<Vec<String>>,
    pub old_bulletproof_challenges: Vec<Vec<BulletproofChallenges>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrevEval {
    pub l: Vec<String>,
    pub r: Vec<String>,
    pub o: Vec<String>,
    pub z: Vec<String>,
    pub t: Vec<String>,
    pub f: Vec<String>,
    pub sigma1: Vec<String>,
    pub sigma2: Vec<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proof2 {
    pub messages: Messages,
    pub openings: Openings,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Messages {
    pub l_comm: Vec<Vec<String>>,
    pub r_comm: Vec<Vec<String>>,
    pub o_comm: Vec<Vec<String>>,
    pub z_comm: Vec<Vec<String>>,
    pub t_comm: TComm,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TComm {
    pub unshifted: Vec<(String, Vec<String>)>,
    pub shifted: (String, Vec<String>),
}





#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Openings {
    pub proof: Proof3,
    pub evals: Vec<Eval>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proof3 {
    pub lr: Vec<Vec<Vec<String>>>,
    pub z_1: String,
    pub z_2: String,
    pub delta: Vec<String>,
    pub sg: Vec<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Eval {
    pub l: Vec<String>,
    pub r: Vec<String>,
    pub o: Vec<String>,
    pub z: Vec<String>,
    pub t: Vec<String>,
    pub f: Vec<String>,
    pub sigma1: Vec<String>,
    pub sigma2: Vec<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub data: Option<(String, Data)>,
    pub status: Option<(String, Status, Status2)>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Data {
    pub payload: Payload,
    pub signer: String,
    pub signature: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub common: Common,
    pub body: (String, Body2),
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Common {
    pub fee: String,
    pub fee_token: String,
    pub fee_payer_pk: String,
    pub nonce: String,
    pub valid_until: String,
    pub memo: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Body2 {
    pub source_pk: String,
    pub receiver_pk: String,
    pub token_id: String,
    pub amount: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub fee_payer_account_creation_fee_paid: Value,
    pub receiver_account_creation_fee_paid: Option<String>,
    pub created_token: Value,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status2 {
    pub fee_payer_balance: String,
    pub source_balance: String,
    pub receiver_balance: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalCommandBalances {
    pub receiver1_balance: Option<String>,
    pub receiver2_balance: Option<String>,
    pub coinbase_receiver_balance: Option<String>,
    pub fee_transfer_receiver_balance: Option<Value>,
}

pub fn parse_file<P: AsRef<Path>>(filename: P) -> anyhow::Result<PrecomputedBlock> {
    let data = std::fs::read(filename)?;
    let str = String::from_utf8_lossy(&data);
    let block: PrecomputedBlock = serde_json::from_str(&str)?;
    Ok(block)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn foobar() -> anyhow::Result<()> {
        let path = "/Users/tbernard/mainnet-317352-3NLL8Xn79QDXJNwYzCkBSWcCiV5YFGJC5Lcq8rURYqCxt2a7aDww.json";
        //let path = "/Users/tbernard/mainnet-2-3NLyWnjZqUECniE1q719CoLmes6WDQAod4vrTeLfN7XXJbHv6EHH.json";
        let now = Instant::now();
        let data = std::fs::read(path)?;
        let str = String::from_utf8_lossy(&data);
        let block: PrecomputedBlock = serde_json::from_str(&str)?;
        //let _ = parse_file(path);
        //println!("{}", block.protocol_state_proof);
        println!("Elapsed time: {:?}", now.elapsed());
        Ok(())
    }
}
