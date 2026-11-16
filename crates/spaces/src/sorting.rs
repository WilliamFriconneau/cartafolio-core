//! A sorted note is marked, so that no node sorts it again in a loop: a
//! short lease first (two leases on one note: the oldest wins), then
//! `processed`. Only the person asks for a note to be sorted again.

/// A node holds a note while it sorts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub note: String,
    pub node: String,
    /// Encoded [`crate::Hlc`] of the lease.
    pub hlc: String,
    /// Milliseconds since 1970 after which the lease is void.
    pub expires_ms: u64,
}

/// How long a lease lasts.
pub const LEASE_MS: u64 = 5 * 60 * 1000;

/// The node that holds `note` at `now_ms`: the oldest live lease.
pub fn lease_holder<'a>(leases: &'a [Lease], note: &str, now_ms: u64) -> Option<&'a Lease> {
    leases
        .iter()
        .filter(|l| l.note == note && l.expires_ms > now_ms)
        .min_by(|a, b| a.hlc.cmp(&b.hlc))
}

/// Whether `node` may sort `note` now: not processed (unless the person asked
/// again), and no other node holds it.
pub fn may_sort(
    processed: bool,
    resort_asked: bool,
    leases: &[Lease],
    note: &str,
    node: &str,
    now_ms: u64,
) -> bool {
    (!processed || resort_asked)
        && lease_holder(leases, note, now_ms).is_none_or(|l| l.node == node)
}
