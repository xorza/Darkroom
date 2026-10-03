use super::*;

#[test]
fn error_slot_tracks_the_last_failure_and_history_keeps_both() {
    let mut log = StatusLog::default();

    assert_eq!(log.current(), None);

    // A failure lands in both the slot and the history; a later failure
    // replaces the slot.
    log.error(StatusFamily::Document, "save failed: a".into());
    log.error(StatusFamily::Run, "compile failed: b".into());
    assert_eq!(log.current(), Some("compile failed: b"));
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["save failed: a", "compile failed: b"]
    );
}

/// A success clears its own family's failure and nothing else: a finished
/// run leaves a failed save on the bar, and a save clears it.
#[test]
fn a_success_clears_only_its_own_familys_failure() {
    let mut log = StatusLog::default();
    log.error(StatusFamily::Document, "save failed: disk full".into());

    for other in [StatusFamily::Run, StatusFamily::Preferences] {
        log.succeeded(other);
        assert_eq!(
            log.current(),
            Some("save failed: disk full"),
            "{other:?} success"
        );
    }
    log.succeeded(StatusFamily::Document);
    assert_eq!(log.current(), None);

    // A success with nothing on the bar is a no-op.
    log.succeeded(StatusFamily::Run);
    assert_eq!(log.current(), None);
}
