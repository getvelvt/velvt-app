use std::sync::Arc;

use serde::Deserialize;

use crate::persistence::AbstractionMapRepo;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InsightLabelReference {
    pub token: String,
    pub label: String,
}

/// Device-only renderer for structured cloud insight templates.
pub struct LocalInsightRehydrator {
    mappings: Arc<dyn AbstractionMapRepo>,
}

impl std::fmt::Debug for LocalInsightRehydrator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LocalInsightRehydrator")
    }
}

impl LocalInsightRehydrator {
    pub fn new(mappings: Arc<dyn AbstractionMapRepo>) -> Self {
        Self { mappings }
    }

    pub fn rehydrate(&self, template: &str, references: &[InsightLabelReference]) -> String {
        let mut rendered = template.to_owned();
        for reference in references {
            let replacement = self
                .mappings
                .display_name_for_label(&reference.label)
                .ok()
                .flatten()
                .unwrap_or_else(|| generic_label(&reference.label));
            rendered = rendered.replace(&format!("{{{}}}", reference.token), &replacement);
        }
        rendered
    }
}

fn generic_label(label: &str) -> String {
    match label.split(':').next().unwrap_or_default() {
        "communication" => "a communication app".into(),
        "meeting" => "a meeting app".into(),
        "document" => "document work".into(),
        "video" => "video activity".into(),
        "audio" => "audio activity".into(),
        "social" => "a social feed".into(),
        "task" => "task management".into(),
        "reference" => "reference work".into(),
        "design" => "design work".into(),
        "creative" => "creative work".into(),
        "system" => "system activity".into(),
        _ => "an activity".into(),
    }
}
