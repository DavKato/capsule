const TOPICS: &[(&str, &str)] = &[
    ("mental-model", include_str!("../topics/mental-model.md")),
    ("setup-files", include_str!("../topics/setup-files.md")),
    (
        "pipeline-shapes",
        include_str!("../topics/pipeline-shapes.md"),
    ),
    (
        "prompt-writing",
        include_str!("../topics/prompt-writing.md"),
    ),
    ("common-edits", include_str!("../topics/common-edits.md")),
    ("commands", include_str!("../topics/commands.md")),
];

pub fn index() -> &'static str {
    "\
Available topics. Load only what your current task needs.

  mental-model     The Pipeline / Stage / Loop / Verdict / Routing model.
                   Load before reasoning about routing or picking pass/fail/done.

  setup-files      What each file in .capsule/ owns.
                   Load before editing any .capsule/ file.

  pipeline-shapes  Decision tree: single-iter vs ralph-loop vs other.
                   Load before picking a template or proposing a shape change.

  prompt-writing   Verdict contract, note-injection, role framing.
                   Load before authoring or editing a stage prompt.

  common-edits     Rename / add / remove a stage; add a hook.
                   Load before structural changes to config.yml.

  commands         capsule subcommands the agent uses (templates, init, check, run).
                   Load when unsure which command to invoke.

Common task recipes (load multiple topics in one call):

  greenfield setup   → capsule explain mental-model setup-files pipeline-shapes prompt-writing commands
  rename a stage     → capsule explain mental-model setup-files common-edits
  write a stage      → capsule explain mental-model prompt-writing setup-files
  debug routing      → capsule explain mental-model common-edits

For everything at once:
  capsule explain --all
"
}

pub fn load(topics: &[&str]) -> Result<String, String> {
    let mut parts = Vec::new();
    for &name in topics {
        match TOPICS.iter().find(|(n, _)| *n == name) {
            Some((_, content)) => parts.push(*content),
            None => {
                let valid: Vec<&str> = TOPICS.iter().map(|(n, _)| *n).collect();
                return Err(format!(
                    "unknown topic: {name}\nValid topics: {}",
                    valid.join(", ")
                ));
            }
        }
    }
    Ok(parts.join("\n"))
}

pub fn load_all() -> String {
    TOPICS
        .iter()
        .map(|(_, content)| *content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_non_empty() {
        assert!(!index().is_empty());
    }

    #[test]
    fn load_single_topic() {
        let result = load(&["mental-model"]).unwrap();
        assert!(!result.is_empty());
        assert!(result.contains("mental-model"));
    }

    #[test]
    fn load_multi_topic_concatenates_in_order() {
        let a = load(&["mental-model"]).unwrap();
        let b = load(&["setup-files"]).unwrap();
        let combined = load(&["mental-model", "setup-files"]).unwrap();
        assert!(combined.contains(a.trim()));
        assert!(combined.contains(b.trim()));
        // order: mental-model comes before setup-files
        let pos_a = combined.find(a.trim()).unwrap();
        let pos_b = combined.find(b.trim()).unwrap();
        assert!(pos_a < pos_b);
    }

    #[test]
    fn unknown_topic_returns_formatted_error() {
        let err = load(&["nonexistent"]).unwrap_err();
        assert!(err.contains("unknown topic: nonexistent"));
        assert!(err.contains("mental-model"));
        assert!(err.contains("commands"));
    }

    #[test]
    fn load_all_is_superset() {
        let all = load_all();
        for &(name, content) in TOPICS {
            assert!(
                all.contains(content.trim()),
                "load_all missing topic: {name}"
            );
        }
    }
}
