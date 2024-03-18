use crate::ledger::account::Amount;
use csv::Reader;
use serde::Deserialize;
use std::{collections::HashMap, fs::File, path::Path};

#[derive(Debug, Deserialize)]
struct Record {
    slot: u32,
    locked: u64,
}

#[derive(Default)]
pub struct LockedBalances {
    locked: HashMap<u32, Amount>,
}

impl LockedBalances {
    pub fn from_csv<P: AsRef<Path>>(path: Option<P>) -> anyhow::Result<Self> {
        let mut locked = HashMap::new();
        if let Some(path) = path {
            let file = File::open(path)?;
            let mut rdr = Reader::from_reader(file);
            for result in rdr.deserialize() {
                let record: Record = result?;
                locked.insert(record.slot, Amount(record.locked * 1_000_000_000_u64));
            }
        }
        Ok(LockedBalances { locked })
    }

    pub fn get_locked_amount(&self, global_slot: u32) -> Option<Amount> {
        self.locked.get(&global_slot).copied()
    }
}
