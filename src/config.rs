use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use itertools::Itertools;
use regex::Regex;
use crate::config_loader::{
    ConfigLoaderTrait,
    ConfigLoader,
    GitRemoteConfigLoader,
    LocalFileConfigLoader,
    LocalGitConfigLoader,
    MultiConfigLoader,
    StaticContentConfigLoader,
};
use crate::Repo;
use crate::util::{iter_to_string, join_submodule_url};


//TODO: Create proper error enums instead of strings


/*
Toprepo, super repo har alla submoduler
Monorepo, utsorterat
 */
const DEFAULT_FETCH_ARGS: [&str; 3] = ["--prune", "--prune-tags", "--tags"];


struct RepoConfig {
    /// Name of the storage directory and used for pattern matching.
    name: String,

    /// Flags if this repos should be expanded or not.
    enabled: bool,

    /// Exact matching against sub repos configs like .gitmodules.
    ///
    /// These URLs are not resolved any may be relative.
    raw_urls: Vec<String>,

    /// Absolute URL to git-fetch from.
    fetch_url: String,

    /// extra options for git-fetch.
    fetch_args: Vec<String>,

    /// Absolute URL to git-push to.
    push_url: String,
}


///////////////////////////////////////////////////////////////////////////////

const CONFIGMAP_UNSET: &str = "git_toprepo_ConfigDict_unset"; //Should this be replaced by None?

//https://git-scm.com/docs/git-config#_configuration_file
#[derive(Debug)]
pub struct ConfigMap {
    map: HashMap<String, Vec<String>>,
}

#[allow(dead_code)]
impl ConfigMap {
    pub fn new() -> ConfigMap {
        ConfigMap { map: HashMap::new() }
    }

    pub fn join<'a, I: IntoIterator<Item=&'a ConfigMap>>(configs: I) -> ConfigMap {
        let mut ret = ConfigMap::new();

        for config in configs {
            for (key, values) in &config.map {
                ret.push(&key, values.clone());
            }
        }

        ret
    }

    pub fn parse(config_lines: &str) -> Result<ConfigMap, String> {
        let mut ret = ConfigMap::new();

        for line in config_lines.split("\n").filter(|s| !s.is_empty()) {
            if let Some((key, value)) = line.split("=").next_tuple() {
                if !ret.map.contains_key(key) {
                    ret.map.insert(key.to_string(), Vec::new());
                }
                ret.map.get_mut(key).unwrap().push(value.to_string());
            } else {
                println!("Could not parse \"{}\"", line);
                //panic!("Could not parse \"{}\"", line);
                //return Err(format!("Could not parse \"{}\"", line).to_string());
            }
        }

        Ok(ret)
    }

    pub fn get(&self, key: &str) -> Option<&Vec<String>> {
        self.map.get(key)
    }

    pub fn get_last(&self, key: &str) -> Option<&str> {
        Some(self.map.get(key)?.last()?.as_str())
    }

    pub fn remove(&mut self, key: &str) -> Option<Vec<String>> {
        self.map.remove(key)
    }

    pub fn remove_last(&mut self, key: &str) -> Option<String> {
        self.map.get_mut(key)?.pop()
    }

    /// Inserts default value if key doesn't exist in the map
    pub fn set_default(&mut self, key: &str, default: Vec<String>) -> &Vec<String> {
        self.map.entry(key.to_string())
            .or_insert(default)
    }

    pub fn push(&mut self, key: &str, mut values: Vec<String>) {
        if !self.map.contains_key(key) {
            self.map.insert(key.to_string(), values);
        } else {
            self.map.get_mut(key).unwrap().append(&mut values);
        }
    }


    /// Extracts for example submodule.<name>.<key>=<value>.
    /// All entries that dont contain the prefix are returned in the residual
    pub fn extract_mapping(&self, prefix: &str) -> HashMap<String, ConfigMap> {
        let mut prefix = prefix.to_string();
        if !prefix.ends_with('.') {
            prefix.push('.');
        }

        let mut extracted = HashMap::new();
        let mut residual = ConfigMap::new();

        for (key, values) in &self.map {
            if let Some(temp) = key.strip_prefix(&prefix) {
                if let Some((name, subkey)) = temp.split(".").next_tuple() {
                    if !extracted.contains_key(name) {
                        extracted.insert(name.to_string(), ConfigMap::new());
                    }

                    extracted.get_mut(name).unwrap().push(subkey, values.clone());
                } else {
                    unreachable!("Illegal config {}", temp);
                }
            }
        }

        extracted
    }

    pub fn get_singleton<'t>(&'t self, key: &str, default: Option<&'t str>) -> Result<&str, String> {
        if !self.map.contains_key(key) {
            return Ok(default.unwrap_or(CONFIGMAP_UNSET));
        }

        let mut values = self.map[key]
            .iter().sorted();

        match values.len() {
            0 => panic!("The key {} should not exist without a value!", key),
            1 => Ok(values.next().unwrap()),
            _ => {
                panic!("Conflicting values for {}: {}", key, values.join(", "));
                Err(format!("Conflicting values for {}: {}", key, values.join(", ")))
            }
        }
    }
}

impl Display for ConfigMap {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let temp = self.map.iter().map(|(key, values)|
        format!("{}: [{}]", key, values.join(", "))
        ).join(", ");

        write!(f, "ConfigMap {{ {} }}", temp)?;

        Ok(())
    }
}


///////////////////////////////////////////////////////////////////////////////
#[derive(Debug)]
pub struct ConfigAccumulator<'a> {
    monorepo: &'a Repo,
    online: bool,
}

impl ConfigAccumulator<'_> {
    pub fn new(monorepo: &Repo, online: bool) -> ConfigAccumulator {
        ConfigAccumulator {
            monorepo,
            online,
        }
    }

    pub fn load_main_config(&self) -> Result<ConfigMap, String> {
        let config_loader = ConfigLoader::from(MultiConfigLoader::new(
            vec![
                ConfigLoader::from(LocalGitConfigLoader::new(self.monorepo)),
                ConfigLoader::from(StaticContentConfigLoader::new("\
[toprepo.config.default]
    type = \"git\"
    url = .
    ref = refs/meta/git-toprepo
    path = toprepo.config".to_string()
                )),
            ]
        ));
        self.load_config(config_loader)
    }

    fn load_config(&self, config_loader: ConfigLoader) -> Result<ConfigMap, String> {
        let mut full_configmap = ConfigMap::new();
        let mut existing_names = HashSet::new();

        let mut queue = vec![config_loader];
        while let Some(config_loader) = queue.pop() {
            if self.online {
                config_loader.fetch_remote_config();
            }

            let current_configmap = config_loader.get_configmap()?;
            let sub_config_loaders = self.get_config_loaders(
                &current_configmap, &full_configmap,
            );

            // Earlier loaded configs overrides later loaded configs.
            full_configmap = ConfigMap::join([&current_configmap, &full_configmap]);
            // Traverse into sub-config-loaders.
            for (name, sub_config_loader) in sub_config_loaders? {
                if existing_names.contains(&name) {
                    panic!("toprepo.config.{} configurations found in multiple sources", name);
                    return Err(format!("toprepo.config.{} configurations found in multiple sources", name));
                }
                existing_names.insert(name);
                queue.push(sub_config_loader);
            }
        }

        Ok(full_configmap)
    }

    fn get_config_loaders(&self, configmap: &ConfigMap, overrides: &ConfigMap) ->
    Result<HashMap<String, ConfigLoader>, String> {
        let mut config_loaders = HashMap::new();
        // Accumulate toprepo.config.<id>.* keys.
        let own_loader_configmaps = configmap.extract_mapping("toprepo.config");
        let full_loader_configmaps = ConfigMap::join([configmap, overrides])
            .extract_mapping("toprepo.config");

        for (name, own_loader_values) in own_loader_configmaps.into_iter() {
            //Check if values are just for overriding or the actual configuration.
            let partial_value = own_loader_values.get_last("partial");
            if let Some(value) = partial_value {
                match value.to_lowercase().as_str() {
                    "1" | "true" => continue,
                    _ => (),
                }
            }

            // Actual configuration, load.
            let full_loader_values = &full_loader_configmaps[&name];
            let config_loader = self.get_config_loader(&name, full_loader_values)?;
            config_loaders.insert(name, config_loader);
        }

        Ok(config_loaders)
    }

    fn get_config_loader<'a>(&self, name: &str, configmap: &ConfigMap) -> Result<ConfigLoader, String> {
        let loader_type = configmap.get_last("type")
            .expect("Missing config loader type").to_lowercase();

        let config_loader = match loader_type.as_str() {
            "none" => ConfigLoader::from(StaticContentConfigLoader::new(String::new())),
            "file" => {
                let mut file_path = self.monorepo.path.clone();
                file_path.push(PathBuf::from(
                    configmap.get_last("file").expect("Missing file")
                ));

                ConfigLoader::from(LocalFileConfigLoader::new(file_path, false))
            }
            "git" => {
                // Load
                let raw_url = configmap.get_last("url").expect("Missing url");
                let reference = configmap.get_last("ref").expect("Missing ref");
                let filename = PathBuf::from(configmap.get_last("url").expect("Missing filename"));

                /*
                .
                refs/meta/git-toprepo
                toprepo.config
                 */


                // Translate.
                let parent_url = self.monorepo.get_toprepo_fetch_url().unwrap();
                let url = join_submodule_url(parent_url, raw_url);
                let filename = PathBuf::from(&filename);

                // Parse.
                ConfigLoader::from(GitRemoteConfigLoader::new(
                    url,
                    reference.to_string(),
                    filename,
                    &self.monorepo,
                    format!("refs/toprepo/config/{}", name),
                ))
            }
            _ => {
                panic!("Invalid toprepo.config.type {}!", loader_type);
                return Err(format!("Invalid toprepo.config.type {}!", loader_type));
            }
        };

        Ok(config_loader)
    }
}


pub struct Config {
    missing_commits: HashMap<String, HashSet<String>>, // TODO What data type is a commit hash?
    top_fetch_url: String,
    top_push_url: String,
    repos: Vec<RepoConfig>,
}

impl Config {
    pub fn new(mut configmap: ConfigMap) -> Config {
        let repo_configmaps = configmap.extract_mapping("toprepo.repo.");

        // Resolve the role.
        configmap.set_default("toprepo.role.default.repos", vec!["+.*".to_string()]);
        let role = configmap.get_last("toprepo.role").unwrap_or("default");

        let wanted_repos_role = format!("toprepo.role.{}.repos", role);
        configmap.set_default(&wanted_repos_role, vec![]);
        let wanted_repos_patterns = configmap.remove(&wanted_repos_role).unwrap();

        let top_fetch_url = match configmap.remove_last("remote.origin.url").as_deref() {
            None | Some("file:///dev/null") => configmap.remove_last("toprepo.top.fetchurl")
                .expect("Config remote.origin.url is not set"),
            Some(url) => url.to_string(),
        };
        let top_push_url = match configmap.remove_last("toprepo.top.pushurl") {
            None => configmap.remove_last("toprepo.top.pushurl")
                .expect(&"Config remote.top.pushurl is not set"),
            Some(url) => url,
        };
        let repo_configs = Config::parse_repo_configs(
            repo_configmaps,
            wanted_repos_patterns,
            top_fetch_url.as_str(),
            top_push_url.as_str(),
        );

        // Find configured missing commits.
        let mut missing_commits = HashMap::new();
        let missing_commits_prefix = "toprepo.missing-commits.rev-";
        for (key, values) in configmap.map {
            if let Some(commit_hash) = key.strip_prefix(missing_commits_prefix) {
                for raw_url in values {
                    missing_commits.entry(raw_url).or_insert(HashSet::new())
                        .insert(commit_hash.to_string());
                }
            }
        }

        Config {
            missing_commits,
            top_fetch_url: top_fetch_url.to_string(),
            top_push_url: top_push_url.to_string(),
            repos: repo_configs,
        }
    }

    fn parse_repo_configs(
        repo_configmaps: HashMap<String, ConfigMap>,
        wanted_repos_patterns: Vec<String>,
        parent_fetch_url: &str,
        parent_push_url: &str,
    ) -> Vec<RepoConfig> {
        let mut repo_configs = Vec::new();

        for (repo_name, repo_configmap) in repo_configmaps {
            repo_configs.push(Config::parse_repo_config(
                repo_name,
                repo_configmap,
                &wanted_repos_patterns,
                parent_fetch_url,
                parent_push_url,
            ))
        }

        repo_configs
    }

    fn parse_repo_config(
        name: String,
        mut repo_configmap: ConfigMap,
        wanted_repos_patterns: &Vec<String>,
        parent_fetch_url: &str,
        parent_push_url: &str,
    ) -> RepoConfig {
        // TODO: name check?
        if PathBuf::from(&name).components().count() != 1 {
            panic!("Subdirectories not allowed in repo name: {}", name);
        }

        let enabled = Config::repo_is_wanted(&name, wanted_repos_patterns)
            .expect(format!("Could not determine if repo {} is wanted or not", name).as_str());

        let mut raw_urls = repo_configmap.remove("urls")
            .expect(format!("toprepo.repo.{}.urls is unspecified", name).as_str());

        let raw_fetch_url = match repo_configmap.get_last("fetchurl") {
            None => {
                if raw_urls.len() != 1 {
                    panic!("Missing toprepo.repo.{}.fetchUrl and multiple \
                    toprepo.repo.{}.urls gives an ambiguous default", name, name)
                }

                raw_urls.pop().unwrap()
            }
            Some(url) => url.to_string(),
        };
        let fetch_url = join_submodule_url(parent_fetch_url, raw_fetch_url.as_str());

        let raw_push_url = repo_configmap.get_last("pushurl")
            .unwrap_or(raw_fetch_url.as_str());
        let push_url = join_submodule_url(parent_push_url, raw_push_url);

        let fetch_args = repo_configmap.remove("fetchargs")
            .unwrap_or(iter_to_string(DEFAULT_FETCH_ARGS));

        RepoConfig {
            name,
            enabled,
            raw_urls,
            fetch_url,
            fetch_args,
            push_url,
        }
    }

    fn repo_is_wanted(name: &str, wanted_repos_patterns: &Vec<String>) -> Option<bool> {
        for pattern in wanted_repos_patterns {
            if !pattern.starts_with(&['+', '-']) {
                panic!("Invalid wanted repo config {} for {}, \
                should start with '+' or '-' followed by a regex.",
                       pattern, name);
            }

            // Returns True if it matches with a '+' and false if it matches with a '-'.
            if Regex::new(&pattern[1..]).unwrap().find(name).is_some() {
                return Some(pattern.starts_with('+'));
            }
        }

        None
    }

    fn raw_url_to_repos(&self) -> HashMap<&str, Vec<&RepoConfig>> {
        let mut raw_url_to_repos = HashMap::new();
        for repo_config in &self.repos {
            for raw_url in &repo_config.raw_urls {
                raw_url_to_repos.entry(raw_url.as_str())
                    .or_insert(Vec::new())
                    .push(repo_config);
            }
        }
        raw_url_to_repos
    }
}