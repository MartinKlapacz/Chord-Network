use std::fmt::{Debug, Pointer};
use std::fmt;
use simple_logger::init;

use crate::chord::Address;
use crate::chord::chord_proto::{AddressMsg, FingerEntryDebugMsg, FingerEntryMsg, FingerTableMsg, KeyMsg};
use crate::crypto;
use crate::crypto::{HashRingKey, Key};

#[derive(Debug, Clone)]
pub struct FingerTable {
    pub fingers: Vec<FingerEntry>,
}

#[derive(Clone)]
pub struct FingerEntry {
    key: Key,
    address: Address,
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
    pub fn new(key: &Key, address: &Address) -> Self {
        FingerEntry {
            address: address.clone(),
            key: key.clone(),
        }
    }

    pub fn get_key(&self) -> &Key {
        &self.key
    }

    pub fn get_address(&self) -> &Address {
        &self.address
    }

    pub fn get_address_mut(&mut self) -> &mut Address {
        &mut self.address
    }
}

impl Into<FingerEntryMsg> for AddressMsg {
    fn into(self) -> FingerEntryMsg {
        FingerEntryMsg {
            id: crypto::hash(&self.address.as_bytes()).to_be_bytes().to_vec(),
            address: self.address,
        }
    }
}

impl Into<FingerEntryMsg> for &AddressMsg {
    fn into(self) -> FingerEntryMsg {
        self.clone().into()
    }
}

impl Into<AddressMsg> for FingerEntryMsg {
    fn into(self) -> AddressMsg {
        AddressMsg {
            address: self.address,
        }
    }
}

impl Into<AddressMsg> for &FingerEntryMsg {
    fn into(self) -> AddressMsg {
        self.clone().into()
    }
}

impl Into<FingerEntryMsg> for FingerEntry {
    fn into(self) -> FingerEntryMsg {
        FingerEntryMsg {
            id: self.key.to_be_bytes().to_vec(),
            address: self.address,
        }
    }
}

impl Into<FingerEntryMsg> for &FingerEntry {
    fn into(self) -> FingerEntryMsg {
        self.clone().into()
    }
}


impl Into<FingerEntry> for FingerEntryMsg {
    fn into(self) -> FingerEntry {
        FingerEntry {
            key: Key::from_be_bytes(self.id.try_into().unwrap()),
            address: self.address,
        }
    }
}

impl Into<FingerEntry> for &FingerEntryMsg {
    fn into(self) -> FingerEntry {
        self.clone().into()
    }
}


impl Into<AddressMsg> for FingerEntry {
    fn into(self) -> AddressMsg {
        AddressMsg {
            address: self.address,
        }
    }
}

impl Into<AddressMsg> for &FingerEntry {
    fn into(self) -> AddressMsg {
        self.clone().into()
    }
}


impl Into<FingerEntry> for AddressMsg {
    fn into(self) -> FingerEntry {
        FingerEntry {
            key: crypto::hash(&self.address.as_bytes()),
            address: self.address,
        }
    }
}

impl Into<FingerEntry> for &AddressMsg {
    fn into(self) -> FingerEntry {
        self.clone().into()
    }
}

impl Into<AddressMsg> for Address {
    fn into(self) -> AddressMsg {
        AddressMsg {
            address: self,
        }
    }
}

impl Into<AddressMsg> for &Address {
    fn into(self) -> AddressMsg {
        self.clone().into()
    }
}

impl Into<Address> for AddressMsg {
    fn into(self) -> Address {
        self.address
    }
}

impl Into<Address> for &AddressMsg {
    fn into(self) -> Address {
        self.clone().into()
    }
}

impl Into<KeyMsg> for AddressMsg {
    fn into(self) -> KeyMsg {
        KeyMsg {
            key: crypto::hash(&self.address.as_bytes()).to_be_bytes().to_vec()
        }
    }
}

impl Into<KeyMsg> for &AddressMsg {
    fn into(self) -> KeyMsg {
        self.clone().into()
    }
}

impl Into<KeyMsg> for Key {
    fn into(self) -> KeyMsg {
        KeyMsg {
            key: self.to_be_bytes().to_vec()
        }
    }
}

impl Into<KeyMsg> for &Key {
    fn into(self) -> KeyMsg {
        self.clone().into()
    }
}


impl Into<Key> for KeyMsg {
    fn into(self) -> Key {
        Key::from_be_bytes(self.key.try_into().unwrap())
    }
}

impl Into<Key> for &KeyMsg {
    fn into(self) -> Key {
        self.clone().into()
    }
}

impl Into<KeyMsg> for &mut FingerEntry {
    fn into(self) -> KeyMsg {
        KeyMsg {
            key: self.key.to_be_bytes().to_vec(),
        }
    }
}

impl Into<FingerEntry> for Address {
    fn into(self) -> FingerEntry {
        FingerEntry {
            key: crypto::hash(&self.as_bytes()),
            address: self,
        }
    }
}

impl Into<FingerEntry> for &Address {
    fn into(self) -> FingerEntry {
        self.clone().into()
    }
}

impl Into<Key> for FingerEntry {
    fn into(self) -> Key {
        Key::from_be_bytes(self.key.to_be_bytes())
    }
}

impl Into<Key> for &FingerEntry {
    fn into(self) -> Key {
        self.clone().into()
    }
}

impl Into<FingerEntryDebugMsg> for FingerEntry {
    fn into(self) -> FingerEntryDebugMsg {
        FingerEntryDebugMsg {
            id: self.key.to_string(),
            address: self.address,
        }
    }
}
impl Into<FingerEntryDebugMsg> for &FingerEntry {
    fn into(self) -> FingerEntryDebugMsg {
        self.clone().into()
    }
}


impl FingerTable {
    pub fn new(key: &Key, address: &Address) -> FingerTable {
        let mut fingers = Vec::new();
        for i in 0..Key::finger_count() {
            fingers.push(FingerEntry {
                // key: (key + 2u128.pow(i as u32)) % 2u128.pow(finger_count as u32),
                key: key.overflowing_add(Key::one().overflowing_shl(i as u32).0).0,
                address: address.clone(),
            });
        };
        FingerTable { fingers }
    }

    pub fn set_finger(&mut self, index: usize, address: Address) -> () {
        self.fingers[index].address = address;
    }

    pub fn set_all_fingers(&mut self, address: &Address) -> () {
        for mut finger in &mut self.fingers {
            finger.address = address.clone();
        }
    }
}

impl Into<FingerTableMsg> for FingerTable {
    fn into(self) -> FingerTableMsg {
        let mut fingers = Vec::new();
        for finger in self.fingers {
            fingers.push(finger.into());
        }
        FingerTableMsg { fingers }
    }
}



