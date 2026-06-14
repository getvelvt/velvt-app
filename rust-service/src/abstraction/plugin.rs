use super::{RawKey, SeedApplication};

/// Privacy-safe label and payload schema selected by a plugin.
pub type PluginMatch = (String, &'static str);

/// Classifies a local-only raw key without controlling engine routing.
pub trait AbstractionPlugin: Send + Sync {
    /// Lower values run first.
    fn priority(&self) -> u32;

    /// Returns a privacy-safe abstract label and static payload schema.
    fn classify(&self, raw_key: &RawKey) -> Option<PluginMatch>;
}

pub(crate) struct AppTitlePlugin {
    entries: Vec<SeedApplication>,
}

impl AppTitlePlugin {
    pub(crate) fn new(entries: Vec<SeedApplication>) -> Self {
        Self { entries }
    }
}

impl AbstractionPlugin for AppTitlePlugin {
    fn priority(&self) -> u32 {
        100
    }

    fn classify(&self, raw_key: &RawKey) -> Option<PluginMatch> {
        self.entries
            .iter()
            .find(|entry| entry.app_name.eq_ignore_ascii_case(raw_key.app_name()))
            .map(|entry| (entry.label.clone(), "label-v1"))
    }
}

pub(crate) struct UnloggedPlugin;

impl AbstractionPlugin for UnloggedPlugin {
    fn priority(&self) -> u32 {
        u32::MAX
    }

    fn classify(&self, _raw_key: &RawKey) -> Option<PluginMatch> {
        Some(("unlogged:unknown".to_owned(), "label-v1"))
    }
}
