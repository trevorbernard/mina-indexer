use crate::{
    block::{signed_command, BlockHash},
    state::{
        ledger::{
            command::{self, PaymentPayload, UserCommandWithStatus},
            public_key::PublicKey,
        },
        Canonicity,
    },
    MAINNET_GENESIS_TIMESTAMP,
};
use mina_serialization_types::{
    json::DeltaTransitionChainProofJson,
    protocol_state::{ProtocolState, ProtocolStateJson},
    protocol_state_proof::ProtocolStateProofBase64Json,
    staged_ledger_diff::{
        self, InternalCommandBalanceData, SignedCommandPayloadBody, StagedLedgerDiff,
        StagedLedgerDiffJson, StakeDelegation, UserCommand,
    },
    v1::{DeltaTransitionChainProof, ProtocolStateProofV1, UserCommandWithStatusV1},
};
use serde::{Deserialize, Serialize};

pub struct BlockFileContents {
    pub(crate) state_hash: String,
    pub(crate) blockchain_length: Option<u32>,
    pub(crate) contents: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BlockFile {
    #[serde(default = "genesis_timestamp")]
    scheduled_time: String,
    protocol_state: ProtocolStateJson,
    protocol_state_proof: ProtocolStateProofBase64Json,
    staged_ledger_diff: StagedLedgerDiffJson,
    delta_transition_chain_proof: DeltaTransitionChainProofJson,
}

fn genesis_timestamp() -> String {
    MAINNET_GENESIS_TIMESTAMP.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrecomputedBlock {
    pub canonicity: Option<Canonicity>,
    pub state_hash: String,
    pub scheduled_time: String,
    pub protocol_state: ProtocolState,
    pub blockchain_length: u32,
    pub protocol_state_proof: ProtocolStateProofV1,
    pub staged_ledger_diff: StagedLedgerDiff,
    pub delta_transition_chain_proof: DeltaTransitionChainProof,
}

impl PrecomputedBlock {
    pub fn from_file_contents(log_contents: BlockFileContents) -> serde_json::Result<Self> {
        let state_hash = log_contents.state_hash;
        let str = String::from_utf8_lossy(&log_contents.contents);
        let BlockFile {
            scheduled_time,
            protocol_state,
            protocol_state_proof,
            staged_ledger_diff,
            delta_transition_chain_proof,
        } = serde_json::from_str::<BlockFile>(&str).unwrap();
        let blockchain_length = if let Some(blockchain_length) = log_contents.blockchain_length {
            blockchain_length
        } else {
            protocol_state.body.consensus_state.blockchain_length.0
        };
        Ok(Self {
            canonicity: None,
            state_hash,
            scheduled_time,
            blockchain_length,
            protocol_state: protocol_state.into(),
            protocol_state_proof: protocol_state_proof.into(),
            staged_ledger_diff: staged_ledger_diff.into(),
            delta_transition_chain_proof: delta_transition_chain_proof.into(),
        })
    }

    pub fn commands(&self) -> Vec<UserCommandWithStatusV1> {
        self.staged_ledger_diff
            .diff
            .clone()
            .inner()
            .0
            .inner()
            .inner()
            .commands
    }

    pub fn block_creator(&self) -> PublicKey {
        self.protocol_state
            .body
            .clone()
            .inner()
            .inner()
            .consensus_state
            .inner()
            .inner()
            .block_creator
            .into()
    }

    fn internal_command_balances(&self) -> Vec<InternalCommandBalanceData> {
        self.staged_ledger_diff
            .diff
            .clone()
            .inner()
            .0
            .inner()
            .inner()
            .internal_command_balances
            .iter()
            .map(|x| x.t.clone())
            .collect()
    }

    pub fn coinbase_receiver_balance(&self) -> Option<u64> {
        for internal_balance in self.internal_command_balances() {
            if let InternalCommandBalanceData::CoinBase(x) = internal_balance {
                return Some(x.t.coinbase_receiver_balance.t.t.t);
            }
        }

        None
    }

    pub fn fee_transfer_receiver1_balance(&self) -> Option<u64> {
        for internal_balance in self.internal_command_balances() {
            if let InternalCommandBalanceData::FeeTransfer(x) = internal_balance {
                return Some(x.t.receiver1_balance.t.t.t);
            }
        }

        None
    }

    pub fn fee_transfer_receiver2_balance(&self) -> Option<u64> {
        for internal_balance in self.internal_command_balances() {
            if let InternalCommandBalanceData::FeeTransfer(x) = internal_balance {
                return x.t.receiver2_balance.map(|balance| balance.t.t.t);
            }
        }

        None
    }

    pub fn coinbase_receiver(&self) -> PublicKey {
        self.protocol_state
            .body
            .clone()
            .inner()
            .inner()
            .consensus_state
            .inner()
            .inner()
            .coinbase_receiver
            .into()
    }

    pub fn block_public_keys(&self) -> Vec<PublicKey> {
        let mut public_keys: Vec<PublicKey> = vec![];
        let consenesus_state = self
            .protocol_state
            .body
            .clone()
            .inner()
            .inner()
            .consensus_state
            .inner()
            .inner();
        public_keys.append(&mut vec![
            consenesus_state.block_creator.into(),
            consenesus_state.coinbase_receiver.into(),
            consenesus_state.block_stake_winner.into(),
        ]);

        let commands = self.commands();
        commands
            .iter()
            .filter(|&command| UserCommandWithStatus(command.clone()).is_applied())
            .for_each(|command| {
                let signed_command = match UserCommandWithStatus(command.clone()).data() {
                    staged_ledger_diff::UserCommand::SignedCommand(signed_command) => {
                        command::SignedCommand(signed_command)
                    }
                };
                public_keys.push(signed_command.signer());
                public_keys.push(signed_command.fee_payer_pk());
                public_keys.append(&mut match signed_command.payload_body() {
                    SignedCommandPayloadBody::PaymentPayload(payment_payload) => vec![
                        PaymentPayload(payment_payload.clone()).source_pk(),
                        PaymentPayload(payment_payload).receiver_pk(),
                    ],
                    SignedCommandPayloadBody::StakeDelegation(stake_delegation) => {
                        match stake_delegation.inner() {
                            StakeDelegation::SetDelegate {
                                delegator,
                                new_delegate,
                            } => vec![delegator.into(), new_delegate.into()],
                        }
                    }
                })
            });

        public_keys
    }

    pub fn global_slot_since_genesis(&self) -> u32 {
        self.protocol_state
            .body
            .t
            .t
            .consensus_state
            .t
            .t
            .global_slot_since_genesis
            .t
            .t
    }

    pub fn timestamp(&self) -> u64 {
        self.protocol_state
            .body
            .clone()
            .inner()
            .inner()
            .blockchain_state
            .inner()
            .inner()
            .timestamp
            .inner()
            .inner()
    }

    pub fn previous_state_hash(&self) -> BlockHash {
        BlockHash::from_hashv1(self.protocol_state.previous_state_hash.clone())
    }

    pub fn transaction_hashes(&self) -> Vec<String> {
        self.commands()
            .iter()
            .map(|commandv1| {
                let UserCommand::SignedCommand(signed_commandv1) = commandv1.t.data.t.t.clone();
                signed_command::SignedCommand(signed_commandv1)
                    .hash_signed_command()
                    .unwrap()
            })
            .collect()
    }
}
