use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub status: VerdictStatus,
    pub notes: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_with_notes_round_trips() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: Some("all clear".to_string()),
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: Verdict = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
        assert!(json.contains("\"pass\""));
        assert!(json.contains("\"all clear\""));
    }

    #[test]
    fn pass_without_notes_round_trips() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            notes: None,
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: Verdict = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn fail_with_notes_round_trips() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            notes: Some("test suite failed".to_string()),
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: Verdict = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
        assert!(json.contains("\"fail\""));
    }

    #[test]
    fn deserializes_from_plain_json() {
        let v: Verdict = serde_json::from_str(r#"{"status":"pass","notes":null}"#).unwrap();
        assert_eq!(v.status, VerdictStatus::Pass);
        assert!(v.notes.is_none());
    }
}
