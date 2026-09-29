//! A browser tab's site: its normalized identity, the curated seed table's
//! matcher, and the two classifier tiers that read the host rather than the
//! words of the tab title.

use std::collections::HashMap;

use velvt_shared_types::{ClassificationConfidence, ClassificationSource, ClassificationStatus};

use super::{
    plugin::{
        browser_context_verdict, inferred_label_for_category, is_browser_app, ClassificationPlugin,
        ClassificationResult, ClassificationTier, DeclaredMetadata,
    },
    site_seeds::SITE_SEEDS,
};

/// The identity of a site: what the seed table matches, what the site key
/// hashes, and the name a person is shown when Velvt asks about it.
///
/// `host` is what `focused_site_context` kept of a tab's URL. Exactly one
/// leading `www.` is stripped, because a site serves the same thing with and
/// without it and a rule taught on one must reach the other. There is no
/// identity for an IP literal (a dotted quad, or anything with a `:`), a host
/// without a dot, or a name under one of [`PRIVATE_NETWORK_NAMES`] --
/// `localhost`, `.local`, `home.arpa`, `.internal`, `.lan`, `.localdomain`:
/// each of those names a machine rather than a site, usually a different
/// machine on each network, so a rule about one would follow whatever answers
/// at that name next.
///
/// The result is at most 253 characters of `[a-z0-9.-]` with no empty label,
/// which is exactly what the `local_site_name.host` CHECK (migration 0040)
/// admits.
pub(crate) fn site_identity(host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    let site = host.strip_prefix("www.").unwrap_or(&host);
    let well_formed = !site.is_empty()
        && site.len() <= 253
        && site.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
        && site.split('.').all(|label| !label.is_empty());
    let names_a_site = site.contains('.')
        && !PRIVATE_NETWORK_NAMES.iter().any(|name| {
            site == *name
                || site
                    .strip_suffix(name)
                    .is_some_and(|under| under.ends_with('.'))
        })
        // The WHATWG URL standard reads a host whose last label is all digits
        // as an IPv4 address, and no top-level domain is numeric.
        && !site
            .rsplit('.')
            .next()
            .is_some_and(|label| label.bytes().all(|byte| byte.is_ascii_digit()));
    (well_formed && names_a_site).then(|| site.to_owned())
}

/// Names only a private network answers, each covering itself and every name
/// under it: `localhost` (RFC 6761), `.local` (multicast DNS), `home.arpa`
/// (RFC 8375, a home network), `.internal` (reserved by ICANN for private
/// use), and the `.lan` and `.localdomain` that home routers and Linux hosts
/// commonly hand out.
const PRIVATE_NETWORK_NAMES: &[&str] = &[
    "localhost",
    "local",
    "home.arpa",
    "internal",
    "lan",
    "localdomain",
];

/// How far a seed's host reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SiteScope {
    /// The normalized host exactly. `www.` is stripped before matching, so
    /// `docs.rs` also covers `www.docs.rs`, and nothing else.
    HostOnly,
    /// The host and every host under it: `wikipedia.org` covers
    /// `en.wikipedia.org`.
    WithSubdomains,
}

/// One curated site, and what a browser tab on it is.
///
/// The table (`site_seeds.rs`) is compiled in rather than read from the
/// taxonomy file, so it changes no taxonomy version and nothing the backend
/// registers. What `every_site_seed_is_well_formed` asserts of each entry is
/// the contract an edit has to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SiteSeed {
    /// A normalized host, as [`site_identity`] returns one.
    pub(crate) host: &'static str,
    pub(crate) scope: SiteScope,
    /// The local `<type>:<behavior>` label. Never uploaded: the serializer
    /// sends a category-scoped type in its place.
    pub(crate) label: &'static str,
    /// A taxonomy category, never the default `UNLOGGED`.
    pub(crate) category: &'static str,
}

/// A seed table, indexed by host once, when the tier that reads it is built.
pub(crate) struct SiteMatcher<'a> {
    by_host: HashMap<&'a str, &'a SiteSeed>,
}

impl<'a> SiteMatcher<'a> {
    pub(crate) fn new(seeds: &'a [SiteSeed]) -> Self {
        let mut by_host = HashMap::with_capacity(seeds.len());
        for seed in seeds {
            // The table test refuses a host listed twice; keeping the first
            // entry only fixes what a broken table would do at run time.
            by_host.entry(seed.host).or_insert(seed);
        }
        Self { by_host }
    }

    /// The seed that reaches a normalized site, if one does.
    ///
    /// Walks the site's suffixes at dot boundaries from the longest to the
    /// shortest -- `a.example.com`, `example.com`, `com` -- and answers with the
    /// first seed that reaches the site: any seed for the site itself, or a
    /// `WithSubdomains` seed for a suffix of it. A `HostOnly` seed for a suffix
    /// does not reach the site, and the walk goes on past it. Longest first is
    /// what lets `mail.google.com` say something different from whatever
    /// `google.com` says.
    pub(crate) fn lookup(&self, site: &str) -> Option<&'a SiteSeed> {
        let mut suffix = site;
        loop {
            if let Some(seed) = self.by_host.get(suffix) {
                if suffix.len() == site.len() || seed.scope == SiteScope::WithSubdomains {
                    return Some(seed);
                }
            }
            suffix = suffix.split_once('.')?.1;
        }
    }
}

/// Tier 1 for a browser tab: the curated site table.
///
/// A browser window's identity is its site, and until this tier the site
/// reached the classifiers only as the first words of the window context,
/// where [`super::plugin::BrowserContextPlugin`]'s keyword rules see the host
/// and the tab title as one run of words. A title could outvote the host there
/// -- `github.com` with a title that mentions YouTube resolves to UNLOGGED --
/// and a host the rules have no keyword for is left to the tiers below. Here the host alone decides the
/// category, with a seed's confidence: a curated statement about one site, as
/// a name seed is about one application.
///
/// The browser-context rules may refine the label in one listed case,
/// [`SEED_LABEL_REFINEMENTS`]. Google serves Docs, Sheets and Slides from
/// `docs.google.com`, so only the title names the product, and a rule verdict
/// of `document:sheets` or `document:slides` replaces the seed's
/// `document:docs`. Every other verdict is ignored: title words never move a
/// seeded site's category, and never swap its label for another product's --
/// a Wikipedia article about GitHub is still Wikipedia.
pub(crate) struct SiteSeedPlugin {
    seeds: SiteMatcher<'static>,
    taxonomy_version: String,
}

impl SiteSeedPlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self::with_seeds(SITE_SEEDS, taxonomy_version)
    }

    pub(crate) fn with_seeds(seeds: &'static [SiteSeed], taxonomy_version: String) -> Self {
        Self {
            seeds: SiteMatcher::new(seeds),
            taxonomy_version,
        }
    }
}

impl ClassificationPlugin for SiteSeedPlugin {
    fn classify(&self, _app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        // No site, nothing to key on: the metadata-free path never carries one.
        None
    }

    fn classify_declared(
        &self,
        app_name: &str,
        window_title: &str,
        declared: DeclaredMetadata<'_>,
    ) -> Option<ClassificationResult> {
        if !is_browser_app(app_name) {
            return None;
        }
        let site = site_identity(declared.site?)?;
        let seed = self.seeds.lookup(&site)?;
        let label = browser_context_verdict(app_name, window_title, &self.taxonomy_version)
            .filter(|refined| {
                refined.status() == ClassificationStatus::Classified
                    && refined.category() == seed.category
                    && refines_seed_label(seed.label, refined.label())
            })
            .map_or_else(
                || seed.label.to_owned(),
                |refined| refined.label().to_owned(),
            );
        Some(ClassificationResult::new(
            label,
            seed.category,
            &self.taxonomy_version,
            ClassificationTier::ExactMatch,
        ))
    }
}

/// The only labels a browser-context verdict may put in place of a seed's
/// own: a seed label, and the labels that can replace it. Each is a product
/// the host cannot tell apart from its siblings and the title names, in the
/// seed's category.
///
/// A rule verdict in the seed's category is not enough on its own. The rules
/// name products in titles as well as hosts, so without this list a Stack
/// Overflow question about GitHub was labelled GitHub, and a rule that also
/// matches the host (`developer mozilla org` in `reference:read`) replaced a
/// curated label (`reference:mdn`) on every visit.
const SEED_LABEL_REFINEMENTS: &[(&str, &[&str])] =
    &[("document:docs", &["document:sheets", "document:slides"])];

fn refines_seed_label(seed_label: &str, label: &str) -> bool {
    SEED_LABEL_REFINEMENTS
        .iter()
        .any(|(from, to)| *from == seed_label && to.contains(&label))
}

/// The long tail: a site no seed names, read from its own hostname.
///
/// Three signals, each a vote for one category or for nothing -- a subdomain
/// label that names a purpose ([`SUBDOMAIN_LABEL_VOTES`]), a public suffix only
/// an institution can register under ([`REFERENCE_TOP_LEVEL_DOMAINS`],
/// [`REFERENCE_SECOND_LEVEL_DOMAINS`]), and a token of the registrable label
/// itself ([`REGISTRABLE_TOKEN_VOTES`]). At least one vote, and every vote the
/// same, classifies at Medium confidence as a heuristic. Any disagreement is no
/// answer: the window falls through to the embedding tier and the browser
/// prior, exactly as it did before this tier existed. A host with a sign-in
/// label in front ([`ACCESS_LABELS`]) is SYSTEM, whatever the other signals
/// say: it is the door to a site rather than the site, and SYSTEM is what a
/// sign-in page is. The drift gate never counts SYSTEM, and the "needs a
/// category" list treats it as categorized, so such a host is neither
/// evidence nor a question, and its name is never kept.
///
/// Registered after the two declared-metadata tiers and before the embedding
/// tier. It answers only for a browser window, which both declared tiers
/// refuse, so none of them ever has an answer for the same event.
pub(crate) struct SiteInferencePlugin {
    seeds: SiteMatcher<'static>,
    taxonomy_version: String,
}

impl SiteInferencePlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self {
            seeds: SiteMatcher::new(SITE_SEEDS),
            taxonomy_version,
        }
    }
}

impl ClassificationPlugin for SiteInferencePlugin {
    fn classify(&self, _app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        None
    }

    fn classify_declared(
        &self,
        app_name: &str,
        _window_title: &str,
        declared: DeclaredMetadata<'_>,
    ) -> Option<ClassificationResult> {
        if !is_browser_app(app_name) {
            return None;
        }
        let site = site_identity(declared.site?)?;
        // A seeded site is the seed tier's to answer, at the seed's confidence;
        // an inference about it would only ever be a weaker copy or a
        // contradiction.
        if self.seeds.lookup(&site).is_some() {
            return None;
        }
        let category = inferred_site_category(&site)?;
        Some(ClassificationResult::with_quality(
            inferred_label_for_category(category)?,
            category,
            &self.taxonomy_version,
            ClassificationTier::LocalPurposeHeuristic,
            ClassificationStatus::Classified,
            ClassificationConfidence::Medium,
            ClassificationSource::Heuristic,
        ))
    }
}

// The site inference signals, all in one place. Each list is a starting point
// and every entry moves every site that carries it, so an edit is a change to
// be measured across many sites rather than a fix for the one in front of you.

/// S1: a whole label left of the registrable domain that says what the host is
/// for -- `mail` in `mail.example.com`, `docs` in `docs.example.co.uk`.
const SUBDOMAIN_LABEL_VOTES: &[(&str, &[&str])] = &[
    (
        "COMMUNICATION",
        &[
            "mail", "webmail", "email", "inbox", "calendar", "meet", "chat", "messages",
        ],
    ),
    (
        "REFERENCE",
        &[
            "docs",
            "doc",
            "documentation",
            "developer",
            "developers",
            "reference",
            "learn",
            "wiki",
            "kb",
            "knowledge",
            "knowledgebase",
            "help",
            "support",
            "manual",
            "guide",
            "guides",
            "handbook",
            // A code forge a company or project runs itself (GitHub
            // Enterprise, a self-hosted GitLab), which the seeds for
            // github.com and gitlab.com cannot reach.
            "github",
            "gitlab",
        ],
    ),
    ("TASK_MANAGEMENT", &["jira", "tasks", "tracker", "issues"]),
];

/// S2: top-level domains only a school, a government or a military can
/// register under. A vote for REFERENCE.
const REFERENCE_TOP_LEVEL_DOMAINS: &[&str] = &["edu", "gov", "mil"];

/// S2, under a country code: the second-level labels that mean the same
/// thing there -- `.ac.uk`, `.edu.au`, `.gov.in`. A vote for REFERENCE when the
/// last label is two letters.
const REFERENCE_SECOND_LEVEL_DOMAINS: &[&str] = &["ac", "edu", "gov"];

/// Overrides all three: a whole label left of the registrable domain that
/// names a sign-in page -- `weblogin` in `weblogin.example.edu`, `sso` in
/// `sso.example.ac.uk`. What such a host is for is getting somewhere else, and
/// S2 alone would file every university's sign-in page as REFERENCE.
///
/// `proxy` is deliberately absent. A library proxy rewrites the publisher's
/// host into its own (`www-nature-com.proxy.lib.example.edu`), so what sits
/// behind that label is usually the paper being read, which S2 files as
/// REFERENCE.
const ACCESS_LABELS: &[&str] = &[
    "login",
    "logon",
    "signin",
    "sso",
    "auth",
    "idp",
    "shibboleth",
    "cas",
    "weblogin",
    "vpn",
    "webauth",
];

/// S3: a hyphen-separated token of the registrable label -- `wiki` in
/// `arch-wiki.org`, `mail` in `mail.com`.
const REGISTRABLE_TOKEN_VOTES: &[(&str, &[&str])] = &[
    ("REFERENCE", &["wiki", "docs"]),
    ("COMMUNICATION", &["mail"]),
];

/// Public suffixes of two labels, under which a registrable domain is three
/// labels long (`example.co.uk`). Under any other suffix it is the last two.
///
/// Not the Public Suffix List: this only has to put S1's and S3's boundary in
/// the right place for the common country-code second levels. A suffix missing
/// here moves the boundary by one label -- the registrable label is then read
/// whole against S1's list instead of by token against S3's -- which can add
/// or drop that one label's vote.
const MULTI_LABEL_PUBLIC_SUFFIXES: &[&str] = &[
    "co.uk", "ac.uk", "gov.uk", "org.uk", "me.uk", "nhs.uk", "com.au", "net.au", "org.au",
    "edu.au", "gov.au", "co.jp", "ac.jp", "go.jp", "or.jp", "ne.jp", "co.nz", "ac.nz", "govt.nz",
    "org.nz", "co.in", "ac.in", "gov.in", "org.in", "com.br", "gov.br", "org.br", "com.cn",
    "edu.cn", "gov.cn", "org.cn", "co.kr", "ac.kr", "go.kr", "com.sg", "edu.sg", "gov.sg",
    "com.hk", "edu.hk", "gov.hk", "com.tw", "edu.tw", "gov.tw", "co.za", "ac.za", "gov.za",
    "com.mx", "edu.mx", "com.tr", "edu.tr", "gov.tr", "co.il", "ac.il", "gov.il",
];

/// A normalized site, split where its registrable domain begins.
struct SiteParts<'a> {
    /// Every label left of the registrable domain (`a`, `docs` in
    /// `a.docs.example.co.uk`).
    subdomain_labels: &'a [&'a str],
    /// The one label the registrant chose (`example`).
    registrable_label: &'a str,
}

fn split_site<'a>(site: &str, labels: &'a [&'a str]) -> Option<SiteParts<'a>> {
    let suffix_labels = if MULTI_LABEL_PUBLIC_SUFFIXES.iter().any(|suffix| {
        site.strip_suffix(suffix)
            .is_some_and(|registrable| registrable.ends_with('.'))
    }) {
        2
    } else {
        1
    };
    let registrable_index = labels.len().checked_sub(suffix_labels + 1)?;
    Some(SiteParts {
        subdomain_labels: &labels[..registrable_index],
        registrable_label: labels[registrable_index],
    })
}

/// The one category every signal that fired agrees on, or `None`.
fn inferred_site_category(site: &str) -> Option<&'static str> {
    let labels: Vec<&str> = site.split('.').collect();
    let parts = split_site(site, &labels)?;
    if parts
        .subdomain_labels
        .iter()
        .any(|label| ACCESS_LABELS.contains(label))
    {
        return Some("SYSTEM");
    }
    let subdomain_votes = parts
        .subdomain_labels
        .iter()
        .filter_map(|label| vote(SUBDOMAIN_LABEL_VOTES, label));
    let suffix_vote = institutional_suffix(&labels).then_some("REFERENCE");
    let registrable_votes = parts
        .registrable_label
        .split('-')
        .filter_map(|token| vote(REGISTRABLE_TOKEN_VOTES, token));
    let mut votes = subdomain_votes.chain(suffix_vote).chain(registrable_votes);
    let first = votes.next()?;
    votes.all(|other| other == first).then_some(first)
}

fn vote(table: &[(&'static str, &[&str])], token: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(_, tokens)| tokens.contains(&token))
        .map(|(category, _)| *category)
}

fn institutional_suffix(labels: &[&str]) -> bool {
    let Some((last, rest)) = labels.split_last() else {
        return false;
    };
    REFERENCE_TOP_LEVEL_DOMAINS.contains(last)
        || (last.len() == 2
            && last.bytes().all(|byte| byte.is_ascii_lowercase())
            && rest
                .last()
                .is_some_and(|second| REFERENCE_SECOND_LEVEL_DOMAINS.contains(second)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use velvt_shared_types::{
        ClassificationConfidence, ClassificationSource, ClassificationStatus,
    };

    use super::{
        inferred_site_category, site_identity, SiteInferencePlugin, SiteMatcher, SiteScope,
        SiteSeed, SiteSeedPlugin, SITE_SEEDS,
    };
    use crate::abstraction::{
        normalize::normalize_classifier_text,
        plugin::{
            browser_context_rule_categories, browser_context_rule_keywords, ClassificationPlugin,
            ClassificationResult, ClassificationTier, DeclaredMetadata,
        },
        taxonomy::is_valid_label,
        Taxonomy,
    };

    /// A table of its own for the tier tests, so what they assert about the
    /// tier cannot move when the shipped table is edited.
    const TEST_SEEDS: &[SiteSeed] = &[
        SiteSeed {
            host: "docs.google.com",
            scope: SiteScope::HostOnly,
            label: "document:docs",
            category: "FOCUS_WORK",
        },
        SiteSeed {
            host: "github.com",
            scope: SiteScope::WithSubdomains,
            label: "reference:github",
            category: "REFERENCE",
        },
        SiteSeed {
            host: "netflix.com",
            scope: SiteScope::WithSubdomains,
            label: "video:netflix",
            category: "PASSIVE_CONSUMPTION",
        },
        SiteSeed {
            host: "wikipedia.org",
            scope: SiteScope::WithSubdomains,
            label: "reference:wikipedia",
            category: "REFERENCE",
        },
        SiteSeed {
            host: "stackoverflow.com",
            scope: SiteScope::WithSubdomains,
            label: "reference:stack_overflow",
            category: "REFERENCE",
        },
        SiteSeed {
            host: "notion.so",
            scope: SiteScope::HostOnly,
            label: "document:notion",
            category: "FOCUS_WORK",
        },
    ];

    fn seed_plugin() -> SiteSeedPlugin {
        SiteSeedPlugin::with_seeds(TEST_SEEDS, "mvp-2".to_owned())
    }

    fn inference_plugin() -> SiteInferencePlugin {
        SiteInferencePlugin::new("mvp-2".to_owned())
    }

    /// What the engine hands a plugin for a browser tab: the site on its own,
    /// and the site followed by the title as the window context.
    fn classify_tab(
        plugin: &dyn ClassificationPlugin,
        app_name: &str,
        site: &str,
        title: &str,
    ) -> Option<ClassificationResult> {
        plugin.classify_declared(
            app_name,
            &format!("{site} {title}"),
            DeclaredMetadata {
                site: Some(site),
                ..DeclaredMetadata::default()
            },
        )
    }

    #[test]
    fn site_identity_strips_one_leading_www_and_nothing_else() {
        assert_eq!(
            site_identity("www.github.com").as_deref(),
            Some("github.com")
        );
        assert_eq!(site_identity("github.com").as_deref(), Some("github.com"));
        assert_eq!(
            site_identity("www.www.example.com").as_deref(),
            Some("www.example.com")
        );
        assert_eq!(
            site_identity("docs.github.com").as_deref(),
            Some("docs.github.com")
        );
        assert_eq!(
            site_identity("wwwexample.com").as_deref(),
            Some("wwwexample.com")
        );
        assert_eq!(
            site_identity("WWW.GitHub.COM").as_deref(),
            Some("github.com")
        );
    }

    /// A machine is not a site: its name means a different machine on the
    /// next network, so a rule taught about it would mean nothing there.
    #[test]
    fn site_identity_refuses_addresses_and_machine_names() {
        for host in [
            "192.168.1.20",
            "127.0.0.1",
            "10.0.0.1",
            "www.10.0.0.1",
            "::1",
            "fe80::1",
            "localhost",
            "www.localhost",
            "app.localhost",
            "printer.local",
            "www.printer.local",
            "home.arpa",
            "router.home.arpa",
            "www.home.arpa",
            "gitlab.internal",
            "nas.lan",
            "printer.localdomain",
            "intranet",
            "www.com",
            "",
            ".example.com",
            "example.com.",
            "example..com",
            "exa_mple.com",
        ] {
            assert_eq!(site_identity(host), None, "{host:?}");
        }
        assert_eq!(site_identity(&format!("{}.com", "a".repeat(250))), None);
        // Only a private name's own suffix: the same words elsewhere in a
        // public host, or inside a label, are a site like any other.
        for host in [
            "internal.example.com",
            "lan.example.org",
            "home.arpa.example.net",
            "mylan.com",
            "example.plan",
        ] {
            assert_eq!(site_identity(host).as_deref(), Some(host), "{host:?}");
        }
    }

    /// What `site_identity` returns is what migration 0040's CHECK on
    /// `local_site_name.host` admits, so storing an identity can never fail.
    #[test]
    fn a_site_identity_is_always_a_storable_host() {
        for host in ["www.github.com", "en.wikipedia.org", "a-b.example.co.uk"] {
            let site = site_identity(host).expect("a real site has an identity");
            assert!((1..=253).contains(&site.len()));
            assert!(site.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || b".-".contains(&byte)));
        }
    }

    #[test]
    fn the_matcher_prefers_the_longest_seeded_suffix() {
        const SEEDS: &[SiteSeed] = &[
            SiteSeed {
                host: "google.com",
                scope: SiteScope::WithSubdomains,
                label: "reference:read",
                category: "REFERENCE",
            },
            SiteSeed {
                host: "mail.google.com",
                scope: SiteScope::WithSubdomains,
                label: "communication:gmail",
                category: "COMMUNICATION",
            },
        ];
        let matcher = SiteMatcher::new(SEEDS);

        assert_eq!(
            matcher.lookup("mail.google.com").map(|seed| seed.category),
            Some("COMMUNICATION")
        );
        assert_eq!(
            matcher
                .lookup("u1.mail.google.com")
                .map(|seed| seed.category),
            Some("COMMUNICATION")
        );
        assert_eq!(
            matcher.lookup("news.google.com").map(|seed| seed.category),
            Some("REFERENCE")
        );
        assert_eq!(matcher.lookup("google.co.uk"), None);
        assert_eq!(matcher.lookup("notgoogle.com"), None);
    }

    /// `HostOnly` is the host and nothing under it; `WithSubdomains` is both.
    /// A `HostOnly` suffix that does not reach the site does not end the walk
    /// either: a broader seed further out still answers.
    #[test]
    fn host_only_seeds_reach_the_host_alone() {
        const SEEDS: &[SiteSeed] = &[
            SiteSeed {
                host: "docs.rs",
                scope: SiteScope::HostOnly,
                label: "reference:read",
                category: "REFERENCE",
            },
            SiteSeed {
                host: "wikipedia.org",
                scope: SiteScope::WithSubdomains,
                label: "reference:wikipedia",
                category: "REFERENCE",
            },
            SiteSeed {
                host: "en.wikipedia.org",
                scope: SiteScope::HostOnly,
                label: "reference:read",
                category: "REFERENCE",
            },
        ];
        let matcher = SiteMatcher::new(SEEDS);

        assert_eq!(
            matcher.lookup("docs.rs").map(|seed| seed.host),
            Some("docs.rs")
        );
        assert_eq!(matcher.lookup("crates.docs.rs"), None);
        assert_eq!(
            matcher.lookup("wikipedia.org").map(|seed| seed.host),
            Some("wikipedia.org")
        );
        assert_eq!(
            matcher.lookup("de.wikipedia.org").map(|seed| seed.host),
            Some("wikipedia.org")
        );
        assert_eq!(
            matcher.lookup("en.wikipedia.org").map(|seed| seed.host),
            Some("en.wikipedia.org")
        );
        assert_eq!(
            matcher.lookup("m.en.wikipedia.org").map(|seed| seed.host),
            Some("wikipedia.org")
        );
    }

    /// The contract of the shipped table, whatever it holds. Every entry is a
    /// host exactly as `site_identity` would produce it -- lowercase, dotted,
    /// no `www.`, no empty label -- listed once, with a label the engine
    /// accepts and a category the taxonomy has that is not the default.
    #[test]
    fn every_site_seed_is_well_formed() {
        let taxonomy = Taxonomy::from_builtin().expect("the shipped taxonomy loads");
        let mut hosts = HashSet::new();

        assert!(!SITE_SEEDS.is_empty());
        for seed in SITE_SEEDS {
            let host = seed.host;
            assert!(
                host.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-')),
                "{host} is not lowercase [a-z0-9.-]"
            );
            assert!(host.contains('.'), "{host} has no dot");
            assert!(
                !host.starts_with('.') && !host.ends_with('.') && !host.contains(".."),
                "{host} has an empty label"
            );
            assert!(!host.starts_with("www."), "{host} starts with www.");
            assert_eq!(
                site_identity(host).as_deref(),
                Some(host),
                "{host} is not a normalized site"
            );
            assert!(hosts.insert(host), "{host} is listed twice");
            assert!(
                is_valid_label(seed.label) && seed.label != "unlogged",
                "{host}: {} is not a <type>:<behavior> label",
                seed.label
            );
            assert!(
                taxonomy.contains_category(seed.category),
                "{host}: {} is not a taxonomy category",
                seed.category
            );
            assert_ne!(
                seed.category,
                taxonomy.default_category(),
                "{host} seeds the default category"
            );
        }
    }

    /// The seed table and the browser-context rules must not contradict each
    /// other: a host a rule keyword also names gets the rule's category, so the
    /// rules and the table never classify one site two ways across builds or
    /// across windows that do and do not report a URL.
    #[test]
    fn every_site_seed_agrees_with_the_browser_rules_that_name_its_host() {
        for seed in SITE_SEEDS {
            for category in browser_context_rule_categories(&normalize_classifier_text(seed.host)) {
                assert_eq!(
                    category, seed.category,
                    "{} is seeded as {} and a browser-context rule names it as {category}",
                    seed.host, seed.category
                );
            }
        }
    }

    /// The other direction: every whole host a browser-context rule names is
    /// seeded, in the rule's category, unless it is listed here as left out on
    /// purpose. A browser tab whose site can be read is not the rules' to
    /// decide, so a host they name that no seed covers loses its answer; this
    /// makes that a decision someone wrote down rather than an accident.
    ///
    /// A keyword names a host when a word after its first is the last label
    /// of a seeded host or a common top-level domain (`github com`,
    /// `docs google com spreadsheets`); the host is its words up to and
    /// including the last such word.
    #[test]
    fn every_host_a_browser_rule_names_is_seeded_or_left_out_on_purpose() {
        // Keyword -> why its host has no seed.
        const LEFT_OUT: &[(&str, &str)] = &[
            (
                "atlassian net",
                "Jira and Confluence share each workspace's host",
            ),
            (
                "linkedin com feed",
                "the feed shares its host with messaging, jobs and Learning",
            ),
        ];
        let top_level_domains: HashSet<&str> = SITE_SEEDS
            .iter()
            .filter_map(|seed| seed.host.rsplit('.').next())
            .chain([
                "com", "org", "net", "io", "ai", "app", "dev", "co", "so", "me",
            ])
            .collect();
        let matcher = SiteMatcher::new(SITE_SEEDS);
        let mut named = HashSet::new();

        for (keyword, category) in browser_context_rule_keywords() {
            let words: Vec<&str> = keyword.split(' ').collect();
            let Some(last) = (1..words.len())
                .rev()
                .find(|&index| top_level_domains.contains(words[index]))
            else {
                continue;
            };
            let host = words[..=last].join(".");
            named.insert(keyword);
            let seed = matcher.lookup(&host);
            if LEFT_OUT.iter().any(|(left_out, _)| *left_out == keyword) {
                assert!(
                    seed.is_none(),
                    "{host} is seeded, so {keyword:?} is no longer left out"
                );
                continue;
            }
            let seed = seed.unwrap_or_else(|| {
                panic!(
                    "a browser-context rule names {host} ({keyword:?}) and no seed covers it; \
                     seed it, or list it in LEFT_OUT with the reason"
                )
            });
            assert_eq!(
                seed.category, category,
                "{host} is seeded as {} and the rule naming it ({keyword:?}) says {category}",
                seed.category
            );
        }
        for (keyword, _) in LEFT_OUT {
            assert!(
                named.contains(keyword),
                "{keyword:?} is listed as left out but no rule names it as a host"
            );
        }
    }

    #[test]
    fn a_seeded_site_is_an_exact_match_with_a_seeds_confidence() {
        for browser in ["Safari", "Google Chrome", "Arc", "Firefox"] {
            let result = classify_tab(&seed_plugin(), browser, "github.com", "Pull requests")
                .unwrap_or_else(|| panic!("{browser} on a seeded site classifies"));

            assert_eq!(result.label(), "reference:github");
            assert_eq!(result.category(), "REFERENCE");
            assert_eq!(result.tier(), ClassificationTier::ExactMatch);
            assert_eq!(result.status(), ClassificationStatus::Classified);
            assert_eq!(result.confidence(), ClassificationConfidence::High);
            assert_eq!(result.source(), ClassificationSource::Seed);
        }
    }

    /// The same site with and without `www.`, and under a `WithSubdomains`
    /// seed, is the same seed.
    #[test]
    fn a_seed_reaches_www_and_its_subdomains() {
        for site in ["www.github.com", "gist.github.com", "docs.github.com"] {
            let result = classify_tab(&seed_plugin(), "Safari", site, "")
                .unwrap_or_else(|| panic!("{site} is covered by the github.com seed"));

            assert_eq!(result.category(), "REFERENCE", "{site}");
        }
    }

    /// Title words decide nothing about a seeded site's category, or its
    /// label. The browser-context rules would read this tab as GitHub and
    /// YouTube at once and abstain; the host is GitHub, so the tab is
    /// REFERENCE.
    #[test]
    fn title_words_cannot_override_a_seeded_host() {
        for title in [
            "GitHub discussion about youtube.com/watch/private",
            "Watch later - YouTube",
            "Inbox (3) - Gmail",
        ] {
            let result = classify_tab(&seed_plugin(), "Google Chrome", "github.com", title)
                .expect("the seeded host classifies");

            assert_eq!(result.category(), "REFERENCE", "{title}");
            assert_eq!(result.label(), "reference:github", "{title}");
            assert_eq!(result.source(), ClassificationSource::Seed, "{title}");
        }

        // A seed the rules have no keyword for: the one rule that matches the
        // title alone is a confident verdict in another category, and it is
        // still ignored.
        let result = classify_tab(&seed_plugin(), "Safari", "netflix.com", "Inbox")
            .expect("the seeded host classifies");
        assert_eq!(result.category(), "PASSIVE_CONSUMPTION");
        assert_eq!(result.label(), "video:netflix");

        // A verdict in the seed's own category is not a label either: a title
        // naming another product, or a rule that matches the host itself,
        // leaves the seed's label as it is.
        for (site, title, label) in [
            (
                "en.wikipedia.org",
                "GitHub - Wikipedia",
                "reference:wikipedia",
            ),
            (
                "stackoverflow.com",
                "How to cache GitHub Actions - Stack Overflow",
                "reference:stack_overflow",
            ),
            ("notion.so", "", "document:notion"),
            ("notion.so", "Roadmap - Notion", "document:notion"),
        ] {
            let result = classify_tab(&seed_plugin(), "Safari", site, title)
                .expect("the seeded host classifies");
            assert_eq!(result.label(), label, "{site} {title}");
            assert_eq!(
                result.source(),
                ClassificationSource::Seed,
                "{site} {title}"
            );
        }
    }

    /// Google serves all three editors from `docs.google.com`, so the table
    /// can only say `document:docs`; the title names the product, and
    /// `SEED_LABEL_REFINEMENTS` lets a Sheets or Slides rule verdict keep its
    /// label there. No other seed's label is ever refined.
    #[test]
    fn the_rules_refine_the_label_within_the_seeds_category() {
        let cases = [
            ("Q3 budget - Google Sheets", "document:sheets"),
            ("Kickoff - Google Slides", "document:slides"),
            ("Quarterly plan - Google Docs", "document:docs"),
            ("", "document:docs"),
            // Docs and Drive in one title disagree, so the rules abstain and
            // the seed's own label stands.
            ("Quarterly plan - Google Drive", "document:docs"),
        ];

        for (title, expected_label) in cases {
            let result = classify_tab(&seed_plugin(), "Safari", "docs.google.com", title)
                .expect("the seeded host classifies");

            assert_eq!(result.label(), expected_label, "{title}");
            assert_eq!(result.category(), "FOCUS_WORK", "{title}");
            assert_eq!(
                result.confidence(),
                ClassificationConfidence::High,
                "{title}"
            );
            assert_eq!(result.source(), ClassificationSource::Seed, "{title}");
        }
    }

    #[test]
    fn an_unseeded_or_unaddressable_site_is_not_the_seed_tiers_to_answer() {
        for site in ["example.org", "github.io", "192.168.1.20", "localhost"] {
            assert!(
                classify_tab(&seed_plugin(), "Safari", site, "").is_none(),
                "{site}"
            );
        }
    }

    /// Neither site tier reads anything but a browser's tab. The same site
    /// reported by an application that is not a browser, a browser window with
    /// no site, and the metadata-free path all answer nothing, so every such
    /// window classifies exactly as it did before these tiers existed.
    #[test]
    fn the_site_tiers_answer_only_for_a_browser_window_with_a_site() {
        let seed = seed_plugin();
        let inference = inference_plugin();
        let plugins: [&dyn ClassificationPlugin; 2] = [&seed, &inference];

        for plugin in plugins {
            for app_name in ["Slack", "Obscure Editor", "Code"] {
                for site in ["github.com", "docs.qwzx.edu"] {
                    assert!(
                        classify_tab(plugin, app_name, site, "").is_none(),
                        "{app_name} / {site}"
                    );
                }
            }
            for app_name in ["Safari", "Google Chrome", "Slack"] {
                for title in ["github.com", "docs.qwzx.edu Syllabus", "private title"] {
                    assert!(
                        plugin
                            .classify_declared(app_name, title, DeclaredMetadata::default())
                            .is_none(),
                        "{app_name} / {title}"
                    );
                    assert!(
                        plugin.classify(app_name, title).is_none(),
                        "{app_name} / {title}"
                    );
                }
            }
        }
    }

    #[test]
    fn agreeing_signals_infer_a_category_at_medium_confidence() {
        let cases = [
            ("mail.qwzx.io", "COMMUNICATION", "communication:inferred"),
            ("docs.qwzx.io", "REFERENCE", "reference:inferred"),
            ("jira.qwzx.io", "TASK_MANAGEMENT", "task:inferred"),
            ("github.qwzx.com", "REFERENCE", "reference:inferred"),
            ("gitlab.qwzx.org", "REFERENCE", "reference:inferred"),
            ("cs.qwzx.edu", "REFERENCE", "reference:inferred"),
            ("library.qwzx.ac.uk", "REFERENCE", "reference:inferred"),
            ("qwzx.gov.au", "REFERENCE", "reference:inferred"),
            ("arch-wiki.org", "REFERENCE", "reference:inferred"),
            // Two signals, one vote each, the same category.
            ("help.docs.qwzx.io", "REFERENCE", "reference:inferred"),
            ("docs.qwzx.edu", "REFERENCE", "reference:inferred"),
        ];

        for (site, category, label) in cases {
            let result = classify_tab(&inference_plugin(), "Firefox", site, "")
                .unwrap_or_else(|| panic!("{site} should be inferred"));

            assert_eq!(result.category(), category, "{site}");
            assert_eq!(result.label(), label, "{site}");
            assert_eq!(
                result.tier(),
                ClassificationTier::LocalPurposeHeuristic,
                "{site}"
            );
            assert_eq!(result.status(), ClassificationStatus::Classified, "{site}");
            assert_eq!(
                result.confidence(),
                ClassificationConfidence::Medium,
                "{site}"
            );
            assert_eq!(result.source(), ClassificationSource::Heuristic, "{site}");
        }
    }

    /// Signals that disagree are no answer. The window falls through to the
    /// tiers below, and the site stays one Velvt can ask about.
    #[test]
    fn disagreeing_signals_infer_nothing() {
        for site in [
            "mail.qwzx.edu",
            "mail.docs.qwzx.io",
            "jira.wiki-qwzx.com",
            "support.mail-qwzx.com",
        ] {
            assert!(
                classify_tab(&inference_plugin(), "Safari", site, "").is_none(),
                "{site}"
            );
        }
    }

    /// A sign-in label in front is SYSTEM whatever else the host says: an
    /// institution's suffix alone would otherwise file its sign-in page as
    /// REFERENCE. Without such a label the suffix still counts, and a library
    /// proxy's rewritten publisher host is reading.
    #[test]
    fn a_sign_in_host_is_system() {
        for site in [
            "weblogin.qwzx.edu",
            "sso.qwzx.ac.uk",
            "login.docs.qwzx.io",
            "idp.qwzx.gov",
        ] {
            assert_eq!(inferred_site_category(site), Some("SYSTEM"), "{site}");
        }
        assert_eq!(inferred_site_category("stat.qwzx.edu"), Some("REFERENCE"));
        assert_eq!(
            inferred_site_category("www-nature-com.proxy.lib.qwzx.edu"),
            Some("REFERENCE")
        );
        // Only a whole label: a token of one, or the registrable label itself,
        // is not a door.
        assert_eq!(inferred_site_category("casper.qwzx.edu"), Some("REFERENCE"));
        assert_eq!(inferred_site_category("docs.proxy.io"), Some("REFERENCE"));
    }

    /// No signal is no answer too, and a label that only contains a signal
    /// word is not the word.
    #[test]
    fn a_site_with_no_signal_infers_nothing() {
        for site in [
            "qwzx.io",
            "bbc.co.uk",
            "mailchimp.com",
            "docsend.com",
            "email-marketing.qwzx.io",
        ] {
            assert_eq!(inferred_site_category(site), None, "{site}");
        }
    }

    /// The seed tier owns a seeded site, so the inference tier stays silent
    /// on one even where its signals would fire.
    #[test]
    fn a_seeded_site_is_never_inferred() {
        for seed in SITE_SEEDS {
            assert!(
                classify_tab(&inference_plugin(), "Safari", seed.host, "").is_none(),
                "{}",
                seed.host
            );
        }
    }

    /// Under a two-label public suffix the registrable label is the third
    /// from the right. Read as the second, `kb` in `kb.co.uk` would be a
    /// subdomain vote for REFERENCE, and `qwzx-wiki` a subdomain label that
    /// votes for nothing.
    #[test]
    fn the_registrable_domain_is_found_under_a_country_code_second_level() {
        assert_eq!(inferred_site_category("kb.qwzx.co.uk"), Some("REFERENCE"));
        assert_eq!(inferred_site_category("kb.co.uk"), None);
        assert_eq!(inferred_site_category("qwzx-wiki.co.uk"), Some("REFERENCE"));
        assert_eq!(inferred_site_category("qwzx-wiki.co"), Some("REFERENCE"));
        assert_eq!(inferred_site_category("qwzx.co.uk"), None);
    }
}
