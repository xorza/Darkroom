use super::*;
use crate::core::status::internals::STATUS_LOG_CAP;

#[test]
fn error_slot_tracks_last_failure_and_history_keeps_both() {
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

    // Clearing the slot (a subsequent success) leaves the history intact.
    log.error = None;
    assert_eq!(log.error, None);
    assert_eq!(log.lines().count(), 2);
}

#[test]
fn history_is_capped_dropping_oldest() {
    let mut log = StatusLog::default();
    // One over the cap: line "0" is evicted, "1"..=CAP remain.
    for i in 0..=STATUS_LOG_CAP {
        log.error(format!("{i}"));
    }
    assert_eq!(log.lines().count(), STATUS_LOG_CAP);
    assert_eq!(log.lines().next(), Some("1"));
    assert_eq!(
        log.lines().last(),
        Some(STATUS_LOG_CAP.to_string().as_str())
    );
}
