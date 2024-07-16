use url::Url;

pub fn join_submodule_url(parent: &str, other: &str) -> String {
    todo!()
}

pub fn iter_to_string<'a, I>(items: I) -> Vec<String>
where
    I: IntoIterator<Item=&'a str>,
{
    items.into_iter().map(|s| s.to_string()).collect()
}