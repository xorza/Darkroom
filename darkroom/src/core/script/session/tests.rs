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
    assert_eq!(store.sessions.len(), 2);
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
    assert_eq!(store.sessions.len(), 0);

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
    assert_eq!(store.sessions.len(), 1);
}

#[test]
fn overflow_reaps_then_errors_when_still_full() {
    let (mut store, t0) = fixture();
    let _a = store.get_or_create(None, "A", t0).unwrap().id;
    let _b = store.get_or_create(None, "B", t0).unwrap().id;
    assert_eq!(store.sessions.len(), 2);

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
    assert_eq!(store.sessions.len(), 1);
    assert_eq!(store.get_or_create(Some(c), "C", t1).unwrap().id, c);
}

#[test]
fn drop_session_removes_scope() {
    let (mut store, t0) = fixture();
    let id = store.get_or_create(None, "A", t0).unwrap().id;
    assert!(store.sessions.remove(&id).is_some());
    assert_eq!(store.sessions.len(), 0);
    assert_eq!(
        store.get_or_create(Some(id), "A", t0).unwrap_err(),
        SessionError::Unknown(id)
    );
    assert!(store.sessions.remove(&id).is_none());
}
