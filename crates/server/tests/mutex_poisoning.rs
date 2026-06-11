//! Regression test for the red-team CRIT-4 finding: every handler used to
//! `conn.lock().unwrap()`, so any panic inside the SQL critical section
//! poisoned the mutex and bricked every subsequent request until the
//! process restarted. After the fix (`.unwrap_or_else(|p| p.into_inner())`
//! at all 9 lock sites), a poisoned mutex must NOT make health/manifest
//! endpoints return 500.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tokito_mcp_server::build_app;
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn poisoned_mutex_does_not_break_subsequent_requests() {
    let state = common::fixture_app_state();

    // Poison the conn mutex by holding the lock in a thread that panics.
    // The AppState's Arc<Mutex<Connection>> is shared with the handlers;
    // a panic inside the lock poisons it for everyone.
    let poisoner = {
        let conn = state.conn.clone();
        std::thread::spawn(move || {
            let _guard = conn.lock().unwrap();
            panic!("deliberate poisoning");
        })
    };
    let _ = poisoner.join();
    assert!(
        state.conn.is_poisoned(),
        "test setup failed to poison the mutex"
    );

    // After the fix, a handler that locks the mutex should succeed via
    // PoisonError::into_inner, not panic the request.
    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/libraries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "with the fix, poisoned mutex still serves /v1/libraries; \
         response was: {:?}",
        resp.into_body().collect().await.unwrap().to_bytes()
    );
}
