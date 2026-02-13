use anyhow::Result;
use anyhow::anyhow;
use git_gr_lib::change::NewChange;
use itertools::Itertools;
use std::collections::HashMap;

// Order submitted_together
//
// Gerrit returns a partially ordered list of commits.
// Where topics are clearly delineated and dependency order internal to
// repositories.
// But across repos we have no order.
//
//     Repos:    A     B     C     D
//     Commits
//
//     New       A3 -------------- D3
//      |                          D2
//      v        A2 -- B2 -- C1 -- D1
//               A1    B1
//     Old
//
// Gerrit reports the commits submitted together:
//     [A3**, A2*, A1, B2*, B1, C1*, D3**, D2, D1*]
// The asterisks mark commits with topics.
// We have decided to use forward chronological order for this algorithm,
//
// We can split this to the (partially arbitrarily) ordered list of lists:
//     [
//         {[A3], [D3]},
//         {[D2]},
//         {[A2], [B2], [C1], [D1]},
//         {[A1]},
//         {[B1]},
//     ]
// Curly braces are used to indicate the atomic commits and topics,
// for visual clarity.
//
// The order between A1 and B1 is arbitrary, we can choose lexicographic order
// on the repository name. We currently call this the secondary ordering or
// split axis.
//
// Multiple commits within the same repo may also be in
// the same topic and should also be treated as an atomic commit.
// Though it is important to remember the internal order,
// if one wants to cherry-pick them instead of squashing.
//
//     Repos:    A     B
//     Commits
//
//     New       A2
//      |        |
//      v        A1 -- B1
//
//     Old
//
// Gerrit reports this as [A2, A1, B1], they all have the same topic
// so we group them into [{[A2, A1], [B1]}].
// This means there is only one topic to cherry-pick, but inside there are two
// commits that must be cherry-picked. First A1 and then A2.
// This is a little annoying to workaround.
// TODO: Introduce another vector-layer for order within repositories.

#[derive(clap::ValueEnum, PartialEq, Default, Debug, Clone)]
pub enum SupercommitSplitStrategy {
    ForceSquash,
    SquashIfPossible,
    // TODO(nils): take the default through configuration instead.
    #[default]
    Recreate,
}

impl std::fmt::Display for SupercommitSplitStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::ForceSquash => "force-squash",
                Self::SquashIfPossible => "squash-if-possible",
                Self::Recreate => "recreate",
            }
        )
    }
}

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
    repo: T,
    /*
    // Secondary split axis.
    // This is used to order commits that have no
    // order from Gerrit in a consistent way.
    // The commits are grouped in logical order and based on their repos.
    // We currently perform lexicographic ordering on the Repo name,
    // but that could be switched to another split axis.
    secondary: T,
    */
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
            repo: c.project,
        }
    }
}

/// Changes submitted together. This is a Gerrit concept relating to _unmerged_
/// commits. That *would* be submitted together. We partition that from a flat
/// list to a list of topics, with repositories, that contain individual
/// commits.
///
/// Vec<                   > : List of topics
///     Vec<              >  : List of repositories
///         Vec<         >   : List of commits
///             NewChange    : commit
pub struct ChangesSubmittedTogether(pub Vec<Vec<Vec<NewChange>>>);

// TODO: What should this be called?
/// Changes to fetch and filter.
///
/// Vec<                        > : List of topics
///     Vec<                   >  : List of supercommits
///         Vec<              >   : List of repositories
///             Vec<         >    : List of commits
///                 NewChange     : commit
pub struct CherryPickable(pub Vec<Vec<Vec<Vec<NewChange>>>>);

impl ChangesSubmittedTogether {
    // TODO: This must be idempotent!
    //       (Use a different return type, or phantom marker)
    // TODO: add a test!
    //       (It is easier to test in the SubmittedTogether<T> world).
    pub fn chronological_order(self) -> Self {
        ChangesSubmittedTogether(self.0.into_iter().rev().collect())
    }

    #[allow(unused)]
    fn gerrit_order(self) -> Self {
        self
    }
}

pub fn split_by_supercommits(
    to_fetch: ChangesSubmittedTogether,
    strategy: &SupercommitSplitStrategy,
) -> Result<CherryPickable> {
    assert!(strategy == &SupercommitSplitStrategy::ForceSquash);

    // let mut res: Vec<Vec<Vec<Vec<NewChange>>>> = Vec::new();
    let mut res = Vec::new();
    for topic in to_fetch.0.into_iter() {
        let supercommit: Vec<Vec<NewChange>> = topic.clone();
        res.push(vec![supercommit]);
    }

    Ok(CherryPickable(res))
}

pub fn order_submitted_together(cons: Vec<NewChange>) -> Result<ChangesSubmittedTogether> {
    // Create a slimmer data representation for only the relevant fields to the
    // reordering algorithm. Reorder those and then rewrite the original commits
    // from `restoration` into the reordered structure.
    let substrate: Vec<SubmittedTogether<String>> = cons.clone().vec_into();
    let restoration = cons
        .iter()
        .map(|c| (c.triplet_id().to_string(), c))
        .collect::<HashMap<String, &NewChange>>();
    let reordered = reorder_submitted_together(&substrate)?;

    let mut res: Vec<Vec<Vec<NewChange>>> = Vec::new();
    for atomic in reordered.into_iter() {
        let mut repo = Vec::new();
        for readrepo in atomic.into_iter() {
            let mut changes = Vec::new();
            for c in readrepo.into_iter() {
                let found = restoration.get(&c.id).unwrap(); //.ok_or(Err(anyhow!("Could not restore commit: {}", x.id)))?;
                changes.push((**found).clone());
            }
            repo.push(changes);
        }
        res.push(repo);
    }

    Ok(ChangesSubmittedTogether(res))
}

fn group_by_repo<T>(cons: &[SubmittedTogether<T>]) -> Vec<Vec<&SubmittedTogether<T>>>
where
    T: Eq + std::hash::Hash,
{
    let mut iter = cons.iter();
    let mut grouped = vec![vec![iter.next().unwrap()]];
    let mut outer = 0;

    for head in iter {
        let inner = grouped[outer].len() - 1;
        match head.repo == grouped[outer][inner].repo {
            true => grouped[outer].push(head),
            false => {
                grouped.push(Vec::new());
                outer += 1;
                grouped[outer].push(head);
            }
        }
    }

    grouped
}

/// This assumes that the input is also grouped based on the repo.
/// The repo name is used as a key by the algorithm to chunk into iterators.
/// `Cons` should have a reverse chronological order within each grouping.
/// The result will retain this order.
fn reorder_submitted_together<T>(
    cons: &[SubmittedTogether<T>],
) -> Result<Vec<Vec<Vec<SubmittedTogether<T>>>>>
where
    T: Eq + std::hash::Hash + Clone + std::fmt::Debug,
{
    // TODO: see if there is a better solution.
    let mut res: Vec<Vec<Vec<SubmittedTogether<T>>>> = Vec::new();
    if cons.is_empty() {
        return Ok(res);
    }

    // TODO: We do not necessarily need to own The ST<T> in here.
    // but want to reuse `group_by_secondary` with the inner data.
    // If we can make `group_by_secondary` work with either owned or reference
    // data this can be improved.
    let mut topic_backlinks: HashMap<String, Vec<SubmittedTogether<T>>> = HashMap::new();
    let count = cons.len();
    for c in cons.iter() {
        if let Some(topic) = c.topic.clone() {
            topic_backlinks.entry(topic).or_default().push(c.clone())
        }
    }

    let grouped = group_by_repo(cons);
    // Successively iterate through all the repo groupings and pop all "free" commits.
    // Then when all groupings have a topic barrier
    // (or if they are empty they are no longer part of this iteration).
    // Match the first topic in topological order.

    // An ordered list of iterators into the different repositories.
    let mut iters = Vec::new();
    let mut slots = HashMap::<&T, usize>::new();
    for (i, inner) in grouped.into_iter().enumerate() {
        let mut iter = inner.into_iter().peekable();
        // TODO: wait for stabilization of `try_insert`: https://github.com/rust-lang/rust/issues/82766
        // slots.try_insert(&iter.peek().unwrap().repo, i)?;
        let key = &iter.peek().unwrap().repo;
        if slots.contains_key(key) {
            return Err(anyhow!(
                "Unexpected scrambled repo. Have already indexed this repo once."
            ));
        }
        slots.insert(key, i);
        iters.push(iter);
    }

    let mut iteration_limit = 1000;
    let mut index = 0;
    loop {
        if iteration_limit == 0 {
            eprintln!("WARNING: Iteration limit hit: truncating work.");
            break;
        }
        iteration_limit -= 1;

        if iters.iter_mut().all(|i| i.peek().is_none()) {
            break;
        }

        let slot = index % iters.len();

        if iteration_limit % 10 == 0 {
            println!("   {res:?}");
            for (i, iter) in iters.iter_mut().enumerate() {
                let head = iter.peek();
                let mut c = " ";
                if i == slot {
                    c = ">";
                }
                let id = match head {
                    Some(commit) => format!("{} {:?}", commit.id.clone(), commit.topic),
                    None => "None".to_string(),
                };
                println!("{c}{i} {id}");
            }
            println!("");
        }

        let candidate = iters[slot].peek();
        if candidate.is_none() {
            index += 1;
            continue;
        }
        let candidate = candidate.unwrap();

        if candidate.topic.is_none() {
            res.push(vec![vec![iters[slot].next().unwrap().clone()]]);
            // Continue and try the same slot again, do not increment index.
            continue;
        }

        // Topic handling is a little more involved. We then need to look
        // across all the iterators to understand whether we are good to
        // finalize a topic.

        let topic = candidate.topic.clone().unwrap();
        let looking_for: &Vec<SubmittedTogether<T>> = &topic_backlinks[&topic];
        let within = looking_for.iter().map(|e| &e.repo).unique();
        let looking_for = group_by_repo(looking_for);

        let mut ok = true;
        for repo in within {
            ok &= iters[slots[repo]].peek().and_then(|h| h.topic.clone()) == Some(topic.clone());
        }
        if !ok {
            // We do not have the topics available in our iterator heads.
            // continue with more work in other repos.
            index += 1;
            continue;
        }

        // All the necessary topics are on the heads. Possibly stacked within an
        // iterator. (We never checked past the heads, it is assumed that the
        // topics within a repo is contiguous. It does not make sense
        // otherwise.)
        // We can now pop them. (And assert that we do in fact find contiguous
        // topics.)
        let mut topic = Vec::new();
        for readrepo in looking_for.into_iter() {
            let mut commits = Vec::new();
            for commit in readrepo.iter() {
                let head = iters[slots[&commit.repo]].next().unwrap();
                if head.topic != commit.topic {
                    return Err(anyhow!(
                        "Unexpected non-topic commit, expected a topic in this repo."
                    ));
                }
                commits.push(head.clone())
            }
            topic.push(commits);
        }

        res.push(topic);
        index += 1;
    }

    let mut res_count = 0;
    for topic in res.iter() {
        for repo in topic.iter() {
            for _commit in repo.iter() {
                res_count += 1;
            }
        }
    }

    assert_eq!(count, res_count, "Not all commits are accounted for");

    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(id: &str, topic: Option<&str>, repo: i32) -> SubmittedTogether<i32> {
        SubmittedTogether::<i32> {
            id: id.to_owned(),
            topic: topic.map(|s| s.to_owned()),
            repo,
        }
    }

    fn new_s(id: &str, topic: Option<&str>, repo: &str) -> SubmittedTogether<String> {
        SubmittedTogether::<String> {
            id: id.to_owned(),
            topic: topic.map(|s| s.to_owned()),
            repo: repo.to_owned(),
        }
    }

    #[test]
    fn no_topic() {
        let a = new("first", None, 1);
        let b = new("second", None, 2);

        let res = reorder_submitted_together(&[a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![[a]], vec![[b]]]);
    }

    #[test]
    fn only_topic() {
        let topic = Some("topic");
        let a = new("first", topic, 1);
        let b = new("second", topic, 2);

        let res = reorder_submitted_together(&[a.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![[a], [b]]]);
    }

    #[test]
    fn topic_in_same_repo() {
        let topic = Some("topic");
        let shared_repo = 2;
        let a = new("first", topic, shared_repo);
        let b = new("second", topic, shared_repo);

        let res = reorder_submitted_together(&[b.clone(), a.clone()]);
        assert_eq!(res.unwrap(), vec![vec![[b, a]]]);
    }

    #[test]
    fn under_topic() {
        let topic = Some("topic");
        let shared_repo = 2;

        let a = new("first", topic, 1);
        let b = new("second", topic, shared_repo);
        let u = new("under", None, shared_repo);

        let res = reorder_submitted_together(&[a.clone(), b.clone(), u.clone()]);
        assert_eq!(res.unwrap(), vec![vec![[a], [b]], vec![[u]]]);
    }

    #[test]
    fn over_topic() {
        let topic = Some("topic");
        let shared_repo = 2;
        let a = new("first", topic, 1);
        let b = new("second", topic, shared_repo);
        let o = new("over", None, shared_repo);

        let res = reorder_submitted_together(&[a.clone(), o.clone(), b.clone()]);
        assert_eq!(res.unwrap(), vec![vec![[o]], vec![[a], [b]]]);
    }

    #[test]
    fn topic_hamburger() {
        let topic = Some("topic");
        let other_topic = Some("other_topic");
        let shared_repo = 2;
        let a = new("first", topic, 1);
        let b = new("second", topic, shared_repo);
        let m = new("middle", None, shared_repo);
        let c = new("fourth", other_topic, shared_repo);
        let d = new("fifth", other_topic, 3);

        let res =
            reorder_submitted_together(&[a.clone(), c.clone(), m.clone(), b.clone(), d.clone()]);
        assert_eq!(
            res.unwrap(),
            vec![vec![[c], [d]], vec![[m]], vec![[a], [b]]]
        );
    }

    #[test]
    fn two_topics() {
        let topic = Some("topic");
        let other_topic = Some("other_topic");
        let shared_repo = 2;
        let at = new("first_on_top", other_topic, 1);
        let bu = new("first_under", topic, shared_repo);
        let bt = new("second_on_top", other_topic, shared_repo);
        let cu = new("second_under", topic, 3);

        let res = reorder_submitted_together(&[at.clone(), bt.clone(), bu.clone(), cu.clone()]);
        assert_eq!(res.unwrap(), vec![vec![[at], [bt]], vec![[bu], [cu]]]);
    }

    #[test]
    fn stacked_commits_in_topic() {
        let topic = Some("topic");
        let shared_repo = 1;
        let a = new("under", topic, shared_repo);
        let b = new("on_top", topic, shared_repo);
        let c = new("other", topic, 2);

        let res = reorder_submitted_together(&[b.clone(), a.clone(), c.clone()]);
        assert_eq!(res.unwrap(), vec![vec![vec![b, a], vec![c]]]);
    }

    #[test]
    fn fail_no_topic_inside_a_stacked_topic() {
        let topic = Some("topic");
        let shared_repo = 1;
        let a = new("under", topic, shared_repo);
        let b = new("interloper", None, shared_repo);
        let c = new("on_top", topic, shared_repo);

        let res = reorder_submitted_together(&[c.clone(), b.clone(), a.clone()]);
        assert!(res.is_err());
    }

    #[test]
    fn fail_scrambled_commits_in_repos() {
        let shared_repo = 1;
        let a = new("first", None, shared_repo);
        let b = new("other", None, 2);
        let c = new("also_first", None, shared_repo);

        let res = reorder_submitted_together(&[a.clone(), b.clone(), c.clone()]);
        assert!(res.is_err())
    }

    #[test]
    fn disjoint_topics() {
        // There is no shared repository information.
        // The Gerrit API should not return data like this,
        // but we want to make a point of how to handle it.
        let topic = Some("topic");
        let other = Some("other");
        let a = new("first", topic, 1);
        let b = new("also_first", topic, 2);
        let c = new("other", other, 3);
        let d = new("also_other", other, 4);

        let one_order = reorder_submitted_together(&[a.clone(), b.clone(), c.clone(), d.clone()]);
        let other_order = reorder_submitted_together(&[d.clone(), c.clone(), b.clone(), a.clone()]);
        // TODO: Order it fully
        /*
        assert_eq!(one_order.unwrap(), other_order.unwrap());
        */
        let _ = one_order;
        let _ = other_order;
    }

    #[test]
    fn seen_in_the_wild() {
        let submitted_together = vec![
            new_s(
                "csp%2Fbazel%2Frules_csp~master~I14c8cee68c00ab13f1faed256529c7443cdf495f",
                Some("ARTCEE-9526-em"),
                "csp/bazel/rules_csp"
            ),
            new_s(
                "csp%2Fdiagnostics_did_rid~master~I36d481a26c0210d306947ced13531f53debece8d",
                Some("ARTCEE-9526-secure_storage"),
                "csp/diagnostics_did_rid"
            ),
            new_s(
                "csp%2Fhp%2Fcore~master~If66426a06b4d7d56588fac8958532076c185a8a7",
                None,
                "csp/hp/core"
            ),
            new_s(
                "csp%2Fhp%2Fcore~master~I4e3f6934555663dd08b2a0bd7cba35fad6a80bf5",
                Some("ARTCEE-9526-com_proxy"),
                "csp/hp/core"
            ),
            new_s(
                "csp%2Fhp%2Fcore~master~I7a1760b883e7cb2c87ee993e14ae41547f9cc1a8",
                Some("ARTCEE-9526-log_framework"),
                "csp/hp/core"
            ),
            new_s(
                "csp%2Fhp%2Fcore~master~I1c16f0b722c66268661724d1401211c50920205f",
                Some("ARTCEE-9526-secure_storage"),
                "csp/hp/core"
            ),
            new_s(
                "csp%2Fhp%2Fcore~master~I824d90f3edc7559549db94ff4e67580e5d715eb7",
                Some("ARTCEE-9526-em"),
                "csp/hp/core"
            ),
            new_s(
                "csp%2Fhp%2Fcore~master~I6c0b1f85a50b7b4334427051bbc8555d851bdb8e",
                None,
                "csp/hp/core"
            ),
            new_s(
                "csp%2Fhp%2Fcore~master~I1497a9b290dce15baed011d1e55f7fd3499f9eea",
                Some("ARTCEE-9526"),
                "csp/hp/core"
            ),
            new_s(
                "csp%2Fhpa%2Ftest~master~I3e22f189f5668923befdf5995c0fcdd159a2570d",
                Some("ARTCEE-9526"),
                "csp/hpa/test"
            ),
            new_s(
                "csp%2Fhpa%2Ftest~master~I3cfde1a7af81896f4e84509aa47c4700265123fa",
                Some("ARTCEE-9526-log_framework"),
                "csp/hpa/test"
            ),
            new_s(
                "csp%2Fhpa%2Ftest~master~If63208d14b15b5d042fc0c0fdc1819702daf3c94",
                Some("ARTCEE-9526-com_proxy"),
                "csp/hpa/test"
            ),
            new_s(
                "csp%2Fhpa_lpa_diagnostic_platform_facade~master~I82763f898a909c3a0a2c1836f417920e91c718a7",
                Some("ARTCEE-9526"),
                "csp/hpa_lpa_diagnostic_platform_facade"
            ),
            new_s(
                "csp%2Flpa_agent~master~Iaec404a5df03322164ae53edd01d73ecff4d0141",
                Some("ARTCEE-9526"),
                "csp/lpa_agent"
            ),
            new_s(
                "csp%2Fproducts%2Fvhpa~master~I58abf025431a3a77bcdb4a8504ee2cc12147acd9",
                Some("ARTCEE-9526-em"),
                "csp/products/vhpa"
            ),
            new_s(
                "csp%2Fproducts%2Fvhpa~master~I88149a728791580323f7774be058260ad5e7a01c",
                Some("ARTCEE-9526-com_proxy"),
                "csp/products/vhpa"
            ),
            new_s(
                "csp%2Fsystem%2Fhpa_activation_manager~master~Ie6ed1d287589e7d24d1f30f9162adab237e5163b",
                Some("ARTCEE-9526"),
                "csp/system/hpa_activation_manager"
            ),
            new_s(
                "csp%2Fuds_tester_handler~master~I77a7f35f2fbf8d635ab5211337b56f0c1624b10d",
                Some("ARTCEE-9526"),
                "csp/uds_tester_handler"
            ),
            new_s(
                "vcuapps%2Fexample_app~master~I2b1b3fe9415a808aab544f2ae0579a923215ca19",
                Some("ARTCEE-9526"),
                "vcuapps/example_app"
            ),
            new_s(
                "vcuapps%2Fexample_app~master~I77968adc2aa9f39df7e61d940de1980314eb2623",
                Some("ARTCEE-9526-com_proxy"),
                "vcuapps/example_app"
            ),
        ];

        // Should not crash.
        let _ = reorder_submitted_together(&submitted_together);
    }
}
