use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use itertools::Itertools;
use url::Url;
use crate::MonoRepo;


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

    pub fn join<'a, I>(configs: I) -> ConfigMap
    where
        I: Iterator<Item=&'a ConfigMap>,
    {
        let mut ret = ConfigMap::new();

        for config in configs {
            for (key, values) in config.map.iter() {
                ret.push(&key, values.clone());
            }
        }

        ret
    }

    pub fn parse(config_lines: &str) -> Result<ConfigMap, String> {
        let mut ret = ConfigMap::new();

        for line in config_lines.split("\n") {
            if let Some((key, value)) = line.split("=").next_tuple() {
                //ret[key].push(value.to_string());
                if !ret.map.contains_key(key) {
                    ret.map.insert(key.to_string(), Vec::new());
                }
                ret.map.get_mut(key).unwrap().push(value.to_string());
            } else {
                return Err(format!("Could not parse {}", line).to_string());
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

        for (key, values) in self.map.iter() {
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
            _ => Err(format!("Conflicting values for {}: {}", key, values.join(", "))),
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
trait ConfigLoader {
    fn fetch_remote_config(&self) -> ();
    fn git_config_list(&self) -> String;
    fn get_configmap(&self) -> Result<ConfigMap, String> {
        ConfigMap::parse(&self.git_config_list())
    }
}

struct MultiConfigLoader {
    config_loaders: Vec<Box<dyn ConfigLoader>>,
}

struct LocalGitConfigLoader {
    //repo: Repo TODO
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

struct GitRemoteConfigLoader {
    url: Url,
    remote_ref: String,
    filename: PathBuf,
    local_repo: (), //TODO
    local_ref: String,
}

impl ConfigLoader for MultiConfigLoader {
    fn fetch_remote_config(&self) -> () { todo!() }
    fn git_config_list(&self) -> String { todo!() }
}

impl ConfigLoader for LocalGitConfigLoader {
    fn fetch_remote_config(&self) -> () { todo!() }
    fn git_config_list(&self) -> String { todo!() }
}

impl ConfigLoader for ContentConfigLoader {
    fn fetch_remote_config(&self) -> () { todo!() }
    fn git_config_list(&self) -> String { todo!() }
}

impl ConfigLoader for StaticContentConfigLoader {
    fn fetch_remote_config(&self) -> () { todo!() }
    fn git_config_list(&self) -> String { todo!() }
}

impl ConfigLoader for LocalFileConfigLoader {
    fn fetch_remote_config(&self) -> () { todo!() }
    fn git_config_list(&self) -> String { todo!() }
}

impl ConfigLoader for GitRemoteConfigLoader {
    fn fetch_remote_config(&self) -> () { todo!() }
    fn git_config_list(&self) -> String { todo!() }
}


//////////////////////////////////////////////
#[derive(Debug)]
struct ConfigAccumulator<'a> {
    monorepo: &'a MonoRepo,
    online: bool,
}

impl ConfigAccumulator<'_> {
    fn load_config(&self, config_loader: Box<dyn ConfigLoader>) -> Result<ConfigMap, String> {
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
            full_configmap = ConfigMap::join([current_configmap, full_configmap].iter());
            // Traverse into sub-config-loaders.
            for (name, sub_config_loader) in sub_config_loaders?.into_iter() {
                if existing_names.contains(&name) {
                    return Err(format!("toprepo.config.{} configurations found in multiple sources", name));
                }
                existing_names.insert(name);
                queue.push(sub_config_loader);
            }
        }

        Ok(full_configmap)
    }

    fn get_config_loaders(&self, configmap: &ConfigMap, overrides: &ConfigMap) ->
    Result<HashMap<String, Box<dyn ConfigLoader>>, String> {
        let mut config_loaders = HashMap::new();
        // Accumulate toprepo.config.<id>.* keys.
        let own_loader_configmaps = configmap.extract_mapping("toprepo.config");
        let full_loader_configmaps = ConfigMap::join([configmap, overrides].into_iter())
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

    fn get_config_loader(&self, name: &str, configmap: &ConfigMap) -> Result<Box<dyn ConfigLoader>, String> {
        let loader_type = configmap.get_last("type")
            .expect("Missing config loader type").to_lowercase();

        let config_loader: Box<dyn ConfigLoader> = match loader_type.as_str() {
            "none" => Box::new(StaticContentConfigLoader { content: String::new() }),
            "file" => {
                let mut file_path = self.monorepo.path.clone();
                file_path.push(PathBuf::from(
                    configmap.get_last("file").expect("Missing file")
                ));

                Box::new(LocalFileConfigLoader { filename: file_path, allow_missing: false })
            }
            "git" => {
                // Load
                let raw_url = configmap.get_last("url").expect("Missing url");
                let reference = configmap.get_last("ref").expect("Missing ref");
                let filename = configmap.get_last("url").expect("Missing filename");

                // Translate.
                let parent_url = self.monorepo.get_toprepo_fetch_url();

                todo!();

                //  Box::new(todo!())
            }
            _ => panic!(),
        };

        Ok(config_loader)
    }
}