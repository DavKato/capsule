use std::collections::HashMap;

use anyhow::{anyhow, Context};

/// Runtime state snapshot used to resume an interrupted pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineState {
    pub current_idx: usize,
    pub global_counter: u32,
    pub fail_counts: HashMap<String, u32>,
    pub last_stage: Option<String>,
    pub last_verdict: Option<crate::verdict::Verdict>,
    pub loop_iterations: HashMap<usize, u32>,
    /// Run environment pairs persisted for resume; omitted from disk on successful exits.
    pub env: Vec<(String, String)>,
}

impl PipelineState {
    pub fn to_json(&self) -> serde_json::Value {
        let fail_counts: serde_json::Map<String, serde_json::Value> = self
            .fail_counts
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();
        let loop_iterations: serde_json::Map<String, serde_json::Value> = self
            .loop_iterations
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect();
        let last_verdict = self
            .last_verdict
            .as_ref()
            .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
        let env: serde_json::Map<String, serde_json::Value> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect();
        serde_json::json!({
            "current_idx": self.current_idx,
            "global_counter": self.global_counter,
            "fail_counts": fail_counts,
            "last_stage": self.last_stage,
            "last_verdict": last_verdict,
            "loop_iterations": loop_iterations,
            "env": env,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> anyhow::Result<Self> {
        let current_idx = v["current_idx"]
            .as_u64()
            .ok_or_else(|| anyhow!("pipeline_state.current_idx missing or invalid"))?
            as usize;
        let global_counter = v["global_counter"]
            .as_u64()
            .ok_or_else(|| anyhow!("pipeline_state.global_counter missing or invalid"))?
            as u32;
        let fail_counts: HashMap<String, u32> = v["fail_counts"]
            .as_object()
            .ok_or_else(|| anyhow!("pipeline_state.fail_counts missing or not an object"))?
            .iter()
            .map(|(k, val)| {
                val.as_u64()
                    .ok_or_else(|| anyhow!("pipeline_state.fail_counts[{}] is not a number", k))
                    .map(|n| (k.clone(), n as u32))
            })
            .collect::<anyhow::Result<_>>()?;
        let last_stage = v["last_stage"].as_str().map(str::to_owned);
        let last_verdict: Option<crate::verdict::Verdict> = if v["last_verdict"].is_null() {
            None
        } else {
            Some(
                serde_json::from_value(v["last_verdict"].clone())
                    .context("pipeline_state.last_verdict is malformed")?,
            )
        };
        let loop_iterations: HashMap<usize, u32> = v["loop_iterations"]
            .as_object()
            .ok_or_else(|| anyhow!("pipeline_state.loop_iterations missing or not an object"))?
            .iter()
            .map(|(k, val)| {
                let ki = k.parse::<usize>().map_err(|_| {
                    anyhow!(
                        "pipeline_state.loop_iterations key {:?} is not a valid index",
                        k
                    )
                })?;
                let vi = val.as_u64().ok_or_else(|| {
                    anyhow!("pipeline_state.loop_iterations[{}] is not a number", k)
                })? as u32;
                Ok((ki, vi))
            })
            .collect::<anyhow::Result<_>>()?;
        // env defaults to empty for backward compat (pre-ADR-0006 files lack this field)
        let env: Vec<(String, String)> = v["env"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .map(|(k, val)| {
                        let s = val
                            .as_str()
                            .ok_or_else(|| anyhow!("pipeline_state.env[{}] is not a string", k))?;
                        Ok((k.clone(), s.to_owned()))
                    })
                    .collect::<anyhow::Result<_>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            current_idx,
            global_counter,
            fail_counts,
            last_stage,
            last_verdict,
            loop_iterations,
            env,
        })
    }
}
