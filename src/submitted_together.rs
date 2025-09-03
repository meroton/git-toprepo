use anyhow::Result;
use anyhow::anyhow;
use itertools::Itertools;
use std::collections::HashMap;

// Order submitted_together
//
// Gerrit returns a partially ordered list of commits.
// Where topics are clearly delineated and dependency order internal to repos.
// But the across repos we have no order.
//
// Repos:    A     B     C     D
// Commits
//    |      A1    B1
//    v      A2 -- B2 -- C1 -- D1
//                             D2
//           A3 -------------- D3
// Gerrit reports the commits submitted together:
//     [A1, A2*, A3**, B1, B2*, C1*, D1*, D2, D3**]
// The asterisks mark commits with topics.
//
// We can split this to the (partially arbitrarily) ordered list of lists:
//     [[A1], [B1], [A2, B2, C1, D1], [D2], [A3, D3]]
// The order between A1 and B1 is arbitrary, we can choose lexicographic order
// on the repository name.
//
// It is less common, but multiple commits within the same repo may also be in
// the same topic and should also be treated as an atomic commit.

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

pub fn order_submitted_together<T>(
    cons: Vec<SubmittedTogether<T>>,
) -> Result<Vec<Vec<SubmittedTogether<T>>>>
where
    T: Eq + std::hash::Hash + Clone + std::fmt::Debug,
{
    /*
    // first group based on secondary, it is possible that we could rely on this
    // being done for us. In which case we save a lot of effort.
    // Then we could just chunk the `cons` input based on `T` directly into
    // Vec<Vec< >>.
    let mut grouped: HashMap<T, Vec<SubmittedTogether<T>>> = HashMap::new();
    for x in cons.into_iter() {
        grouped.entry(x.secondary.clone())
            .or_insert_with(Vec::new)
            .push(x);
    }

    let key_order = grouped.keys().sorted_unstable();
    */

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

    // Find topic order. PartialOrd can be found within each grouping.
    #[derive(Debug)]
    struct PartialOrd {
        before: String,
        after: String,
    }
    let mut ords = Vec::new();

    for repo in grouped.iter() {
        let sentinel = "".to_owned();
        let mut last = sentinel;
        for commit in repo.iter() {
            match (commit.topic.clone(), last.clone().as_ref()) {
                (Some(t), "") => {
                    last = t;
                }
                (Some(after), before) => {
                    last = after.clone();
                    ords.push(PartialOrd {
                        before: before.to_owned(),
                        after,
                    });
                }
                (None, _) => (),
            }
        }
    }
    ords.retain(|e| e.before != e.after);

    // Successively iterate through all the secondary groupings and pop all "free" commits.
    // Then when all groupings have a topic barrier (or if they are empty they
    // are no longer part of this iteration).
    // Match the first topic in topological order.

    println!("> grouped ===\n{grouped:?}");

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

        let res = order_submitted_together(vec![a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a], vec![b]]);
    }

    #[test]
    fn only_topic() {
        let topic = Some("topic");
        let a = new("first", topic, 1);
        let b = new("second", topic, 2);

        let res = order_submitted_together(vec![a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a, b]]);
    }

    #[test]
    fn topic_in_same_repo() {
        let topic = Some("topic");
        let shared_secondary = 2;
        let a = new("first", topic, shared_secondary);
        let b = new("second", topic, shared_secondary);

        let res = order_submitted_together(vec![a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a, b]]);
    }

    #[test]
    fn under_topic() {
        let topic = Some("topic");
        let shared_secondary = 2;

        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let u = new("under", None, shared_secondary);

        let res = order_submitted_together(vec![u.clone(), a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![u], vec![a, b]]);
    }

    #[test]
    fn over_topic() {
        let topic = Some("topic");
        let shared_secondary = 2;
        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let o = new("over", None, shared_secondary);

        let res = order_submitted_together(vec![a.clone(), b.clone(), o.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a, b], vec![o]]);
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
            order_submitted_together(vec![a.clone(), b.clone(), m.clone(), c.clone(), d.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a, b], vec![m], vec![c, d]]);
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

        let res = order_submitted_together(vec![at.clone(), bu.clone(), bt.clone(), cu.clone()]);
        assert_eq!(res.unwrap(), vec![vec![bu, cu], vec![at, bt]]);
    }

    #[test]
    fn stacked_commits_in_topic() {
        let topic = Some("topic");
        let shared_secondary = 1;
        let a = new("under", topic, shared_secondary);
        let b = new("on_top", topic, shared_secondary);
        let c = new("other", topic, 2);

        let res = order_submitted_together(vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(res.unwrap(), vec![vec![a, b, c]]);
    }

    #[test]
    fn fail_no_topic_inside_a_stacked_topic() {
        let topic = Some("topic");
        let shared_secondary = 1;
        let a = new("under", topic, shared_secondary);
        let b = new("interloper", None, shared_secondary);
        let c = new("on_top", topic, shared_secondary);

        let res = order_submitted_together(vec![a.clone(), b.clone(), c.clone()]);
        // TODO: This should fail! Cannot compute.
        assert!(res.is_err());
    }

    #[test]
    fn fail_scrambled_commits_in_repos() {
        let shared_secondary = 1;
        let a = new("first", None, shared_secondary);
        let b = new("other", None, 2);
        let c = new("also_first", None, shared_secondary);

        let res = order_submitted_together(vec![a.clone(), b.clone(), c.clone()]);
        // TODO: This should fail! Cannot compute.
        assert_eq!(res.unwrap(), vec![vec![a], vec![b], vec![c]]);
    }
}
