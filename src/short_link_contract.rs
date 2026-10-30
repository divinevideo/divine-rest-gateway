// ABOUTME: Pure request and response contract for public short-link creation.

use bech32::{primitives::decode::CheckedHrpstring, Bech32};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreationRequest {
    url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkMapping {
    pub code: String,
    pub short_url: String,
    pub url: String,
}

pub fn parse_creation_body(body: &[u8]) -> Result<String, ()> {
    if body.len() > 4096 {
        return Err(());
    }
    let request: CreationRequest = serde_json::from_slice(body).map_err(|_| ())?;
    if valid_target(&request.url) {
        Ok(request.url)
    } else {
        Err(())
    }
}

fn valid_target(raw: &str) -> bool {
    if raw.len() > 2048 {
        return false;
    }
    let Some(path) = raw.strip_prefix("https://divine.video/") else {
        return false;
    };
    let Some((kind, id)) = path.split_once('/') else {
        return false;
    };
    if id.is_empty()
        || id.len() > 1024
        || id == "."
        || id == ".."
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    {
        return false;
    }
    match kind {
        "video" => true,
        "profile" => {
            (id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                || CheckedHrpstring::new::<Bech32>(id).is_ok_and(|value| {
                    value.hrp().as_str() == "npub" && value.byte_iter().count() == 32
                })
        }
        _ => false,
    }
}

pub fn validate_mapping(body: &[u8], target: &str) -> Result<LinkMapping, ()> {
    if body.len() > 4096 {
        return Err(());
    }
    let mapping: LinkMapping = serde_json::from_slice(body).map_err(|_| ())?;
    if mapping.url != target
        || mapping.code.len() != 8
        || !mapping.code.bytes().all(|b| b.is_ascii_alphanumeric())
        || mapping.short_url != format!("https://divine.video/l/{}", mapping.code)
    {
        return Err(());
    }
    Ok(mapping)
}

pub fn origin_allowed(origin: Option<&str>) -> bool {
    matches!(
        origin,
        None | Some("https://divine.video" | "https://www.divine.video")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_canonical_video_and_profile_targets_are_accepted() {
        let key = "01".repeat(32);
        for target in [
            "https://divine.video/video/Case_Sensitive-ID".to_string(),
            format!("https://divine.video/profile/{key}"),
        ] {
            let body = serde_json::json!({"url": target}).to_string();
            assert_eq!(parse_creation_body(body.as_bytes()), Ok(target));
        }
        for target in [
            "https://evil.example/video/a",
            "https://www.divine.video/video/a",
            "https://divine.video/video/../a",
            "https://divine.video/video/a?q=1",
            "https://divine.video/video/%61",
            "https://divine.video/l/Abc12345",
            "https://divine.video/profile/short",
            "https://divine.video/video/a/b",
            "https://divine.video/video/a#b",
        ] {
            assert!(
                parse_creation_body(serde_json::json!({"url": target}).to_string().as_bytes())
                    .is_err(),
                "{target}"
            );
        }
        for body in [
            r#"{"url":"https://divine.video/video/a","extra":true}"#,
            r#"{"url":1}"#,
            r#"{"url":"https://divine.video/video/a","url":"https://divine.video/video/b"}"#,
        ] {
            assert!(parse_creation_body(body.as_bytes()).is_err());
        }
    }

    #[test]
    fn upstream_mapping_must_name_the_exact_requested_target_and_alias() {
        let target = "https://divine.video/video/CaseSensitive";
        let mut mapping = serde_json::json!({"code":"Abc12345","short_url":"https://divine.video/l/Abc12345","url":target});
        assert!(validate_mapping(mapping.to_string().as_bytes(), target).is_ok());
        for (field, value) in [
            ("code", "short"),
            ("short_url", "https://evil.example/l/Abc12345"),
            ("url", "https://divine.video/video/casesensitive"),
        ] {
            let original = mapping[field].clone();
            mapping[field] = value.into();
            assert!(validate_mapping(mapping.to_string().as_bytes(), target).is_err());
            mapping[field] = original;
        }
    }

    #[test]
    fn browser_origins_are_explicit_and_native_can_omit_origin() {
        for origin in [
            None,
            Some("https://divine.video"),
            Some("https://www.divine.video"),
        ] {
            assert!(origin_allowed(origin));
        }
        for origin in [
            "null",
            "http://divine.video",
            "https://evil.example",
            "https://alice.divine.video",
            "https://divine.video.evil.example",
        ] {
            assert!(!origin_allowed(Some(origin)));
        }
    }
}
