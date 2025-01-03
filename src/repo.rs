use anyhow::Result;

pub struct SubRepo {
    pub name: String,
    pub config: crate::config::SubrepoConfig,
}

impl SubRepo {
    pub fn get_url(&self, _direction: gix::remote::Direction) -> String {
        todo!();
    }
}

pub enum RepoName {
    TopRepo,
    SubRepo(String),
}

/// Map a remote or URL to a repository.
///
/// A repo can be specified by subrepo path inside the toprepo or as a full or
/// partial URL.
pub fn remote_to_repo(
    // toprepo: &TopRepo,
    _direction: gix::remote::Direction,
    _remote: &str,
    // git_modules: &Vec<GitModuleInfo>,
    // config: &Config,
) -> Result<RepoName> {
    todo!();
    /*
    // Map a full or partial URL or path to one or more repos.
    let mut remote_to_name = HashMap::from_iter([
        (".", vec![RepoName::TopRepo]),
        ("", vec![RepoName::TopRepo]),
    ]);
    toprepo.gix_repo.find_remotes()...

    fn add_url<'a>(
        remote_to_name: &mut HashMap<&'a str, Vec<(&'a str, Option<&'a GitModuleInfo>)>>,
        url: &'a str,
        name: &'a str,
        gitmod: Option<&'a GitModuleInfo>,
    ) {
        let entry = (name, gitmod);
        remote_to_name.entry(url).or_insert(Vec::new()).push(entry);

        // Also match partial URLs.
        // Example: ssh://user@github.com:22/foo/bar.git
        let url = url.strip_suffix(".git").unwrap_or(url);
        remote_to_name.entry(url).or_insert(Vec::new()).push(entry);

        if let Some(i) = url.find("://") {
            let (_, url) = url.split_at(i);
            remote_to_name.entry(url).or_insert(Vec::new()).push(entry);
        }
        if let Some(i) = url.find("@") {
            let (_, url) = url.split_at(i);
            remote_to_name.entry(url).or_insert(Vec::new()).push(entry);
        }
        if !url.starts_with(".") {
            if let Some(i) = url.find("/") {
                let (_, url) = url.split_at(i);
                remote_to_name.entry(url).or_insert(Vec::new()).push(entry);
            }
        }
    }

    add_url(
        &mut remote_to_name,
        &config.top_fetch_url,
        TopRepo::NAME,
        None,
    );
    add_url(
        &mut remote_to_name,
        &config.top_push_url,
        TopRepo::NAME,
        None,
    );

    for module in &git_modules {
        for cfg in &config.repos {
            if cfg.raw_urls.contains(&module.raw_url) {
                // Add URLs from .gitmodules.
                add_url(&mut remote_to_name, &module.url, &cfg.name, Some(&module));
                add_url(
                    &mut remote_to_name,
                    &module.raw_url,
                    &cfg.name,
                    Some(&module),
                );

                remote_to_name
                    .get_mut(module.name.as_str())
                    .unwrap()
                    .push((&cfg.name, Some(&module)));
                remote_to_name
                    .get_mut(module.path.to_str().unwrap())
                    .unwrap()
                    .push((&cfg.name, Some(&module)));

                // Add URLs from the toprepo config.
                add_url(
                    &mut remote_to_name,
                    &cfg.fetch_url,
                    &cfg.name,
                    Some(&module),
                );
                add_url(&mut remote_to_name, &cfg.push_url, &cfg.name, Some(&module));
                for raw_url in &cfg.raw_urls {
                    add_url(&mut remote_to_name, &raw_url, &cfg.name, Some(&module));
                }
            }
        }
    }

    //Now, try to find our repo.
    let full_remote = remote;
    let mut remote = remote;
    remote = remote.strip_suffix("/").unwrap_or(remote);
    remote = remote.strip_suffix(".git").unwrap_or(remote);
    let mut entries = remote_to_name.get(remote);

    if entries.is_none() && remote.contains("://") {
        (_, remote) = remote.split_once("://").unwrap();
        entries = remote_to_name.get(remote);
    }
    if entries.is_none() && remote.contains("@") {
        (_, remote) = remote.split_once("@").unwrap();
        entries = remote_to_name.get(remote);
    }
    if entries.is_none() && remote.contains("/") && !remote.starts_with(".") {
        (_, remote) = remote.split_once("/").unwrap();
        entries = remote_to_name.get(remote);
    }
    let entries = entries.expect(format!("Could not resolve '{}'", full_remote).as_str());

    if entries.len() > 1 {
        let names = entries.into_iter().map(|(name, _)| *name).join(", ");
        panic!("Multiple remote candidates: [{}]", names)
    }

    match entries[0] {
        (name, Some(gitmod)) => {
            let i = git_modules.iter().position(|module| module == gitmod);
            let name = name.to_string();
            let gitmod = git_modules.swap_remove(i.unwrap());
            (name, Some(gitmod))
        }
        (name, None) => (name.to_string(), None),
    }
    */
}
