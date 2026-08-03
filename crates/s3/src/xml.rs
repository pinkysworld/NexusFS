//! Minimal S3 XML response bodies.
//!
//! Hand-written rather than generated: the response set is small and fixed, and a
//! serialisation crate would add a dependency for four document shapes.

/// Escape the five XML predefined entities.
///
/// Object keys come from user input and can legitimately contain `&` or `<`, which
/// would otherwise produce a malformed document.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

const HEADER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
const NS: &str = r#"xmlns="http://s3.amazonaws.com/doc/2006-03-01/""#;

pub struct ObjectRow {
    pub key: String,
    pub size: u64,
    pub etag: String,
    pub last_modified: String,
}

pub fn list_buckets(buckets: &[(String, String)]) -> String {
    let rows: String = buckets
        .iter()
        .map(|(name, created)| {
            format!(
                "<Bucket><Name>{}</Name><CreationDate>{}</CreationDate></Bucket>",
                esc(name),
                esc(created)
            )
        })
        .collect();

    format!(
        "{HEADER}<ListAllMyBucketsResult {NS}>\
<Owner><ID>nexusfs</ID><DisplayName>nexusfs</DisplayName></Owner>\
<Buckets>{rows}</Buckets>\
</ListAllMyBucketsResult>"
    )
}

#[allow(clippy::too_many_arguments)]
pub fn list_objects_v2(
    bucket: &str,
    prefix: &str,
    delimiter: Option<&str>,
    max_keys: usize,
    truncated: bool,
    next_token: Option<&str>,
    objects: &[ObjectRow],
    common_prefixes: &[String],
) -> String {
    let contents: String = objects
        .iter()
        .map(|o| {
            format!(
                "<Contents><Key>{}</Key><LastModified>{}</LastModified>\
<ETag>&quot;{}&quot;</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                esc(&o.key),
                esc(&o.last_modified),
                esc(&o.etag),
                o.size
            )
        })
        .collect();

    let prefixes: String = common_prefixes
        .iter()
        .map(|p| {
            format!(
                "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
                esc(p)
            )
        })
        .collect();

    let delim = delimiter
        .map(|d| format!("<Delimiter>{}</Delimiter>", esc(d)))
        .unwrap_or_default();

    let next = next_token
        .map(|t| format!("<NextContinuationToken>{}</NextContinuationToken>", esc(t)))
        .unwrap_or_default();

    format!(
        "{HEADER}<ListBucketResult {NS}>\
<Name>{}</Name><Prefix>{}</Prefix>{delim}<KeyCount>{}</KeyCount>\
<MaxKeys>{max_keys}</MaxKeys><IsTruncated>{truncated}</IsTruncated>{next}\
{contents}{prefixes}</ListBucketResult>",
        esc(bucket),
        esc(prefix),
        objects.len() + common_prefixes.len()
    )
}

pub fn error(code: &str, message: &str, resource: &str) -> String {
    format!(
        "{HEADER}<Error><Code>{}</Code><Message>{}</Message><Resource>{}</Resource></Error>",
        esc(code),
        esc(message),
        esc(resource)
    )
}

/// Format epoch milliseconds as an ISO-8601 timestamp.
///
/// S3 clients parse `LastModified`, so it has to be well formed even though nothing
/// in NexusFS depends on it.
pub fn iso8601(ms: u64) -> String {
    use time::format_description::well_known::Iso8601;
    use time::OffsetDateTime;

    OffsetDateTime::from_unix_timestamp_nanos((ms as i128) * 1_000_000)
        .ok()
        .and_then(|t| t.format(&Iso8601::DEFAULT).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}
