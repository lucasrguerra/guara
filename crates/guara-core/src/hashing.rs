//! Hashes de integridade usados em perícia: MD5, SHA-1 e SHA-256 numa única passada.

use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hashes {
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
}

#[derive(Default)]
pub struct MultiHasher {
    md5: Md5,
    sha1: Sha1,
    sha256: Sha256,
}

impl MultiHasher {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn update(&mut self, data: &[u8]) {
        self.md5.update(data);
        self.sha1.update(data);
        self.sha256.update(data);
    }
    pub fn finalize(self) -> Hashes {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        Hashes {
            md5: hex(&self.md5.finalize()),
            sha1: hex(&self.sha1.finalize()),
            sha256: hex(&self.sha256.finalize()),
        }
    }
}

pub fn hash_bytes(data: &[u8]) -> Hashes {
    let mut h = MultiHasher::new();
    h.update(data);
    h.finalize()
}
