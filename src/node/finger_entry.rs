use std::fmt::Debug;
use std::fmt;
use serde::Serialize;
use crate::utils::types::{Address, HashPos};


/// An entry in the FingerTable
#[derive(Clone, Default, Serialize)]
pub struct FingerEntry {
    pub(crate) key: HashPos,
    pub(crate) address: Address,
}

impl Debug for FingerEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("")
            .field("key", &self.key)
            .field("address", &self.address)
            .finish()
    }
}

impl FingerEntry {
    pub fn new(key: &HashPos, address: &Address) -> Self {
        FingerEntry {
            address: address.clone(),
            key: key.clone(),
        }
    }

    pub fn get_key(&self) -> &HashPos {
        &self.key
    }

    pub fn get_address(&self) -> &Address {
        &self.address
    }

    pub fn get_address_mut(&mut self) -> &mut Address {
        &mut self.address
    }
}
