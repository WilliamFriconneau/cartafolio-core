//! Processed notes are not sorted again in a loop.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_spaces::*;

fn lease(node: &str, ms: u64) -> Lease {
    Lease {
        note: "n1".into(),
        node: node.into(),
        hlc: Hlc {
            ms,
            counter: 0,
            node: node.into(),
        }
        .encode(),
        expires_ms: ms + LEASE_MS,
    }
}

#[test]
fn the_oldest_live_lease_wins() {
    let leases = [lease("mac", 2000), lease("serveur", 1000)];
    assert_eq!(lease_holder(&leases, "n1", 1500).unwrap().node, "serveur");
    assert!(!may_sort(false, false, &leases, "n1", "mac", 1500));
    assert!(may_sort(false, false, &leases, "n1", "serveur", 1500));
    // Expired: anyone may take it.
    assert!(may_sort(
        false,
        false,
        &leases,
        "n1",
        "mac",
        1000 + LEASE_MS + 2000 + 1
    ));
}

#[test]
fn a_processed_note_is_sorted_again_only_when_the_person_asks() {
    assert!(!may_sort(true, false, &[], "n1", "mac", 0));
    assert!(may_sort(true, true, &[], "n1", "mac", 0));
}
