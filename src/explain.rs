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
    include_str!("../topics/index.md")
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

    #[test]
    fn setup_files_h1_matches_filename() {
        let content = load(&["setup-files"]).unwrap();
        assert!(
            content.trim_start().starts_with("# setup-files"),
            "setup-files topic must start with '# setup-files'"
        );
    }

    #[test]
    fn setup_files_has_load_when_opener() {
        let content = load(&["setup-files"]).unwrap();
        let lower = content.to_lowercase();
        assert!(
            lower.contains("load when") || lower.contains("load before"),
            "setup-files topic must contain 'load when' or 'load before' guidance"
        );
    }

    #[test]
    fn setup_files_covers_all_files() {
        let content = load(&["setup-files"]).unwrap();
        for file in &[
            "config.yml",
            "Dockerfile",
            "before-all.sh",
            "before-each.sh",
            ".env",
            "prompt",
        ] {
            assert!(
                content.contains(file),
                "setup-files topic must mention: {file}"
            );
        }
    }

    #[test]
    fn setup_files_within_line_budget() {
        let content = load(&["setup-files"]).unwrap();
        let lines = content.lines().count();
        assert!(
            lines <= 80,
            "setup-files topic exceeds 80-line budget: {lines} lines"
        );
    }

    #[test]
    fn pipeline_shapes_h1_matches_filename() {
        let content = load(&["pipeline-shapes"]).unwrap();
        assert!(
            content.trim_start().starts_with("# pipeline-shapes"),
            "pipeline-shapes topic must start with '# pipeline-shapes'"
        );
    }

    #[test]
    fn pipeline_shapes_has_load_when_opener() {
        let content = load(&["pipeline-shapes"]).unwrap();
        let lower = content.to_lowercase();
        assert!(
            lower.contains("load when"),
            "pipeline-shapes topic must contain 'load when' guidance"
        );
    }

    #[test]
    fn pipeline_shapes_covers_both_shapes() {
        let content = load(&["pipeline-shapes"]).unwrap();
        assert!(
            content.contains("single-iter"),
            "pipeline-shapes must cover single-iter shape"
        );
        assert!(
            content.contains("ralph-loop"),
            "pipeline-shapes must cover ralph-loop shape"
        );
    }

    #[test]
    fn pipeline_shapes_has_decision_criteria() {
        let content = load(&["pipeline-shapes"]).unwrap();
        let lower = content.to_lowercase();
        assert!(
            lower.contains("decision")
                || lower.contains("when to use")
                || lower.contains("use when"),
            "pipeline-shapes must contain decision criteria for choosing a shape"
        );
        assert!(
            lower.contains("review"),
            "pipeline-shapes decision criteria must mention review gate"
        );
    }

    #[test]
    fn pipeline_shapes_shows_structural_skeletons() {
        let content = load(&["pipeline-shapes"]).unwrap();
        assert!(
            content.contains("stages:"),
            "pipeline-shapes must show stages list in skeleton"
        );
        assert!(
            content.contains("loop:"),
            "pipeline-shapes must show loop presence in skeleton"
        );
        assert!(
            content.contains("iterations:"),
            "pipeline-shapes must show flat-form config skeleton"
        );
    }

    #[test]
    fn pipeline_shapes_explains_template_init() {
        let content = load(&["pipeline-shapes"]).unwrap();
        assert!(
            content.contains("capsule init --template"),
            "pipeline-shapes must explain how templates map to shapes via capsule init --template"
        );
        assert!(
            content.contains("capsule init --template single-iter"),
            "pipeline-shapes must mention capsule init --template single-iter"
        );
        assert!(
            content.contains("capsule init --template ralph-loop"),
            "pipeline-shapes must mention capsule init --template ralph-loop"
        );
    }

    #[test]
    fn pipeline_shapes_within_line_budget() {
        let content = load(&["pipeline-shapes"]).unwrap();
        let lines = content.lines().count();
        assert!(
            lines <= 80,
            "pipeline-shapes topic exceeds 80-line budget: {lines} lines"
        );
    }

    #[test]
    fn index_lists_all_canonical_topics_with_load_when_guidance() {
        let idx = index();
        for &(name, _) in TOPICS {
            assert!(idx.contains(name), "index missing topic: {name}");
        }
        assert!(
            idx.contains("Load before") || idx.contains("Load when"),
            "index must contain load-when guidance"
        );
        assert!(
            idx.contains("capsule explain --all"),
            "index must contain --all pointer"
        );
        assert!(
            idx.contains("Common task recipes"),
            "index must contain recipes section"
        );
    }
}
