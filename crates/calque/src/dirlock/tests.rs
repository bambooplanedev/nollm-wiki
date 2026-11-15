//! Tests for `dirlock`.
use super::*;

#[test]
fn shared_waits_for_exclusive() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().to_path_buf();
    std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let d2 = d.clone();
    let a = std::thread::spawn(move || {
        exclusive(&d2, || {
            tx.send(Instant::now()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
    });
    let start = rx.recv().unwrap();
    let at = shared(&d, Instant::now);
    a.join().unwrap();
    assert!(at.duration_since(start) >= Duration::from_millis(250));
}

#[cfg(unix)]
#[test]
fn shared_runs_without_lock_file_when_readonly() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let serve = t.path().join(SERVE_DIR);
    std::fs::create_dir(&serve).unwrap();
    std::fs::set_permissions(&serve, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root writes anyway: nothing to test.
    if std::fs::write(serve.join("probe"), "").is_err() {
        assert_eq!(shared(t.path(), || 7), 7);
        assert!(!serve.join("compile.lock").exists());
    }
    std::fs::set_permissions(&serve, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_missing_dir_is_not_created() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("typo");
    assert_eq!(exclusive(&d, || 1), 1);
    assert_eq!(shared(&d, || 2), 2);
    assert!(!d.exists());
}

#[test]
fn exclusive_without_serve_dir_creates_nothing() {
    let t = tempfile::tempdir().unwrap();
    assert_eq!(exclusive(t.path(), || 3), 3);
    assert_eq!(shared(t.path(), || 4), 4);
    assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 0);
}

#[test]
fn try_writer_without_a_lock_file_is_an_unlocked_writer() {
    let t = tempfile::tempdir().unwrap();
    // A directory in its place: opening it for writing fails, even as root.
    std::fs::create_dir_all(t.path().join(SERVE_DIR).join("writer.lock")).unwrap();
    assert!(matches!(try_writer(t.path(), true), Some(Writer::Unlocked)));
    assert!(try_writer(t.path(), false).is_none());
}

#[test]
fn try_shared_does_not_wait_for_exclusive() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().to_path_buf();
    std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let d2 = d.clone();
    let a = std::thread::spawn(move || {
        exclusive(&d2, || {
            tx.send(()).unwrap();
            done_rx.recv().unwrap();
        });
    });
    rx.recv().unwrap();
    assert_eq!(try_shared(&d, || 1), None);
    done_tx.send(()).unwrap();
    a.join().unwrap();
    assert_eq!(try_shared(&d, || 2), Some(2));
}

#[test]
fn shared_until_gives_up_at_the_deadline() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
    let (release, a) = hold(d, Duration::from_secs(10), || ());
    let t = Instant::now();
    assert_eq!(shared_until(d, t + Duration::from_millis(200), || ()), None);
    assert!(
        t.elapsed() >= Duration::from_millis(200) && t.elapsed() < Duration::from_secs(1),
        "{:?}",
        t.elapsed()
    );
    release.send(()).unwrap();
    a.join().unwrap();
    assert_eq!(shared_until(d, Instant::now(), || 7), Some(7));
}

#[test]
fn shared_until_runs_once_the_lock_frees() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    std::fs::create_dir(d.join(SERVE_DIR)).unwrap();
    let (_release, a) = hold(d, Duration::from_millis(100), || ());
    let t = Instant::now();
    assert_eq!(shared_until(d, t + Duration::from_secs(2), || 1), Some(1));
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    a.join().unwrap();
}

#[test]
fn try_writer_is_exclusive() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join(SERVE_DIR)).unwrap();
    let a = try_writer(t.path(), true);
    assert!(matches!(a, Some(Writer::Locked(_))));
    assert!(try_writer(t.path(), true).is_none());
    drop(a);
    assert!(matches!(
        try_writer(t.path(), false),
        Some(Writer::Locked(_))
    ));
}
