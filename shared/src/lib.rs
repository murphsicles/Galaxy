use toml;
use std::collections::HashMap;

#[derive(Debug)]
pub struct ShardManager {
    shard_id: u32,
    shard_count: u32,
    node_map: HashMap<u32, String>,
}

impl ShardManager {
    pub fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;
        let shard_count = config["sharding"]["shard_count"].as_integer().unwrap_or(1) as u32;
        let node_map = config["sharding"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v.as_str().unwrap().to_string()))
            .collect();

        ShardManager {
            shard_id,
            shard_count,
            node_map,
        }
    }

    pub fn get_shard_id(&self) -> u32 {
        self.shard_id
    }

    pub fn get_node_for_shard(&self, shard_id: u32) -> Option<&str> {
        self.node_map.get(&shard_id).map(|s| s.as_str())
    }

    pub fn is_valid_shard(&self, shard_id: u32) -> bool {
        shard_id < self.shard_count
    }
}
