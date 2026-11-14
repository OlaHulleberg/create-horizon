//! Create's nugget of experience (`ExperienceNuggetItem.use`): the stack (one when sneaking)
//! thrown out as experience orbs, 3 points a nugget, in one orb for one nugget, else five.

/// The orbs' values for `nuggets` used (`total = ceil(3 × amount)`, `valuePer = max(1, 1 + total /
/// maxOrbs)`, the last ones taking what is left; orbs of 0 skipped). As in Create, an orb past
/// the total takes the difference: below zero for some counts (2 nuggets: 2, 2, 2, -2).
pub fn orbs(nuggets: u32) -> Vec<i32> {
    let total = 3 * nuggets as i64;
    let max_orbs = if nuggets == 1 { 1 } else { 5 };
    let per = (1 + total / max_orbs).max(1);
    (0..max_orbs)
        .map(|i| per.min(total - i * per))
        .filter(|v| *v != 0)
        .map(|v| v as i32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuggets_become_three_points_each() {
        assert_eq!(orbs(1), vec![3]);
        // 64: 192 points, 39 an orb, the last 36.
        assert_eq!(orbs(64), vec![39, 39, 39, 39, 36]);
        assert_eq!(orbs(2), vec![2, 2, 2, -2]);
        assert_eq!(orbs(64).iter().sum::<i32>(), 192);
    }
}
