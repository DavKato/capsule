use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Runtime state snapshot used to resume an interrupted pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineState {
    pub current_idx: usize,
    pub global_counter: u32,
    #[serde(alias = "fail_counts")]
    pub retry_counts: HashMap<String, u32>,
    #[serde(default)]
    pub failure_totals: HashMap<String, u32>,
    pub last_stage: Option<String>,
    pub last_verdict: Option<crate::verdict::Verdict>,
    #[serde(with = "string_key_map")]
    pub loop_iterations: HashMap<usize, u32>,
    #[serde(default, with = "env_as_map")]
    pub env: Vec<(String, String)>,
}

/// Serialize/deserialize `HashMap<usize, u32>` with string keys for JSON compat.
mod string_key_map {
    use std::collections::HashMap;

    use serde::de::{self, Deserializer, MapAccess, Visitor};
    use serde::ser::{SerializeMap, Serializer};

    pub fn serialize<S: Serializer>(map: &HashMap<usize, u32>, ser: S) -> Result<S::Ok, S::Error> {
        let mut m = ser.serialize_map(Some(map.len()))?;
        for (k, v) in map {
            m.serialize_entry(&k.to_string(), v)?;
        }
        m.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<HashMap<usize, u32>, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = HashMap<usize, u32>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map with string-encoded usize keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut map = HashMap::new();
                while let Some((k, v)) = access.next_entry::<String, u32>()? {
                    let idx = k.parse::<usize>().map_err(de::Error::custom)?;
                    map.insert(idx, v);
                }
                Ok(map)
            }
        }
        de.deserialize_map(V)
    }
}

/// Serialize `Vec<(String, String)>` as a JSON object for backward compat.
mod env_as_map {
    use serde::de::{Deserializer, MapAccess, Visitor};
    use serde::ser::{SerializeMap, Serializer};

    pub fn serialize<S: Serializer>(
        pairs: &Vec<(String, String)>,
        ser: S,
    ) -> Result<S::Ok, S::Error> {
        let mut m = ser.serialize_map(Some(pairs.len()))?;
        for (k, v) in pairs {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        de: D,
    ) -> Result<Vec<(String, String)>, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Vec<(String, String)>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map of string key-value pairs")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut pairs = Vec::new();
                while let Some((k, v)) = access.next_entry::<String, String>()? {
                    pairs.push((k, v));
                }
                Ok(pairs)
            }
        }
        de.deserialize_map(V)
    }
}
