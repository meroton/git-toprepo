use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::process::Command;
use enum_dispatch::enum_dispatch;
use itertools::Itertools;
use url::Url;
use crate::MonoRepo;
use crate::util::join_submodule_url;


//TODO: Create proper error enums instead of strings

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

    pub fn join<'a, I: IntoIterator<Item=&'a ConfigMap>>(configs: I) -> ConfigMap
    {
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

    pub fn get_last(&self, key: &str) -> Option<&String> {
        self.map.get(key)?.last()
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


//////////////////////////////////////////////
#[enum_dispatch]
trait ConfigLoaderTrait {
    fn fetch_remote_config(&self);
    fn git_config_list(self) -> String;
    fn get_configmap(self) -> Result<ConfigMap, String>
    where
        Self: Sized,
    {
        ConfigMap::parse(&self.git_config_list())
    }
}

// Similar to an abstract class, removes the need for Box<dyn &ConfigLoaderTrait>
#[enum_dispatch(ConfigLoaderTrait)]
enum ConfigLoader<'a> {
    MultiConfigLoader(MultiConfigLoader<'a>),
    LocalGitConfigLoader(LocalGitConfigLoader<'a>),
    ContentConfigLoader,
    StaticContentConfigLoader,
    LocalFileConfigLoader,
    GitRemoteConfigLoader(GitRemoteConfigLoader<'a>),
}


struct MultiConfigLoader<'a> {
    config_loaders: Vec<ConfigLoader<'a>>,
}

struct LocalGitConfigLoader<'a> {
    repo: &'a MonoRepo, // <---- This reference causes lifetime voodoo.
}

struct ContentConfigLoader {
    // idk
}

struct StaticContentConfigLoader {
    content: String,
}

struct LocalFileConfigLoader {
    filename: PathBuf,
    allow_missing: bool,
}

struct GitRemoteConfigLoader<'a> {
    url: Url,
    remote_ref: String,
    filename: PathBuf,
    local_repo: &'a MonoRepo, // <---- This reference causes lifetime voodoo.
    local_ref: String,
}

impl ConfigLoaderTrait for MultiConfigLoader<'_> {
    fn fetch_remote_config(&self) {
        for config_loader in &self.config_loaders {
            config_loader.fetch_remote_config();
        }
    }

    fn git_config_list(self) -> String {
        let mut config_list = String::new();

        // Joins all ConfigLoaders in reverse order.
        for config_loader in self.config_loaders {
            let mut part: String = config_loader.git_config_list();
            if !part.is_empty() && !part.ends_with('\n') {
                part.push('\n');
            }

            part.push_str(&config_list);
            config_list = part;
        }

        config_list
    }
}

impl ConfigLoaderTrait for LocalGitConfigLoader<'_> {
    fn fetch_remote_config(&self) {}
    fn git_config_list(self) -> String {
        let command = Command::new("git")
            .args(["-C", self.repo.path.to_str().unwrap().trim()])
            .args(["config", "--list"])
            .output()
            .unwrap();

        let ret = String::from_utf8(command.stdout)
            .expect("Could not load Local git configuration.");
        println!("-----\n{}-----", ret);
        ret
    }
}

impl ConfigLoaderTrait for ContentConfigLoader {
    fn fetch_remote_config(&self) { todo!() }
    fn git_config_list(self) -> String { todo!() }
}

impl ConfigLoaderTrait for StaticContentConfigLoader {
    fn fetch_remote_config(&self) {}
    fn git_config_list(self) -> String {
        self.content
    }
}

impl ConfigLoaderTrait for LocalFileConfigLoader {
    fn fetch_remote_config(&self) { todo!() }
    fn git_config_list(self) -> String { todo!() }
}

impl ConfigLoaderTrait for GitRemoteConfigLoader<'_> {
    fn fetch_remote_config(&self) { todo!() }
    fn git_config_list(self) -> String { todo!() }
}

//////////////////////////////////////////////
#[derive(Debug)]
pub struct ConfigAccumulator<'a> {
    monorepo: &'a MonoRepo,
    online: bool,
}

impl ConfigAccumulator<'_> {
    pub fn new(monorepo: &MonoRepo, online: bool) -> ConfigAccumulator {
        ConfigAccumulator {
            monorepo,
            online,
        }
    }

    pub fn load_main_config(&self) -> Result<ConfigMap, String> {
        let config_loader = ConfigLoader::from(MultiConfigLoader {
            config_loaders: vec![
                ConfigLoader::from(LocalGitConfigLoader { repo: self.monorepo }),
                ConfigLoader::from(StaticContentConfigLoader { content: "...".to_string() }),
            ]
        });
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
            "none" => ConfigLoader::from(StaticContentConfigLoader { content: String::new() }),
            "file" => {
                let mut file_path = self.monorepo.path.clone();
                file_path.push(PathBuf::from(
                    configmap.get_last("file").expect("Missing file")
                ));

                ConfigLoader::from(LocalFileConfigLoader { filename: file_path, allow_missing: false })
            }
            "git" => {
                // Load
                let raw_url = Url::parse(
                    configmap.get_last("url").expect("Missing url")
                ).unwrap();
                let reference = configmap.get_last("ref").expect("Missing ref").clone();
                let filename = PathBuf::from(configmap.get_last("url").expect("Missing filename"));

                // Translate.
                let parent_url = self.monorepo.get_toprepo_fetch_url().unwrap();
                let url = join_submodule_url(parent_url, raw_url);
                let filename = PathBuf::from(&filename);

                // Parse.
                ConfigLoader::from(GitRemoteConfigLoader {
                    url,
                    remote_ref: reference,
                    filename,
                    local_repo: &self.monorepo,
                    local_ref: format!("refs/toprepo/config/{}", name),
                })
            }
            _ => {
                panic!("Invalid toprepo.config.type {}!", loader_type);
                return Err(format!("Invalid toprepo.config.type {}!", loader_type));
            }
        };

        Ok(config_loader)
    }
}