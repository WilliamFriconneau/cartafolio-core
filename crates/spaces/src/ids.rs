//! Ids made on the device, unique and sortable (UUIDv7), and the hybrid
//! logical clock of every event: ready for peers, never a row number nor a
//! path, unchanged by a rename or a move. Portable: std only.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn random_u64() -> u64 {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.finish()
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A UUIDv7 made at `ms` (milliseconds since 1970): sorts by time.
pub fn uuid_v7_at(ms: u64) -> String {
    let a = random_u64();
    let b = random_u64();
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    bytes[6..14].copy_from_slice(&a.to_be_bytes());
    bytes[14..].copy_from_slice(&b.to_be_bytes()[..2]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

pub fn new_id() -> String {
    uuid_v7_at(now_ms())
}

/// A hybrid logical clock stamp: wall time in ms, a counter, the node.
/// Its text form sorts like the stamps.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hlc {
    pub ms: u64,
    pub counter: u32,
    pub node: String,
}

impl Hlc {
    /// The next stamp after `last` (none: the first), at wall time `now_ms`.
    pub fn tick(last: Option<&Hlc>, now_ms: u64, node: &str) -> Hlc {
        match last {
            Some(l) if l.ms >= now_ms => Hlc {
                ms: l.ms,
                counter: l.counter + 1,
                node: node.into(),
            },
            _ => Hlc {
                ms: now_ms,
                counter: 0,
                node: node.into(),
            },
        }
    }

    /// The stamp after receiving `remote` (for the peers, later).
    pub fn receive(&self, remote: &Hlc, now_ms: u64) -> Hlc {
        let ms = self.ms.max(remote.ms).max(now_ms);
        let counter = if ms == self.ms && ms == remote.ms {
            self.counter.max(remote.counter) + 1
        } else if ms == self.ms {
            self.counter + 1
        } else if ms == remote.ms {
            remote.counter + 1
        } else {
            0
        };
        Hlc {
            ms,
            counter,
            node: self.node.clone(),
        }
    }

    pub fn encode(&self) -> String {
        format!("{:013}-{:06}-{}", self.ms, self.counter, self.node)
    }

    pub fn decode(s: &str) -> Option<Hlc> {
        let mut parts = s.splitn(3, '-');
        Some(Hlc {
            ms: parts.next()?.parse().ok()?,
            counter: parts.next()?.parse().ok()?,
            node: parts.next()?.to_string(),
        })
    }
}
