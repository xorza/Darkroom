//! Per-client Rhai scope storage for the scripting executor.
//!
//! A [`SessionStore`] owns a collection of [`Session`]s keyed by
//! [`Uuid`]. Clients send a session id with every script request: a
//! nil id asks for a fresh session, any other id resumes the matching
//! one (errors if unknown or reaped). The store lives on the single
//! executor task — no locking, no contention.
//!
//! Stale sessions are reaped on access. `get_or_create` bumps
//! `last_activity` on every hit; `reap(now)` drops everything idle
//! past the configured timeout. `now: Instant` is a parameter on
//! every mutating method so tests drive the clock without sleeping.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rhai::Scope;
use uuid::Uuid;

/// Drop a session after this much inactivity. Hit whenever a request
/// comes in; a disconnected client has this long to reconnect and
/// resume before their Rhai scope is reclaimed.
pub const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// Hard cap on live sessions. On overflow the store reaps expired
/// entries first; if full after reaping, new session creation errors
/// out (clients retry later).
pub const MAX_SESSIONS: usize = 32;

/// Reasons [`SessionStore::get_or_create`] can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// Client supplied a session id that isn't known (never existed or
    /// was reaped for inactivity).
    Unknown(Uuid),
    /// Store's `max_sessions` cap reached even after reaping expired
    /// entries. `max` carries the cap the store was configured with so
    /// the rendered message reflects reality (not just [`MAX_SESSIONS`]).
    Full { max: usize },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "unknown session: {id}"),
            Self::Full { max } => write!(
                f,
                "session store full (max {max}); retry after an existing session expires"
            ),
        }
    }
}

/// Per-client Rhai state. One session = one [`Scope`] that persists
/// across script requests sharing the same session id.
#[derive(Debug)]
struct Session {
    scope: Scope<'static>,
    last_activity: Instant,
    origin: String,
}

/// Borrowed handle the executor runs a script against. `id` is the
/// session that now owns `scope`; returned from
/// [`SessionStore::get_or_create`] so the caller can echo the id back
/// in the reply.
#[derive(Debug)]
pub struct SessionRef<'a> {
    pub id: Uuid,
    pub scope: &'a mut Scope<'static>,
}

/// Holds every live session. Single-threaded — owned by the executor
/// task, so no Mutex. `now: Instant` is injected into every mutating
/// method so tests can drive the clock without sleeping.
#[derive(Debug)]
pub struct SessionStore {
    sessions: HashMap<Uuid, Session>,
    idle_timeout: Duration,
    max_sessions: usize,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::with_limits(SESSION_IDLE_TIMEOUT, MAX_SESSIONS)
    }
}

impl SessionStore {
    /// Testing-facing constructor; lets tests supply small/short limits
    /// without waiting on a real clock. Production code should use
    /// [`SessionStore::default`], which picks up module-level consts.
    pub fn with_limits(idle_timeout: Duration, max_sessions: usize) -> Self {
        Self {
            sessions: HashMap::new(),
            idle_timeout,
            max_sessions,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Look up or create a session.
    /// - `requested == None` → create a new session; returns its fresh id.
    /// - `requested == Some(id)` → resume; errors if the id isn't known.
    ///
    /// Bumps `last_activity` on every hit. Callers should invoke
    /// [`Self::reap`] first if they want capacity to reflect expired
    /// sessions.
    pub fn get_or_create(
        &mut self,
        requested: Option<Uuid>,
        origin: &str,
        now: Instant,
    ) -> Result<SessionRef<'_>, SessionError> {
        if let Some(id) = requested {
            let session = self
                .sessions
                .get_mut(&id)
                .ok_or(SessionError::Unknown(id))?;
            session.last_activity = now;
            return Ok(SessionRef {
                id,
                scope: &mut session.scope,
            });
        }

        if self.sessions.len() >= self.max_sessions {
            // Reap expired first — maybe the cap is already past.
            self.reap(now);
            if self.sessions.len() >= self.max_sessions {
                return Err(SessionError::Full {
                    max: self.max_sessions,
                });
            }
        }

        let id = Uuid::new_v4();
        let session = Session {
            scope: Scope::new(),
            last_activity: now,
            origin: origin.to_string(),
        };
        self.sessions.insert(id, session);
        let scope = &mut self.sessions.get_mut(&id).unwrap().scope;
        Ok(SessionRef { id, scope })
    }

    /// Drop sessions idle longer than `idle_timeout`. Returns the number
    /// reaped. No-op if `now` is earlier than any session's
    /// `last_activity` (won't happen in production — `Instant` is monotonic).
    pub fn reap(&mut self, now: Instant) -> usize {
        let timeout = self.idle_timeout;
        let before = self.sessions.len();
        self.sessions.retain(|id, s| {
            let idle = now.saturating_duration_since(s.last_activity);
            let keep = idle < timeout;
            if !keep {
                tracing::info!(
                    session = %id,
                    origin = %s.origin,
                    idle_secs = idle.as_secs(),
                    "reaping idle script session"
                );
            }
            keep
        });
        before - self.sessions.len()
    }

    #[cfg(test)]
    pub(crate) fn drop_session(&mut self, id: Uuid) -> bool {
        self.sessions.remove(&id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (SessionStore, Instant) {
        (
            SessionStore::with_limits(Duration::from_secs(10), 2),
            Instant::now(),
        )
    }

    #[test]
    fn create_returns_fresh_id_each_time() {
        let (mut store, t0) = fixture();
        let a = store.get_or_create(None, "A", t0).unwrap().id;
        let b = store.get_or_create(None, "B", t0).unwrap().id;
        assert_ne!(a, b);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn resume_returns_same_scope() {
        let (mut store, t0) = fixture();
        let id = store.get_or_create(None, "A", t0).unwrap().id;

        {
            let r = store.get_or_create(Some(id), "A", t0).unwrap();
            r.scope.push("x", 42_i64);
        }

        let r = store.get_or_create(Some(id), "A", t0).unwrap();
        let got: i64 = r.scope.get_value("x").expect("x is set");
        assert_eq!(got, 42);
    }

    #[test]
    fn resume_unknown_errors_with_the_requested_id() {
        let (mut store, t0) = fixture();
        let ghost = Uuid::new_v4();
        let err = store.get_or_create(Some(ghost), "A", t0).unwrap_err();
        assert_eq!(err, SessionError::Unknown(ghost));
    }

    #[test]
    fn cross_session_isolation() {
        let (mut store, t0) = fixture();
        let a = store.get_or_create(None, "A", t0).unwrap().id;
        let b = store.get_or_create(None, "B", t0).unwrap().id;

        store
            .get_or_create(Some(a), "A", t0)
            .unwrap()
            .scope
            .push("secret", 99_i64);

        let r = store.get_or_create(Some(b), "B", t0).unwrap();
        assert!(
            r.scope.get_value::<i64>("secret").is_none(),
            "B should not see A's binding"
        );
    }

    #[test]
    fn reap_drops_idle_sessions() {
        let (mut store, t0) = fixture();
        let a = store.get_or_create(None, "A", t0).unwrap().id;

        let t1 = t0 + Duration::from_secs(11);
        let reaped = store.reap(t1);
        assert_eq!(reaped, 1);
        assert_eq!(store.len(), 0);

        let err = store.get_or_create(Some(a), "A", t1).unwrap_err();
        assert_eq!(err, SessionError::Unknown(a));
    }

    #[test]
    fn activity_defers_reaping() {
        let (mut store, t0) = fixture();
        let id = store.get_or_create(None, "A", t0).unwrap().id;

        let t_mid = t0 + Duration::from_secs(6);
        let _ = store.get_or_create(Some(id), "A", t_mid).unwrap();

        let t_late = t_mid + Duration::from_secs(9);
        assert_eq!(store.reap(t_late), 0);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn overflow_reaps_then_errors_when_still_full() {
        let (mut store, t0) = fixture();
        let _a = store.get_or_create(None, "A", t0).unwrap().id;
        let _b = store.get_or_create(None, "B", t0).unwrap().id;
        assert_eq!(store.len(), 2);

        let err = store.get_or_create(None, "C", t0).unwrap_err();
        assert_eq!(err, SessionError::Full { max: 2 });
        // Rendered message reports the store's actual cap, not the
        // module-wide MAX_SESSIONS const.
        assert!(
            err.to_string().contains("max 2"),
            "expected message to mention cap=2, got: {err}"
        );
    }

    #[test]
    fn overflow_succeeds_after_expired_sessions_get_reaped() {
        let (mut store, t0) = fixture();
        let _a = store.get_or_create(None, "A", t0).unwrap().id;
        let _b = store.get_or_create(None, "B", t0).unwrap().id;

        let t1 = t0 + Duration::from_secs(15);
        let c = store.get_or_create(None, "C", t1).unwrap().id;
        assert_eq!(store.len(), 1);
        assert_eq!(store.get_or_create(Some(c), "C", t1).unwrap().id, c);
    }

    #[test]
    fn drop_session_removes_scope() {
        let (mut store, t0) = fixture();
        let id = store.get_or_create(None, "A", t0).unwrap().id;
        assert!(store.drop_session(id));
        assert_eq!(store.len(), 0);
        assert_eq!(
            store.get_or_create(Some(id), "A", t0).unwrap_err(),
            SessionError::Unknown(id)
        );
        assert!(!store.drop_session(id));
    }
}
