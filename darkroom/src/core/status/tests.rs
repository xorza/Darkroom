use super::*;

#[test]
fn error_slot_tracks_the_last_failure_and_history_keeps_both() {
    let mut log = StatusLog::default();

    assert_eq!(log.error, None);

    // A failure lands in both the slot and the history; a later failure
    // replaces the slot.
    log.error("save failed: a".into());
    log.error("compile failed: b".into());
    assert_eq!(log.error.as_deref(), Some("compile failed: b"));
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["save failed: a", "compile failed: b"]
    );
}
