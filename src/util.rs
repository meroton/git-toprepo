use url::Url;

pub fn join_submodule_url(parent: &str, mut other: &str) -> String {
    if other.starts_with("./") || other.starts_with("../") || other == "." {
        let scheme_end = match parent.find("://") {
            Some(i) => i + 3,
            None => 0,
        };
        let (scheme, parent) = parent.split_at(scheme_end);
        let mut parent = parent.trim_end_matches("/").to_string();

        loop {
            if other.starts_with("/") {
                (_, other) = other.split_at(1);
            } else if other.starts_with("./") {
                (_, other) = other.split_at(2);
            } else if other.starts_with("../") {
                match parent.rfind("/") {
                    Some(i) => { parent.drain(..i); }

                    //Too many "../", move it from other to parent.
                    None => parent += "/..",
                }

                (_, other) = other.split_at(3);
            } else {
                break;
            }
        }

        return if other == "." || other.is_empty() {
            format!("{}{}", scheme, parent)
        } else {
            format!("{}{}/{}", scheme, parent, other)
        };
    }

    other.to_string()
}

pub fn iter_to_string<'a, I>(items: I) -> Vec<String>
where
    I: IntoIterator<Item=&'a str>,
{
    items.into_iter().map(|s| s.to_string()).collect()
}