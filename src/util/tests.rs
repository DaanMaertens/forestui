use super::*;
use chrono::Duration;

#[test]
fn slugify_matches_python() {
    assert_eq!(slugify("My Forest!"), "my-forest");
    assert_eq!(slugify("forest"), "forest");
    assert_eq!(slugify("--a--b--"), "a-b");
}

/// Every expectation here was read off `humanize.naturaldelta` itself
/// rather than reasoned about, because its boundaries are not where you
/// would put them: 16 days is already "a month", 150 seconds is 2 minutes
/// and not 3, and 364 days is "a year".
#[test]
fn naturaldelta_matches_humanize() {
    const DAY: u64 = 24 * 60 * 60;
    let cases: [(u64, &str); 27] = [
        (1, "a second"),
        (2, "2 seconds"),
        (59, "59 seconds"),
        (60, "a minute"),
        (89, "a minute"),
        (90, "2 minutes"),
        (150, "2 minutes"),
        (151, "3 minutes"),
        (1800, "30 minutes"),
        (3599, "an hour"),
        (3600, "an hour"),
        (5400, "2 hours"),
        (18000, "5 hours"),
        (86399, "a day"),
        (86400, "a day"),
        (108000, "a day"),
        (129600, "a day"),
        (2 * DAY, "2 days"),
        (5 * DAY, "5 days"),
        (15 * DAY, "15 days"),
        (16 * DAY, "a month"),
        (45 * DAY, "a month"),
        (46 * DAY, "2 months"),
        (200 * DAY, "7 months"),
        (364 * DAY, "a year"),
        (366 * DAY, "1 year, 1 day"),
        (1000 * DAY, "2 years"),
    ];
    for (secs, expected) in cases {
        assert_eq!(naturaldelta(secs), expected, "at {secs} seconds");
    }
}

#[test]
fn naturaltime_adds_tense() {
    let now = Utc::now();
    assert_eq!(naturaltime(now - Duration::seconds(30)), "30 seconds ago");
    assert_eq!(naturaltime(now - Duration::hours(30)), "a day ago");
    // A second of slack: the elapsed time between `now` and the call would
    // otherwise round 5h down.
    assert_eq!(
        naturaltime(now + Duration::hours(5) + Duration::seconds(1)),
        "5 hours from now"
    );
}

#[test]
fn atomic_write_replaces_and_leaves_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("config.json");
    std::fs::write(&target, "old").unwrap();

    write_atomically(&target, "new").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");

    // No staging file may survive the swap.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "config.json")
        .collect();
    assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
}

#[test]
fn atomic_write_follows_a_symlinked_target() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles");
    std::fs::create_dir(&real).unwrap();
    let data = real.join("config.json");
    std::fs::write(&data, "old").unwrap();

    let link = dir.path().join("config.json");
    std::os::unix::fs::symlink(&data, &link).unwrap();

    write_atomically(&link, "new").unwrap();

    // Renaming over the link would leave a regular file here and strand the
    // dotfiles copy on the old contents forever.
    assert!(link.is_symlink(), "the symlink was replaced by a real file");
    assert_eq!(std::fs::read_to_string(&data).unwrap(), "new");
}

#[test]
fn atomic_write_keeps_the_targets_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("settings.json");
    std::fs::write(&target, "old").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();

    write_atomically(&target, "new").unwrap();

    // A fresh temp file is born at the umask default; carrying it onto the
    // target would widen a config the user deliberately locked down.
    let mode = std::fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "permissions were widened to {mode:o}");
}

#[test]
fn truncate_appends_ellipsis() {
    assert_eq!(truncate("abcdef", 3), "abc...");
    assert_eq!(truncate("ab", 3), "ab");
}

#[test]
fn a_noun_agrees_with_its_count() {
    let issue = Noun::new("issue", "issues");
    assert_eq!(issue.of(0), "issues");
    assert_eq!(issue.of(1), "issue");
    assert_eq!(issue.of(2), "issues");
    assert_eq!(issue.count(0), "0 issues");
    assert_eq!(issue.count(1), "1 issue");
    assert_eq!(issue.count(2), "2 issues");
}
