use crate::{
    block::precomputed::PrecomputedBlock,
    command::{Command, Delegation, Payment, UserCommandWithStatus},
    ledger::public_key::PublicKey,
};
use blake2::digest::VariableOutput;
use mina_serialization_types::staged_ledger_diff as mina_rs;
use serde_derive::{Deserialize, Serialize};
use std::io::Write;
use versioned::Versioned;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SignedCommand(pub mina_serialization_types::staged_ledger_diff::SignedCommandV1);

impl SignedCommand {
    pub fn payload(&self) -> &mina_rs::SignedCommandPayload {
        &self.0.t.t.payload.t.t
    }

    pub fn from_user_command(uc: UserCommandWithStatus) -> Self {
        match uc.0.t.data.t.t {
            mina_rs::UserCommand::SignedCommand(signed_command) => signed_command.into(),
        }
    }

    pub fn source_nonce(&self) -> u32 {
        self.0.t.t.payload.t.t.common.t.t.t.nonce.t.t as u32
    }

    pub fn fee_payer(&self) -> PublicKey {
        self.0
            .t
            .t
            .payload
            .t
            .t
            .common
            .t
            .t
            .t
            .fee_payer_pk
            .clone()
            .into()
    }

    pub fn contains_public_key(&self, pk: &PublicKey) -> bool {
        &self.receiver_pk() == pk || &self.source_pk() == pk
    }

    pub fn receiver_pk(&self) -> PublicKey {
        match self.0.t.t.payload.t.t.body.t.t.clone() {
            mina_rs::SignedCommandPayloadBody::PaymentPayload(payment_payload) => {
                payment_payload.t.t.receiver_pk.into()
            }
            mina_rs::SignedCommandPayloadBody::StakeDelegation(delegation_payload) => {
                match delegation_payload.t {
                    mina_rs::StakeDelegation::SetDelegate {
                        delegator: _,
                        new_delegate,
                    } => new_delegate.into(),
                }
            }
        }
    }

    pub fn source_pk(&self) -> PublicKey {
        match self.0.t.t.payload.t.t.body.t.t.clone() {
            mina_rs::SignedCommandPayloadBody::PaymentPayload(payment_payload) => {
                payment_payload.t.t.source_pk.into()
            }
            mina_rs::SignedCommandPayloadBody::StakeDelegation(delegation_payload) => {
                match delegation_payload.t {
                    mina_rs::StakeDelegation::SetDelegate {
                        delegator,
                        new_delegate: _,
                    } => delegator.into(),
                }
            }
        }
    }

    pub fn is_delegation(&self) -> bool {
        match self.0.t.t.payload.t.t.body.t.t.clone() {
            mina_rs::SignedCommandPayloadBody::PaymentPayload(_payment_payload) => false,
            mina_rs::SignedCommandPayloadBody::StakeDelegation(_delegation_payload) => true,
        }
    }

    pub fn payload_body(&self) -> mina_rs::SignedCommandPayloadBody {
        self.0
            .clone()
            .inner()
            .inner()
            .payload
            .inner()
            .inner()
            .body
            .inner()
            .inner()
    }

    pub fn payload_common(&self) -> mina_rs::SignedCommandPayloadCommon {
        self.0
            .clone()
            .inner()
            .inner()
            .payload
            .inner()
            .inner()
            .common
            .inner()
            .inner()
            .inner()
    }

    pub fn fee_payer_pk(&self) -> PublicKey {
        self.payload_common().fee_payer_pk.into()
    }

    pub fn signer(&self) -> PublicKey {
        self.0.clone().inner().inner().signer.0.inner().into()
    }

    pub fn hash_signed_command(&self) -> anyhow::Result<String> {
        let mut binprot_bytes = Vec::new();
        bin_prot::to_writer(&mut binprot_bytes, &self.0).map_err(anyhow::Error::from)?;

        let binprot_bytes_bs58 = bs58::encode(&binprot_bytes[..])
            .with_check_version(0x13)
            .into_string();
        let mut hasher = blake2::Blake2bVar::new(32).unwrap();

        hasher.write_all(binprot_bytes_bs58.as_bytes()).unwrap();

        let mut hash = hasher.finalize_boxed().to_vec();
        hash.insert(0, hash.len() as u8);
        hash.insert(0, 1);

        Ok(bs58::encode(hash).with_check_version(0x12).into_string())
    }

    pub fn from_precomputed(block: &PrecomputedBlock) -> Vec<Self> {
        block
            .commands()
            .iter()
            .map(|cmd| cmd.0.clone().inner().data.inner().inner())
            .map(|mina_rs::UserCommand::SignedCommand(signed_cmd)| SignedCommand(signed_cmd))
            .collect()
    }
}
impl From<SignedCommand> for Command {
    fn from(value: SignedCommand) -> Command {
        match value
            .0
            .inner()
            .inner()
            .payload
            .inner()
            .inner()
            .body
            .inner()
            .inner()
        {
            mina_rs::SignedCommandPayloadBody::PaymentPayload(payment_payload_v1) => {
                let mina_rs::PaymentPayload {
                    source_pk,
                    receiver_pk,
                    amount,
                    ..
                } = payment_payload_v1.inner().inner();
                Command::Payment(Payment {
                    source: source_pk.into(),
                    receiver: receiver_pk.into(),
                    amount: amount.inner().inner().into(),
                })
            }
            mina_rs::SignedCommandPayloadBody::StakeDelegation(stake_delegation_v1) => {
                let mina_rs::StakeDelegation::SetDelegate {
                    delegator,
                    new_delegate,
                } = stake_delegation_v1.inner();
                Command::Delegation(Delegation {
                    delegate: new_delegate.into(),
                    delegator: delegator.into(),
                })
            }
        }
    }
}

impl From<Versioned<Versioned<mina_rs::SignedCommand, 1>, 1>> for SignedCommand {
    fn from(value: Versioned<Versioned<mina_rs::SignedCommand, 1>, 1>) -> Self {
        SignedCommand(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::block::parse_file;
    use std::path::PathBuf;

    #[tokio::test]
    async fn transaction_hash() {
        // refer to the hashes on Minascan
        // https://minascan.io/mainnet/tx/CkpZDcqGWQVpckXjcg99hh4EzmCrnPzMM8VzHaLAYxPU5tMubuLaj
        // https://minascan.io/mainnet/tx/CkpZZsSm9hQpGkGzMi8rcsQEWPZwGJXktiqGYADNwLoBeeamhzqnX

        let block_file = PathBuf::from("./tests/data/sequential_blocks/mainnet-105489-3NK4huLvUDiL4XuCUcyrWCKynmvhqfKsx5h2MfBXVVUq2Qwzi5uT.json");
        let precomputed_block = parse_file(&block_file).unwrap();
        let hashes = precomputed_block.command_hashes();
        let expect = vec![
            "CkpZZsSm9hQpGkGzMi8rcsQEWPZwGJXktiqGYADNwLoBeeamhzqnX".to_string(),
            "CkpZDcqGWQVpckXjcg99hh4EzmCrnPzMM8VzHaLAYxPU5tMubuLaj".to_string(),
        ];

        assert_eq!(hashes, expect);
    }
}
