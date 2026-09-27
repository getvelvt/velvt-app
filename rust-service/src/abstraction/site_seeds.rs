//! The curated site table: data only.
//!
//! `sites.rs` holds everything that reads it, and the tests there hold every
//! entry to the contract: a host exactly as `site_identity` normalizes one,
//! listed once, a valid local label, a taxonomy category other than
//! `UNLOGGED`, and the category of any browser-context rule that names the
//! host.

use super::sites::{SiteScope, SiteSeed};

#[rustfmt::skip]
pub(crate) const SITE_SEEDS: &[SiteSeed] = &[
    SiteSeed { host: "github.com", scope: SiteScope::WithSubdomains, label: "reference:github", category: "REFERENCE" },
    SiteSeed { host: "gitlab.com", scope: SiteScope::WithSubdomains, label: "reference:gitlab", category: "REFERENCE" },
    SiteSeed { host: "stackoverflow.com", scope: SiteScope::WithSubdomains, label: "reference:stack_overflow", category: "REFERENCE" },
    SiteSeed { host: "wikipedia.org", scope: SiteScope::WithSubdomains, label: "reference:wikipedia", category: "REFERENCE" },
    SiteSeed { host: "developer.mozilla.org", scope: SiteScope::HostOnly, label: "reference:read", category: "REFERENCE" },
    SiteSeed { host: "docs.rs", scope: SiteScope::HostOnly, label: "reference:read", category: "REFERENCE" },
    SiteSeed { host: "docs.google.com", scope: SiteScope::HostOnly, label: "document:docs", category: "FOCUS_WORK" },
    SiteSeed { host: "drive.google.com", scope: SiteScope::HostOnly, label: "document:drive", category: "REFERENCE" },
    SiteSeed { host: "mail.google.com", scope: SiteScope::HostOnly, label: "communication:gmail", category: "COMMUNICATION" },
    SiteSeed { host: "calendar.google.com", scope: SiteScope::HostOnly, label: "communication:calendar", category: "COMMUNICATION" },
    SiteSeed { host: "meet.google.com", scope: SiteScope::HostOnly, label: "meeting:meet", category: "COMMUNICATION" },
    SiteSeed { host: "youtube.com", scope: SiteScope::WithSubdomains, label: "video:youtube", category: "PASSIVE_CONSUMPTION" },
    SiteSeed { host: "netflix.com", scope: SiteScope::WithSubdomains, label: "video:netflix", category: "PASSIVE_CONSUMPTION" },
    SiteSeed { host: "reddit.com", scope: SiteScope::WithSubdomains, label: "social:reddit", category: "SOCIAL_FEED" },
    SiteSeed { host: "x.com", scope: SiteScope::WithSubdomains, label: "social:x", category: "SOCIAL_FEED" },
    SiteSeed { host: "twitter.com", scope: SiteScope::WithSubdomains, label: "social:x", category: "SOCIAL_FEED" },
    SiteSeed { host: "instagram.com", scope: SiteScope::WithSubdomains, label: "social:instagram", category: "SOCIAL_FEED" },
    SiteSeed { host: "linear.app", scope: SiteScope::WithSubdomains, label: "task:manage", category: "TASK_MANAGEMENT" },
    SiteSeed { host: "app.asana.com", scope: SiteScope::HostOnly, label: "task:manage", category: "TASK_MANAGEMENT" },
    SiteSeed { host: "notion.so", scope: SiteScope::WithSubdomains, label: "document:write", category: "FOCUS_WORK" },
    SiteSeed { host: "app.slack.com", scope: SiteScope::HostOnly, label: "communication:slack", category: "COMMUNICATION" },
    SiteSeed { host: "chatgpt.com", scope: SiteScope::WithSubdomains, label: "reference:ai_assistant", category: "REFERENCE" },
    SiteSeed { host: "claude.ai", scope: SiteScope::WithSubdomains, label: "reference:ai_assistant", category: "REFERENCE" },
];
