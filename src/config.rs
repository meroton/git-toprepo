use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use itertools::Itertools;

const CONFIG_DICT_UNSET: &str = "git_toprepo_ConfigDict_unset";

#[derive(Debug)]
pub struct ConfigMap {
    map: HashMap<String, Vec<String>>,
}

#[allow(dead_code)]
impl ConfigMap {
    pub fn new() -> ConfigMap {
        ConfigMap { map: HashMap::new() }
    }

    pub fn push(&mut self, key: &str, mut values: Vec<String>) {
        if !self.map.contains_key(key) {
            self.map.insert(key.to_string(), values);
        } else {
            self.map.get_mut(key).unwrap().append(&mut values);
        }
    }

    pub fn join<T: Iterator<Item=ConfigMap>>(configs: T) -> ConfigMap {
        let mut ret = ConfigMap::new();

        for config in configs {
            for (key, values) in config.map.into_iter() {
                ret.push(&key, values);
            }
        }

        ret
    }

    pub fn parse(&mut self, config_lines: &str) -> Result<ConfigMap, String> {
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

    /// Extracts for example submodule.<name>.<key>=<value>.
    /// All entries that dont contain the prefix are returned in the residual
    pub fn extract_mapping(self, prefix: &str) -> (HashMap<String, ConfigMap>, ConfigMap) {
        let mut prefix = prefix.to_string();
        if !prefix.ends_with('.') {
            prefix.push('.');
        }

        let mut extracted = HashMap::new();
        let mut residual = ConfigMap::new();

        for (key, values) in self.map.into_iter() {
            if let Some(temp) = key.strip_prefix(&prefix) {
                if let Some((name, subkey)) = temp.split(".").next_tuple() {
                    if !extracted.contains_key(name) {
                        extracted.insert(name.to_string(), ConfigMap::new());
                    }

                    extracted.get_mut(name).unwrap().push(subkey, values);
                } else {
                    panic!() //Is this reachable?
                }
            } else {
                residual.push(&key, values);
            }
        }

        (extracted, residual)
    }

    pub fn get_singleton<'t>(&'t self, key: &str, default: Option<&'t str>) -> Result<&str, String> {
        if !self.map.contains_key(key) {
            return Ok(default.unwrap_or(CONFIG_DICT_UNSET));
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