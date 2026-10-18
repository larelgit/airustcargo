//! Delta debugging (Zeller's `ddmin`): shrinks a list that makes something fail to a small
//! list that still does.
//!
//! The fuzzer's session is a list of rounds (input calls and a step length). When a round ends in
//! a broken rule, [`minimize`] looks for the shortest sub-list of rounds that breaks the same rule
//! on a fresh game. The function is pure: the caller decides what "still fails" means, and each
//! attempt is whatever it costs there (a fresh test app, a snapshot load).
//!
//! The original list is assumed to fail and is never tried; neither is the empty list. A
//! candidate tried twice is answered from memory and costs no attempt, so `attempts` counts
//! calls of `still_fails`. The result depends only on the list, the predicate and the budget.

use std::collections::BTreeMap;

/// What [`minimize`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minimized<T> {
    /// The smallest failing list found (the original when nothing smaller failed).
    pub items: Vec<T>,
    /// Calls of `still_fails` made.
    pub attempts: u32,
    /// The budget ran out before the list was proven 1-minimal: `items` is the best so far.
    pub exhausted: bool,
}

/// Shrinks `items`, which is assumed to fail, with `ddmin`: split into `n` chunks, try each chunk,
/// then each complement, then double `n`; finish with a one-at-a-time removal pass. At most
/// `budget` calls of `still_fails`; the kept items stay in their original order.
pub fn minimize<T: Clone>(items: &[T], mut still_fails: impl FnMut(&[T]) -> bool, budget: u32) -> Minimized<T> {
    let mut run = Run { items, still_fails: &mut still_fails, budget, attempts: 0, memo: BTreeMap::new(), out_of_budget: false };
    // The current failing candidate, as indices into `items`.
    let mut cur: Vec<usize> = (0..items.len()).collect();
    run.memo.insert(cur.clone(), true);
    let mut n = 2usize;

    'ddmin: while cur.len() >= 2 {
        n = n.min(cur.len());
        let chunks = split(&cur, n);
        for chunk in &chunks {
            match run.test(chunk) {
                Some(true) => {
                    cur = chunk.clone();
                    n = 2;
                    continue 'ddmin;
                }
                Some(false) => {}
                None => break 'ddmin,
            }
        }
        // With two chunks a complement is the other chunk: already tried.
        if n > 2 {
            for i in 0..chunks.len() {
                let complement: Vec<usize> =
                    chunks.iter().enumerate().filter(|(j, _)| *j != i).flat_map(|(_, c)| c.iter().copied()).collect();
                match run.test(&complement) {
                    Some(true) => {
                        cur = complement;
                        n = (n - 1).max(2);
                        continue 'ddmin;
                    }
                    Some(false) => {}
                    None => break 'ddmin,
                }
            }
        }
        if n >= cur.len() {
            break;
        }
        n = (n * 2).min(cur.len());
    }

    // One at a time, until nothing can go. Candidates ddmin tried are answered from memory.
    if !run.out_of_budget {
        'single: while cur.len() >= 2 {
            for i in 0..cur.len() {
                let mut candidate = cur.clone();
                candidate.remove(i);
                match run.test(&candidate) {
                    Some(true) => {
                        cur = candidate;
                        continue 'single;
                    }
                    Some(false) => {}
                    None => break 'single,
                }
            }
            break;
        }
    }

    Minimized { items: cur.iter().map(|&i| items[i].clone()).collect(), attempts: run.attempts, exhausted: run.out_of_budget }
}

struct Run<'a, T> {
    items: &'a [T],
    still_fails: &'a mut dyn FnMut(&[T]) -> bool,
    budget: u32,
    attempts: u32,
    memo: BTreeMap<Vec<usize>, bool>,
    out_of_budget: bool,
}

impl<T: Clone> Run<'_, T> {
    /// Whether the candidate still fails; `None` when the budget is spent.
    fn test(&mut self, indices: &[usize]) -> Option<bool> {
        debug_assert!(!indices.is_empty());
        if let Some(&known) = self.memo.get(indices) {
            return Some(known);
        }
        if self.attempts >= self.budget {
            self.out_of_budget = true;
            return None;
        }
        self.attempts += 1;
        let candidate: Vec<T> = indices.iter().map(|&i| self.items[i].clone()).collect();
        let fails = (self.still_fails)(&candidate);
        self.memo.insert(indices.to_vec(), fails);
        Some(fails)
    }
}

/// `n` consecutive chunks of nearly equal size, none empty (`n <= items.len()`).
fn split(items: &[usize], n: usize) -> Vec<Vec<usize>> {
    let (base, extra) = (items.len() / n, items.len() % n);
    let mut at = 0;
    (0..n)
        .map(|i| {
            let len = base + usize::from(i < extra);
            let chunk = items[at..at + len].to_vec();
            at += len;
            chunk
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_needed_items_among_many_are_all_that_is_left() {
        let items: Vec<u32> = (0..20).collect();
        let found = minimize(&items, |c| c.contains(&3) && c.contains(&7), 1000);
        assert_eq!(found.items, vec![3, 7]);
        assert!(!found.exhausted);
    }

    #[test]
    fn a_sum_threshold_keeps_a_list_that_cannot_lose_an_item() {
        let items = vec![30, 1, 2, 40, 3, 50, 4, 5];
        let found = minimize(&items, |c| c.iter().sum::<u32>() > 100, 1000);
        assert!(found.items.iter().sum::<u32>() > 100);
        for i in 0..found.items.len() {
            let mut less = found.items.clone();
            less.remove(i);
            assert!(less.iter().sum::<u32>() <= 100, "{:?} still fails without item {i}", found.items);
        }
        assert!(found.items.len() < items.len());
    }

    #[test]
    fn a_predicate_that_needs_order_keeps_the_order() {
        let items: Vec<u32> = (0..16).rev().collect();
        // Fails only when 9 comes before 2 (the list is descending, so both survive in that order).
        let found = minimize(
            &items,
            |c| {
                let a = c.iter().position(|&x| x == 9);
                let b = c.iter().position(|&x| x == 2);
                matches!((a, b), (Some(a), Some(b)) if a < b)
            },
            1000,
        );
        assert_eq!(found.items, vec![9, 2]);
    }

    #[test]
    fn a_single_item_that_alone_fails_is_found() {
        let items: Vec<u32> = (0..9).collect();
        assert_eq!(minimize(&items, |c| c.contains(&5), 1000).items, vec![5]);
    }

    #[test]
    fn it_never_asks_about_the_empty_list_or_the_original() {
        let items: Vec<u32> = (0..13).collect();
        let mut asked = Vec::new();
        minimize(
            &items,
            |c| {
                asked.push(c.to_vec());
                c.contains(&4) || c.len() > 9
            },
            1000,
        );
        assert!(!asked.is_empty());
        assert!(asked.iter().all(|c| !c.is_empty() && c.len() < items.len()), "{asked:?}");
    }

    #[test]
    fn a_list_of_one_or_a_list_that_cannot_shrink_is_returned_as_it_is() {
        assert_eq!(minimize(&[7], |_| unreachable!(), 10), Minimized { items: vec![7], attempts: 0, exhausted: false });
        let items = vec![1, 2, 3];
        let found = minimize(&items, |c| c.len() == 3, 100);
        assert_eq!(found.items, items);
        assert!(!found.exhausted);
    }

    #[test]
    fn an_exhausted_budget_returns_the_best_list_so_far() {
        let items: Vec<u32> = (0..64).collect();
        let found = minimize(&items, |c| c.contains(&3) && c.contains(&7), 5);
        assert!(found.exhausted);
        assert_eq!(found.attempts, 5);
        assert!(found.items.contains(&3) && found.items.contains(&7), "the best list still fails");
        assert!(found.items.len() < items.len());
    }

    #[test]
    fn zero_budget_tries_nothing() {
        let items = vec![1, 2, 3, 4];
        let found = minimize(&items, |_| unreachable!(), 0);
        assert_eq!((found.items, found.attempts, found.exhausted), (items, 0, true));
    }

    #[test]
    fn the_attempt_count_and_the_result_are_deterministic_and_never_repeat_a_candidate() {
        let items: Vec<u32> = (0..40).collect();
        let play = || {
            let mut seen = std::collections::BTreeSet::new();
            let found = minimize(
                &items,
                |c| {
                    assert!(seen.insert(c.to_vec()), "{c:?} asked twice");
                    c.contains(&11) && c.contains(&29) && c.contains(&30)
                },
                1000,
            );
            (found.items, found.attempts)
        };
        let first = play();
        assert_eq!(first.0, vec![11, 29, 30]);
        assert_eq!(first, play());
    }
}
