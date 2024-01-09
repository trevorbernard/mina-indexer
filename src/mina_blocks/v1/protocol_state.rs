use serde_derive::{Deserialize, Serialize};

use super::precomputed_block::{BlockchainState, ConsensusState, Constants};

/// The Protocol State represents a snapshot of the blockchain's current state,
/// including consensus information, network parameters, and references to
/// previous blocks.
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolState {
    /// Represents the unique cryptographic hash of the protocol state
    pub previous_state_hash: String,
    pub body: Body,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Body {
    /// The genesis state hash is the protocol state hash for the genesis
    /// protocol state.
    pub genesis_state_hash: String,
    pub blockchain_state: BlockchainState,
    pub consensus_state: ConsensusState,
    pub constants: Constants,
}
