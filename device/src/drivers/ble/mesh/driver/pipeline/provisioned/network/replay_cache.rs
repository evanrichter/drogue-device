use crate::drivers::ble::mesh::address::UnicastAddress;
use uluru::LRUCache;

#[derive(PartialEq)]
struct CacheEntry {
    seq: u32,
    src: UnicastAddress,
    iv_index: u16,
}

pub struct ReplayCache {
    lru: LRUCache<CacheEntry, 10>,
}

impl Default for ReplayCache {
    fn default() -> Self {
        Self {
            lru: Default::default(),
        }
    }
}

impl ReplayCache {
    pub fn has_seen(&mut self, iv_index: u32, seq: u32, src: UnicastAddress) -> bool {
        let iv_index = (iv_index & 0xFFFF) as u16;

        if let Some(entry) = self.lru.find(|e| e.src == src) {
            if iv_index < entry.iv_index {
                true
            } else if iv_index == entry.iv_index {
                if seq <= entry.seq {
                    true
                } else {
                    entry.seq = seq;
                    false
                }
            } else {
                entry.iv_index = iv_index;
                entry.seq = seq;
                false
            }
        } else {
            self.lru.insert(CacheEntry { seq, src, iv_index });
            false
        }
    }
}
