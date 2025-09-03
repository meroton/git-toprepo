use anyhow::Result;
use anyhow::anyhow;
use git_gr_lib::change::NewChange;
use itertools::Itertools;
use std::collections::HashMap;

// Order submitted_together
//
// Gerrit returns a partially ordered list of commits.
// Where topics are clearly delineated and dependency order internal to repos.
// But across repos we have no order.
//
// Repos:    A     B     C     D
// Commits
//
// New       A3 -------------- D3
//  |                          D2
//  v        A2 -- B2 -- C1 -- D1
//           A1    B1
// Old
// Gerrit reports the commits submitted together:
//     [A3**, A2*, A1, B2*, B1, C1*, D3**, D2, D1*]
// The asterisks mark commits with topics.
// We have decided to use forward chronological order for this algorithm,
//
// We can split this to the (partially arbitrarily) ordered list of lists:
//     [[A3, D3], [D2], [A2, B2, C1, D1], [A1], [B1]]
//
// The order between A1 and B1 is arbitrary, we can choose lexicographic order
// on the repository name.
//
// Multiple commits within the same repo may also be in
// the same topic and should also be treated as an atomic commit.
//
// Repos:    A     B
// Commits
//
// New       A2
//  |        |
//  v        A1 -- B1
//
// Old
// Gerrit reports this as [A2, A1, B1], they all have the same topic
// so we group them into [[A2, A1, B1]].
// This means there is only one topic to cherry-pick, but inside there are two
// commits that must be cherry-picked. First A1 and then A2.
// This is a little annoying to workaround.
// TODO: Introduce another vector-layer for order within repositories.

/// A small data view for this algorithm, to help in testing.
/// Real data should use a small `From<Real Data>` impl.
/// And then reconstruct the structure based on the id.
#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct SubmittedTogether<T>
where
    T: Eq + std::hash::Hash,
{
    id: String,
    topic: Option<String>,
    secondary: T,
}

// https://stackoverflow.com/a/78372188
pub trait VecInto<D> {
    fn vec_into(self) -> Vec<D>;
}

impl<E, D> VecInto<D> for Vec<E>
where
    D: From<E>,
{
    fn vec_into(self) -> Vec<D> {
        self.into_iter().map(std::convert::Into::into).collect()
    }
}

impl From<NewChange> for SubmittedTogether<String> {
    fn from(c: NewChange) -> Self {
        Self {
            id: c.triplet_id().to_string(),
            topic: c.topic,
            secondary: c.project,
        }
    }
}

pub fn order_submitted_together(cons: Vec<NewChange>) -> Result<Vec<Vec<NewChange>>> {
    let substrate: Vec<SubmittedTogether<String>> = cons.clone().vec_into();
    let restoration = cons
        .iter()
        .map(|c| (c.triplet_id().to_string(), c))
        .collect::<HashMap<String, &NewChange>>();
    let reordered = reorder_submitted_together(&substrate)?;

    let mut res: Vec<Vec<NewChange>> = Vec::new();
    for inner in reordered.into_iter() {
        let mut changes = Vec::new();
        for x in inner.into_iter() {
            let found = restoration.get(&x.id).unwrap(); //.ok_or(Err(anyhow!("Could not restore commit: {}", x.id)))?;
            changes.push((**found).clone());
        }
        res.push(changes);
    }

    Ok(res)
}

/// This assumes that the input is also grouped based on the secondary key.
/// But the key is still used by the algorithm to chunk into iterators.
/// `Cons` should have a reverse chronological order within each grouping.
/// The result will retain this order.
pub fn reorder_submitted_together<T>(
    cons: &[SubmittedTogether<T>],
) -> Result<Vec<Vec<SubmittedTogether<T>>>>
where
    T: Eq + std::hash::Hash + Clone + std::fmt::Debug,
{
    let mut count = 0;
    let mut topic_backlinks: HashMap<String, Vec<&SubmittedTogether<T>>> = HashMap::new();
    for c in cons.iter() {
        count += 1;
        if let Some(topic) = c.topic.clone() {
            topic_backlinks.entry(topic).or_default().push(c)
        }
    }

    // TODO: see if there is a better solution.
    let mut res: Vec<Vec<SubmittedTogether<T>>> = Vec::new();
    if cons.is_empty() {
        return Ok(res);
    }
    let mut iter = cons.iter();
    let mut grouped = vec![vec![iter.next().unwrap()]];
    let mut outer = 0;

    for head in iter {
        let inner = grouped[outer].len() - 1;
        match head.secondary == grouped[outer][inner].secondary {
            true => grouped[outer].push(head),
            false => {
                grouped.push(Vec::new());
                outer += 1;
                grouped[outer].push(head);
            }
        }
    }

    // Successively iterate through all the secondary groupings and pop all "free" commits.
    // Then when all groupings have a topic barrier (or if they are empty they
    // are no longer part of this iteration).
    // Match the first topic in topological order.

    // An ordered list of iterators into the different repositories.
    let mut iters = Vec::new();
    let mut slots = HashMap::<&T, usize>::new();
    for (i, inner) in grouped.into_iter().enumerate() {
        let mut iter = inner.into_iter().peekable();
        slots.insert(&iter.peek().unwrap().secondary, i);
        iters.push(iter);
    }

    let mut iteration_limit = 1000;
    let mut index = 0;
    loop {
        if iteration_limit == 0 {
            break;
        }
        iteration_limit -= 1;

        let slot = index % iters.len();

        let candidate = iters[slot].peek();
        if candidate.is_none() {
            index += 1;
            continue;
        }
        let candidate = candidate.unwrap();

        if candidate.topic.is_none() {
            res.push(vec![iters[slot].next().unwrap().clone()]);
            // Continue and try the same slot again, do not increment index.
            continue;
        }

        // Topic handling is a little more involved. We then need to look
        // across all the iterators to understand whether we are good to
        // finalize a topic.

        let topic = candidate.topic.clone().unwrap();
        let looking_for: &Vec<&SubmittedTogether<T>> = &topic_backlinks[&topic];
        let within = looking_for.iter().map(|e| &e.secondary).unique();

        let mut ok = true;
        for secondary in within {
            ok &=
                iters[slots[secondary]].peek().and_then(|h| h.topic.clone()) == Some(topic.clone());
        }
        if !ok {
            // We do not have the topics available in our iterator heads.
            // continue with more work in other repos.
            index += 1;
            continue;
        }

        // All the necessary topics are on the heads. Possibly stacked within an
        // iterator.
        // We can now pop them.
        let mut commits = Vec::new();
        for commit in looking_for {
            let head = iters[slots[&commit.secondary]].next().unwrap();
            if head.topic != commit.topic {
                return Err(anyhow!(
                    "Unexpected non-topic commit, expected a topic in this repo."
                ));
            }
            commits.push(head.clone())
        }

        res.push(commits);
        index += 1;
    }

    let mut res_count = 0;
    for outer in res.iter() {
        for _ in outer.iter() {
            res_count += 1;
        }
    }

    assert_eq!(count, res_count, "Not all commits are accounted for");

    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(id: &str, topic: Option<&str>, secondary: i32) -> SubmittedTogether<i32> {
        SubmittedTogether::<i32> {
            id: id.to_owned(),
            topic: topic.map(|s| s.to_owned()),
            secondary,
        }
    }

    #[test]
    fn no_topic() {
        let a = new("first", None, 1);
        let b = new("second", None, 2);

        let res = reorder_submitted_together(&[a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a], vec![b]]);
    }

    #[test]
    fn only_topic() {
        let topic = Some("topic");
        let a = new("first", topic, 1);
        let b = new("second", topic, 2);

        let res = reorder_submitted_together(&[a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a, b]]);
    }

    #[test]
    fn topic_in_same_repo() {
        let topic = Some("topic");
        let shared_secondary = 2;
        let a = new("first", topic, shared_secondary);
        let b = new("second", topic, shared_secondary);

        let res = reorder_submitted_together(&[b.clone(), a.clone()]);
        assert_eq!(res.unwrap(), vec![vec![b, a]]);
    }

    #[test]
    fn under_topic() {
        let topic = Some("topic");
        let shared_secondary = 2;

        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let u = new("under", None, shared_secondary);

        let res = reorder_submitted_together(&[a.clone(), b.clone(), u.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a, b], vec![u]]);
    }

    #[test]
    fn over_topic() {
        let topic = Some("topic");
        let shared_secondary = 2;
        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let o = new("over", None, shared_secondary);

        let res = reorder_submitted_together(&[a.clone(), o.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![o], vec![a, b]]);
    }

    #[test]
    fn topic_hamburger() {
        let topic = Some("topic");
        let other_topic = Some("other_topic");
        let shared_secondary = 2;
        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let m = new("middle", None, shared_secondary);
        let c = new("fourth", other_topic, shared_secondary);
        let d = new("fifth", other_topic, 3);

        let res =
            reorder_submitted_together(&[a.clone(), c.clone(), m.clone(), b.clone(), d.clone()]);
        assert_eq!(res.unwrap(), vec![vec![c, d], vec![m], vec![a, b]]);
    }

    #[test]
    fn two_topics() {
        let topic = Some("topic");
        let other_topic = Some("other_topic");
        let shared_secondary = 2;
        let at = new("first_on_top", other_topic, 1);
        let bu = new("first_under", topic, shared_secondary);
        let bt = new("second_on_top", other_topic, shared_secondary);
        let cu = new("second_under", topic, 3);

        let res = reorder_submitted_together(&[at.clone(), bt.clone(), bu.clone(), cu.clone()]);
        assert_eq!(res.unwrap(), vec![vec![at, bt], vec![bu, cu]]);
    }

    #[test]
    fn stacked_commits_in_topic() {
        let topic = Some("topic");
        let shared_secondary = 1;
        let a = new("under", topic, shared_secondary);
        let b = new("on_top", topic, shared_secondary);
        let c = new("other", topic, 2);

        let res = reorder_submitted_together(&[b.clone(), a.clone(), c.clone()]);
        assert_eq!(res.unwrap(), vec![vec![b, a, c]]);
    }

    #[test]
    fn fail_no_topic_inside_a_stacked_topic() {
        let topic = Some("topic");
        let shared_secondary = 1;
        let a = new("under", topic, shared_secondary);
        let b = new("interloper", None, shared_secondary);
        let c = new("on_top", topic, shared_secondary);

        let res = reorder_submitted_together(&[c.clone(), b.clone(), a.clone()]);
        assert!(res.is_err());
    }

    #[test]
    fn fail_scrambled_commits_in_repos() {
        let shared_secondary = 1;
        let a = new("first", None, shared_secondary);
        let b = new("other", None, 2);
        let c = new("also_first", None, shared_secondary);

        let res = reorder_submitted_together(&[a.clone(), b.clone(), c.clone()]);
        // TODO: This should fail! Cannot compute.
        /*
        assert!(res.is_err())
        */
        let _ = res;
    }
}
