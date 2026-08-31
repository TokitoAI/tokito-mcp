//! TokitoAI/tokito-mcp#106 review (P1) — hostile `/v1/search` queries must
//! never surface as a 500. `search::normalize_query`'s rewrite runs ahead of
//! every FTS5 `MATCH`; this exercises the exact probe table from the review
//! at the HTTP boundary, where a raw SQL failure would otherwise turn into
//! `AppError::Symbols(_) -> 500` for a caller who did nothing wrong (or, for
//! the queries #105's normalization itself could mangle, wrong on FTS5's
//! terms alone).
//!
//! Two buckets:
//!   - genuinely malformed FTS5 syntax (`he"llo`, an unterminated quote) —
//!     these must 400, not 500.
//!   - everything else (including inputs #105's rewrite could have turned
//!     malformed, like `fp_filters:Connector*` or `AND_gate`) — these must
//!     come back 200, exactly like they did before #105 ever touched the
//!     query.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tokito_mcp_server::build_app;
use tower::ServiceExt;

mod common;

/// Percent-encodes `s` for safe use in a URI query-string value. No
/// URL-encoding crate is a dev-dependency of this crate, and the alphabet of
/// characters these hostile probes need encoded is small, so a minimal
/// byte-wise encoder is simplest — this also naturally handles the UTF-8
/// probe by encoding every non-ASCII byte.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn search(query: &str) -> (StatusCode, Value) {
    let app = build_app(common::fixture_app_state());
    let uri = format!("/v1/search?q={}&limit=10", percent_encode(query));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value =
        serde_json::from_slice(&body).unwrap_or_else(|_| panic!("non-JSON body: {body:?}"));
    (status, json)
}

/// Every probe in this list must never 500 — full stop. This is the
/// invariant the review is pinning; the more specific tests below additionally
/// pin the *exact* status each one should carry.
const HOSTILE_PROBES: &[&str] = &[
    "fp_filters:Connector*",
    "_",
    "__",
    "AND_gate",
    "OR_gate",
    "NOT_gate",
    "he\"llo",
    "unterminated \"quote",
    "(pin OR header)",
    "NEAR(pin header)",
    "コネクタ",
];

#[tokio::test]
async fn no_hostile_probe_ever_returns_500() {
    for query in HOSTILE_PROBES {
        let (status, body) = search(query).await;
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "query {query:?} must not 500, got {status} body={body:?}"
        );
    }
}

/// Genuinely malformed FTS5 syntax — an unbalanced quote can never parse, in
/// any query engine. These are client errors: 400, with the stable
/// `bad_request` error code REST callers already key off of.
#[tokio::test]
async fn malformed_quote_syntax_returns_400_not_500() {
    for query in ["he\"llo", "unterminated \"quote"] {
        let (status, body) = search(query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "query {query:?}");
        assert_eq!(body["error"]["code"], "bad_request");
    }
}

/// Queries that already carry FTS5 syntax markers — a column filter, parens
/// (including `NEAR(...)`), a standalone boolean operator hiding behind an
/// underscore — are passed straight through unmodified rather than rewritten
/// by `normalize_query`, so they keep exactly their pre-#105 (valid, 200)
/// behavior. This is the regression the review flagged: desugaring
/// `fp_filters:Connector*`'s underscore split the column name into a
/// bareword plus a bogus reference, and desugaring `AND_gate` resurrected
/// `AND` as a real operator with no operand — both used to 500.
#[tokio::test]
async fn syntax_bearing_queries_keep_pre_pr_200_behavior() {
    for query in [
        "fp_filters:Connector*",
        "AND_gate",
        "OR_gate",
        "NOT_gate",
        "(pin OR header)",
        "NEAR(pin header)",
    ] {
        let (status, body) = search(query).await;
        assert_eq!(status, StatusCode::OK, "query {query:?} body={body:?}");
    }
}

/// A lone underscore (or run of them) tokenizes to nothing under
/// `unicode61`, so FTS5 happily accepts it as a zero-token `MATCH` and
/// returns no rows — but naively desugaring it to a space and sending that
/// to FTS5 is a syntax error, since an empty `MATCH` argument doesn't parse
/// at all. `normalize_query` must fall back to the original text rather than
/// forwarding whitespace.
#[tokio::test]
async fn underscore_only_queries_return_200_empty() {
    for query in ["_", "__"] {
        let (status, body) = search(query).await;
        assert_eq!(status, StatusCode::OK, "query {query:?}");
        assert_eq!(body["items"].as_array().unwrap().len(), 0);
    }
}

/// Non-ASCII input must not be flagged as FTS5 syntax and must survive
/// normalization without erroring.
#[tokio::test]
async fn unicode_query_returns_200() {
    let (status, _) = search("コネクタ").await;
    assert_eq!(status, StatusCode::OK);
}

// --- TokitoAI/tokito-mcp#106 review, round 2: a row-decode failure must
// stay a generic 500, never a 400 InvalidQuery ---
//
// `run_match_query`'s error mapping is scoped to genuine FTS5/SQLite
// query-syntax rejections (see `is_query_syntax_error`'s doc comment); a
// row that fails to decode — a corrupt catalog, not a bad query — must get
// the same treatment any other internal fault does: 500, a generic
// client-facing message (not the raw rusqlite detail), and a server-side
// log. The `tracing::error!` call lives on the exact code path that
// produces that generic message (`error.rs`'s `INTERNAL_SERVER_ERROR`
// branch), so pinning the HTTP-visible contract below — 500, `symbols`
// code, generic message — is equivalent to pinning that the log fires: no
// other branch in `error.rs` can produce this combination.

async fn search_against(
    app_state: tokito_mcp_server::state::AppState,
    query: &str,
) -> (StatusCode, Value) {
    let app = build_app(app_state);
    let uri = format!("/v1/search?q={}&limit=10", percent_encode(query));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value =
        serde_json::from_slice(&body).unwrap_or_else(|_| panic!("non-JSON body: {body:?}"));
    (status, json)
}

#[tokio::test]
async fn row_decode_failure_on_a_matched_row_stays_500_with_a_generic_message() {
    let (status, body) = search_against(
        common::fixture_app_state_with_corrupt_description(),
        "resistor",
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body={body:?}");
    assert_eq!(body["error"]["code"], "symbols");
    assert_eq!(
        body["error"]["message"], "internal server error",
        "must not leak the raw rusqlite InvalidColumnType detail"
    );
}

/// Same failure, reached through `find_compatible`'s query-present branch —
/// pinned separately since that branch has its own, independently narrowed
/// error mapping (`query_error`, not `run_match_query`).
#[tokio::test]
async fn row_decode_failure_via_find_compatible_also_stays_500() {
    let app = build_app(common::fixture_app_state_with_corrupt_description());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/compatible?query=resistor&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value =
        serde_json::from_slice(&body).unwrap_or_else(|_| panic!("non-JSON body: {body:?}"));
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body={json:?}");
    assert_eq!(json["error"]["message"], "internal server error");
}
