/// Reduces a raw focused-document URL to a validated local hostname.
///
/// The caller must discard the original URL immediately. Paths, queries,
/// fragments, credentials, and ports are deliberately excluded so stable
/// browser identity follows the site rather than an individual document.
pub(crate) fn focused_site_context(raw_url: Option<&str>) -> Option<String> {
    let value = raw_url?.trim();
    let (_, remainder) = value.split_once("://")?;
    let authority = remainder
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    let host = if let Some(ipv6) = authority.strip_prefix('[') {
        ipv6.split_once(']')?.0
    } else {
        authority.split(':').next().unwrap_or_default()
    }
    .trim_end_matches('.')
    .to_ascii_lowercase();
    (!host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':')))
    .then_some(host)
}

#[cfg(test)]
mod tests {
    use super::focused_site_context;

    #[test]
    fn keeps_only_normalized_hostname() {
        assert_eq!(
            focused_site_context(Some("https://User@example.COM:8443/private?q=secret#part")),
            Some("example.com".to_owned())
        );
    }

    #[test]
    fn rejects_non_hierarchical_and_invalid_urls() {
        assert_eq!(focused_site_context(Some("file:///private/document")), None);
        assert_eq!(focused_site_context(Some("not a url")), None);
        assert_eq!(focused_site_context(None), None);
    }
}
