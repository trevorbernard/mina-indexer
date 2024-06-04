use super::{username::UsernameStore, IndexerStore};
use crate::store::column_families::ColumnFamilyHelpers;
use log::trace;

impl UsernameStore for IndexerStore {
    fn get_username(
        &self,
        pk: &crate::ledger::public_key::PublicKey,
    ) -> anyhow::Result<Option<String>> {
        trace!("Getting username for {pk}");
        Ok(self
            .database
            .get_pinned_cf(self.username_cf(), pk.0.as_bytes())?
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("username from bytes")))
    }

    fn set_username(
        &self,
        pk: &crate::ledger::public_key::PublicKey,
        username: String,
    ) -> anyhow::Result<()> {
        trace!("Setting username: {pk} -> {username}");
        Ok(self
            .database
            .put_cf(self.username_cf(), pk.0.as_bytes(), username.as_bytes())?)
    }
}
