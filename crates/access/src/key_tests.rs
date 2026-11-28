use crate::key::find_match;
use subtle::ConstantTimeEq;

#[test]
fn a_wrong_key_is_refused_in_constant_time_shape() {
    let hashes = [[1u8; 32], [2; 32], [3; 32]];
    for (provided, expected) in [([1; 32], Some(0)), ([3; 32], Some(2)), ([4; 32], None)] {
        let mut comparisons = 0;
        let matched = find_match(hashes.iter(), &provided, |left, right| {
            comparisons += 1;
            left.ct_eq(right)
        });
        assert_eq!(matched, expected);
        assert_eq!(comparisons, hashes.len());
    }
}
