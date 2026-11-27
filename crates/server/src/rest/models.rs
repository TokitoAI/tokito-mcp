//! Exact model retrieval from the official pinned KiCad library.
use axum::{
    extract::Path,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use sha2::{Digest, Sha256};
use std::{sync::OnceLock, time::Duration};
use tokito_symbols::footprints::FootprintId;

const REVISION: &str = "2a697f255a3654e5175e2e9b5d2abdb4ca874015";
const MAX_BYTES: usize = 8 * 1024 * 1024;
static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
static REQUESTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

fn upstream(library: &str, name: &str) -> Result<reqwest::Url, StatusCode> {
    let stem = name
        .strip_suffix(".step")
        .or_else(|| name.strip_suffix(".wrl"))
        .ok_or(StatusCode::BAD_REQUEST)?;
    FootprintId::new(library, stem).ok_or(StatusCode::BAD_REQUEST)?;
    let mut url = reqwest::Url::parse(
        "https://gitlab.com/api/v4/projects/kicad%2Flibraries%2Fkicad-packages3D/repository/files/",
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    url.path_segments_mut()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .pop_if_empty()
        .push(&format!("{library}.3dshapes/{name}"))
        .push("raw");
    url.query_pairs_mut().append_pair("ref", REVISION);
    Ok(url)
}

pub(super) async fn get_model(
    Path((library, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let url = upstream(&library, &name)?;
    let _permit = REQUESTS
        .try_acquire()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(12))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let unavailable = StatusCode::BAD_GATEWAY;
    let mut response = client.get(url).send().await.map_err(|_| unavailable)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(StatusCode::NOT_FOUND);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(unavailable);
    }
    let upstream = response.headers();
    if upstream
        .get("x-gitlab-commit-id")
        .and_then(|value| value.to_str().ok())
        != Some(REVISION)
    {
        return Err(unavailable);
    }
    let expected = upstream
        .get("x-gitlab-content-sha256")
        .and_then(|value| value.to_str().ok())
        .ok_or(unavailable)?
        .to_owned();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable)? {
        if chunk.len() > MAX_BYTES - bytes.len() {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        bytes.extend_from_slice(&chunk);
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    if bytes.is_empty() || digest != expected {
        return Err(unavailable);
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        "x-tokito-source-revision",
        HeaderValue::from_static(REVISION),
    );
    headers.insert(
        "x-tokito-source-sha256",
        HeaderValue::from_str(&digest).map_err(|_| unavailable)?,
    );
    Ok((headers, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_lookup_uses_only_the_pinned_official_path() {
        let url = upstream("RF_Module", "ESP32-WROOM-32.step").unwrap();
        assert_eq!(url.host_str(), Some("gitlab.com"));
        assert!(url
            .path()
            .ends_with("/RF_Module.3dshapes%2FESP32-WROOM-32.step/raw"));
        assert_eq!(url.query(), Some(format!("ref={REVISION}").as_str()));
        assert!(upstream("RF_Module", "../bad.step").is_err());
        assert!(upstream("RF_Module", "ESP32-WROOM-32.exe").is_err());
    }
}
