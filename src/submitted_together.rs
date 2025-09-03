/// Order submitted_together
///
/// Gerrit returns a partially ordered list of commits.
/// Where topics are clearly delineated and dependency order internal to repos.
/// But the across repos we have no order.
///
/// Repos:    A     B     C     D
/// Commits
///    |      A1    B1
///    v      A2 -- B2 -- C1 -- D1
///                             D2
///           A3 -------------- D3
/// Gerrit reports the commits submitted together:
///     [A1, A2*, A3**, B1, B2*, C1*, D1*, D2, D3**]
/// The asterisks mark commits with topics.
///
/// We can split this to the (partially arbitrarily) ordered list of lists:
///     [[A1], [B1], [A2, B2, C1, D1], [D2], [A3, D3]]
/// The order between A1 and B1 is arbitrary, we can choose lexicographic order
/// on the repository name.
///
/// It is less common, but multiple commits within the same repo may also be in
/// the same topic and should also be treated as an atomic commit.

/// A small data view for this algorithm, to help in testing.
/// Real data should use a small `From<Real Data>` impl.
/// And then reconstruct the structure based on the id.
#[derive(Clone,Debug,PartialEq,PartialOrd)]
pub struct SubmittedTogether<T: PartialOrd> {
    id: String,
    topic: Option<String>,
    secondary: T,
}

pub fn order_submitted_together<T: PartialOrd>(cons: Vec<SubmittedTogether<T>>) -> Vec<Vec<SubmittedTogether<T>>> {
    todo!()
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
        assert_eq!(res, vec![vec![a], vec![b]]);
    }

    #[test]
    fn only_topic() {
        let topic = Some("topic");
        let a = new("first", topic, 1);
        let b = new("second", topic, 2);

        let res = order_submitted_together(vec![a.clone(), b.clone()]);
        assert_eq!(res, vec![vec![a, b]]);
    }

    #[test]
    fn topic_in_same_repo() {
        let topic = Some("topic");
        let shared_secondary = 2;
        let a = new("first", topic, shared_secondary);
        let b = new("second", topic, shared_secondary);

        let res = order_submitted_together(vec![a.clone(), b.clone()]);
        assert_eq!(res, vec![vec![a, b]]);
    }

    #[test]
    fn under_topic() {
        let topic = Some("topic");
        let shared_secondary = 2;

        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let u = new("under", None, shared_secondary);

        let res = order_submitted_together(vec![u.clone(), a.clone(), b.clone()]);
        assert_eq!(res, vec![vec![u], vec![a, b]]);
    }

    #[test]
    fn over_topic() {
        let topic = Some("topic");
        let shared_secondary = 2;
        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let o = new("over", None, shared_secondary);

        let res = order_submitted_together(vec![a.clone(), b.clone(), o.clone()]);
        assert_eq!(res, vec![vec![a, b], vec![o]]);
    }

    #[test]
    fn topic_hamburger() {
        let topic = Some("topic");
        let other_topic = Some("other_topic");
        let shared_secondary = 2;
        let a = new("first", topic, 1);
        let b = new("second", topic, shared_secondary);
        let m = new("middle", None, shared_secondary);
        let c = new("third", other_topic, 3);
        let d = new("fourth", other_topic, shared_secondary);

        let res = order_submitted_together(vec![a.clone(), b.clone(), m.clone(), c.clone(), d.clone()]);
        assert_eq!(res, vec![vec![a, b], vec![m], vec![c, d]]);
    }
}
