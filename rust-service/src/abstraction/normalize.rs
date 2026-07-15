/// Canonical local preprocessing for every classifier input.
///
/// This deliberately uses ASCII case-folding because the shipped dictionary and
/// rules are ASCII. Punctuation becomes a token boundary and whitespace is
/// collapsed so exact, heuristic, and embedding tiers see the same text.
pub(crate) fn normalize_classifier_text(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn normalize_classifier_input(app_name: &str, window_title: &str) -> String {
    normalize_classifier_text(&format!("{app_name} {window_title}"))
}

pub(crate) fn contains_token_phrase(haystack: &str, phrase: &str) -> bool {
    let normalized_phrase = normalize_classifier_text(phrase);
    if normalized_phrase.is_empty() {
        return false;
    }
    let padded_haystack = format!(" {haystack} ");
    let padded_phrase = format!(" {normalized_phrase} ");
    padded_haystack.contains(&padded_phrase)
}

#[cfg(test)]
mod tests {
    use super::{contains_token_phrase, normalize_classifier_text};

    #[test]
    fn normalization_trims_casefolds_and_collapses_punctuation() {
        assert_eq!(
            normalize_classifier_text("  Google.Docs — Draft  "),
            "google docs draft"
        );
    }

    #[test]
    fn token_phrases_do_not_match_inside_words() {
        assert!(contains_token_phrase("mail inbox", "mail"));
        assert!(!contains_token_phrase("email inbox", "mail"));
        assert!(!contains_token_phrase("maximum effort", "max"));
    }
}
