use crate::config::FolderReplicationRule;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplicationHealth {
    Healthy,
    UnderReplicated,
    OverReplicated,
    Missing,
}

pub struct ReplicationRulesEngine;

impl ReplicationRulesEngine {
    /// Computes the required replica count for a given relative pool path.
    /// Traverses rules matching longest matching prefix first.
    pub fn get_target_replicas(
        relative_path: &str,
        default_replicas: usize,
        rules: &[FolderReplicationRule],
    ) -> usize {
        let normalized = Self::normalize_path(relative_path);

        // Sort rules by pattern length descending so most specific rule matches first
        let mut sorted_rules: Vec<&FolderReplicationRule> = rules
            .iter()
            .filter(|r| r.enabled)
            .collect();
        sorted_rules.sort_by(|a, b| b.path_pattern.len().cmp(&a.path_pattern.len()));

        for rule in sorted_rules {
            let rule_pattern = Self::normalize_path(&rule.path_pattern);
            if Self::matches_pattern(&normalized, &rule_pattern) {
                return rule.replica_count.max(1);
            }
        }

        default_replicas.max(1)
    }

    pub fn normalize_path(path_str: &str) -> String {
        let mut p = path_str.replace('\\', "/");
        if !p.starts_with('/') {
            p.insert(0, '/');
        }
        // Remove trailing slash unless root
        if p.len() > 1 && p.ends_with('/') {
            p.pop();
        }
        p.to_lowercase()
    }

    fn matches_pattern(path: &str, pattern: &str) -> bool {
        if pattern == "/" {
            return true;
        }
        if path == pattern {
            return true;
        }
        // Starts with pattern + "/"
        let prefix = format!("{}/", pattern.trim_end_matches('/'));
        path.starts_with(&prefix)
    }
}
