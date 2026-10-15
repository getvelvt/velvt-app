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

/// The text an insight is shown with.
///
/// The cloud sends `text`, its own rendering, and may also send `template`,
/// copy with `{token}`s, and `label_references` to fill them with the names
/// this Mac holds. A template is rendered here only when it has references to
/// fill and is copy rather than an identifier. velvt-core stores the template
/// id (`reference_activity`, `stable_focus_period`) as the template of every
/// template insight, with no references, and rendering that put the id in the
/// notification (1.0.14 simulation, critic-01). Otherwise the cloud's own text
/// is used: with nothing to fill, it is the same sentence.
pub fn insight_text(
    text: String,
    template: Option<String>,
    references: Option<Vec<InsightLabelReference>>,
    rehydrator: Option<&LocalInsightRehydrator>,
) -> String {
    match (template, references, rehydrator) {
        (Some(template), Some(references), Some(rehydrator))
            if !references.is_empty() && !looks_like_template_id(&template) =>
        {
            rehydrator.rehydrate(&template, &references)
        }
        _ => text,
    }
}

/// Whether `value` is an identifier, such as a template id, rather than copy
/// a person could read: one run of lowercase ASCII letters, digits and
/// underscores. Every sentence Velvt writes has a space or a capital in it.
pub fn looks_like_template_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::SqlitePersistence;

    fn rehydrator() -> LocalInsightRehydrator {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        LocalInsightRehydrator::new(persistence.abstraction_map_repo())
    }

    fn reference(token: &str, label: &str) -> InsightLabelReference {
        InsightLabelReference {
            token: token.into(),
            label: label.into(),
        }
    }

    const TEXT: &str = "Reference work accounted for 15 minutes today.";

    /// What velvt-core sends for every template insight: the template id as
    /// the template, and no references. Rendering it showed the id.
    #[test]
    fn a_template_id_with_no_references_is_never_shown() {
        let rehydrator = rehydrator();
        for (template, references) in [
            ("reference_activity", Some(vec![])),
            ("stable_focus_period", Some(vec![])),
            ("stable_focus_period", None),
            (
                "repeated_switching_loop",
                Some(vec![reference("app", "communication:chat")]),
            ),
        ] {
            assert_eq!(
                insight_text(
                    TEXT.into(),
                    Some(template.into()),
                    references,
                    Some(&rehydrator)
                ),
                TEXT,
                "{template}"
            );
        }
    }

    /// Copy with references to fill is still rendered on this Mac.
    #[test]
    fn a_template_with_references_is_rendered_here() {
        let rehydrator = rehydrator();
        let rendered = insight_text(
            "You switched to a communication app often.".into(),
            Some("You switched to {app} often.".into()),
            Some(vec![reference("app", "communication:chat")]),
            Some(&rehydrator),
        );
        assert_eq!(rendered, "You switched to a communication app often.");
    }

    /// Copy that names nothing renders to the cloud's own sentence anyway, so
    /// the cloud's text is used rather than the template.
    #[test]
    fn copy_with_nothing_to_fill_uses_the_clouds_text() {
        assert_eq!(
            insight_text(
                TEXT.into(),
                Some(TEXT.into()),
                Some(vec![]),
                Some(&rehydrator())
            ),
            TEXT
        );
        assert_eq!(insight_text(TEXT.into(), None, None, None), TEXT);
    }

    #[test]
    fn identifiers_are_told_from_copy() {
        for id in [
            "reference_activity",
            "stable_focus_period",
            "unavailable",
            "v2_loop",
        ] {
            assert!(looks_like_template_id(id), "{id}");
        }
        for copy in [
            TEXT,
            "Your longest uninterrupted work block was 40 minutes today.",
            "Focus",
            "",
            "   ",
        ] {
            assert!(!looks_like_template_id(copy), "{copy:?}");
        }
    }
}
