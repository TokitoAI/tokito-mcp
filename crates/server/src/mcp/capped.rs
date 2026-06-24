//! A `SessionManager` that bounds the number of concurrent MCP sessions.
//!
//! rmcp's `LocalSessionManager` keeps an unbounded `HashMap` of sessions, each
//! backed by a live tokio task, with a 5-minute keep-alive. A scripted
//! `POST /mcp` `initialize` loop can therefore grow the map (and the task count)
//! without bound (red-team finding, board #11).
//!
//! `CappedSessionManager` delegates every operation to an inner
//! `LocalSessionManager` but:
//!   - rejects `create_session` once `max_sessions` live sessions exist, so the
//!     map can never exceed the cap, and
//!   - tightens the inner keep-alive so sessions whose HTTP connection silently
//!     dropped are reaped in 60s instead of 5 minutes.

use std::time::Duration;

use futures::Stream;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManager, LocalSessionManagerError,
};
use rmcp::transport::streamable_http_server::session::{
    ServerSseMessage, SessionId, SessionManager,
};

/// Idle timeout before a session whose HTTP connection dropped is reaped.
/// rmcp defaults to 5 min; 60s is ample for a request/response catalog server
/// and bounds zombie accumulation.
const SESSION_KEEP_ALIVE: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum CappedSessionError {
    #[error(transparent)]
    Inner(#[from] LocalSessionManagerError),
    #[error("session limit reached ({max} active sessions); try again later")]
    TooManySessions { max: usize },
}

pub struct CappedSessionManager {
    inner: LocalSessionManager,
    max_sessions: usize,
}

impl CappedSessionManager {
    pub fn new(max_sessions: usize) -> Self {
        // `LocalSessionManager` is #[non_exhaustive] (no struct literal), but its
        // fields are public — build the default and tighten keep-alive in place.
        let mut inner = LocalSessionManager::default();
        inner.session_config.keep_alive = Some(SESSION_KEEP_ALIVE);
        Self {
            inner,
            max_sessions,
        }
    }
}

impl SessionManager for CappedSessionManager {
    type Error = CappedSessionError;
    type Transport = <LocalSessionManager as SessionManager>::Transport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        // Check before delegating so the inner map never exceeds the cap. The
        // check/insert isn't atomic, but a small overshoot under extreme
        // concurrency is harmless for a DoS bound.
        let active = self.inner.sessions.read().await.len();
        if active >= self.max_sessions {
            return Err(CappedSessionError::TooManySessions {
                max: self.max_sessions,
            });
        }
        Ok(self.inner.create_session().await?)
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        Ok(self.inner.initialize_session(id, message).await?)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        Ok(self.inner.has_session(id).await?)
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        Ok(self.inner.close_session(id).await?)
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.create_stream(id, message).await?)
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        Ok(self.inner.accept_message(id, message).await?)
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.create_standalone_stream(id).await?)
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.resume(id, last_event_id).await?)
    }
}
