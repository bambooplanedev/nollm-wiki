//! Tests for `live`.
use super::exe::Exe;
use super::*;
use notify::event::{AccessKind, AccessMode, Flag, ModifyKind};
use notify::{Event, EventKind};

#[test]
fn opening_a_walked_file_is_not_an_edit() {
    let dir = Path::new("/proj/.nollm-wiki");
    let file = PathBuf::from("/proj/src/a.rs");
    let set = WatchSet::new(
        Path::new("/proj"),
        [file.clone()],
        [PathBuf::from("/proj/src")],
    );
    let open =
        Event::new(EventKind::Access(AccessKind::Open(AccessMode::Any))).add_path(file.clone());
    let modify = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(file);
    let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
    assert!(!is_edit(&open, dir, Some(&set)));
    assert!(!is_edit(&open, dir, None));
    assert!(is_edit(&modify, dir, Some(&set)));
    assert!(is_edit(&rescan, dir, Some(&set)));
}

#[test]
fn own_token_reads_only_this_server_files() {
    let dir = Path::new("/out");
    let serve = dir.join(BARRIER_DIR);
    assert_eq!(
        own_token(dir, "barrier", "9.0", &serve.join("barrier-9.0-7")),
        Some(7)
    );
    assert_eq!(
        own_token(dir, "ack", "9.0", &serve.join("ack-9.0-7")),
        Some(7)
    );
    assert_eq!(
        own_token(dir, "ack", "9.0", &serve.join("barrier-9.0-7")),
        None
    );
    assert_eq!(
        own_token(dir, "barrier", "9.0", &serve.join("barrier-9.1-7")),
        None
    );
    assert_eq!(
        own_token(dir, "ack", "9.0", &serve.join("tmp-ack-9.0-7")),
        None
    );
    assert_eq!(
        own_token(dir, "barrier", "9.0", &dir.join("barrier-9.0-7")),
        None
    );
}

#[test]
fn a_transcript_beside_a_missing_memory_folder_does_not_recompile() {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    let slot = tempfile::tempdir().unwrap();
    let slot = slot.path().canonicalize().unwrap();
    let opts = CompileOptions {
        incremental: true,
        memory: vec![slot.join("memory")],
        ..CompileOptions::default()
    };
    let live = Live::start(src.path(), out.path(), opts).unwrap();
    let _ = live.fresh();
    let gens = |l: &Live| {
        let s = l.lock();
        (s.event_gen, s.compiled_gen)
    };
    let before = gens(&live);

    std::fs::write(slot.join("abc.jsonl"), "{}\n").unwrap();
    std::thread::sleep(watch::DEBOUNCE * 4);
    assert_eq!(gens(&live), before);

    std::fs::create_dir(slot.join("memory")).unwrap();
    std::fs::write(slot.join("memory/n.md"), "# N\n\nnote\n").unwrap();
    let _ = live.fresh();
    assert!(gens(&live).1 > before.1);
}

/// `<dir>/wiki` with `body`, and an `Exe` tracking it.
fn tracked(dir: &Path, body: &str) -> (PathBuf, Exe) {
    let bin = dir.join("wiki");
    std::fs::write(&bin, body).unwrap();
    let exe = Exe::of([bin.clone()]);
    (bin, exe)
}

#[test]
fn an_untouched_binary_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let (_bin, exe) = tracked(dir.path(), "one");
    assert!(!exe.replaced());
    assert!(!exe.replaced());
}

#[test]
fn a_rename_over_the_binary_stays_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, exe) = tracked(dir.path(), "one");
    let saved = dir.path().join("saved");
    std::fs::copy(&bin, &saved).unwrap();
    let new = dir.path().join("new");
    std::fs::write(&new, "one").unwrap();
    std::fs::rename(&new, &bin).unwrap();
    assert!(exe.replaced());
    std::fs::rename(&saved, &bin).unwrap();
    assert!(exe.replaced());
}

#[test]
fn a_rebuild_in_place_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, exe) = tracked(dir.path(), "one");
    std::fs::write(&bin, "a longer one").unwrap();
    assert!(exe.replaced());
}

#[test]
fn a_removed_binary_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, exe) = tracked(dir.path(), "one");
    std::fs::remove_file(&bin).unwrap();
    assert!(exe.replaced());
}

#[test]
fn no_readable_executable_means_no_check() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!Exe::of([dir.path().join("gone")]).replaced());
    assert!(!Exe::of([]).replaced());
}

#[test]
fn a_path_that_is_another_file_is_not_tracked() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = tracked(dir.path(), "one");
    let other = dir.path().join("other");
    std::fs::write(&other, "something else").unwrap();
    let exe = Exe::of([bin, other.clone()]);
    std::fs::remove_file(&other).unwrap();
    assert!(!exe.replaced());
}

#[cfg(unix)]
#[test]
fn a_retargeted_symlink_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = tracked(dir.path(), "one");
    let other = dir.path().join("other");
    std::fs::write(&other, "one").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&bin, &link).unwrap();
    let exe = Exe::of([bin, link.clone()]);
    assert!(!exe.replaced());
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&other, &link).unwrap();
    assert!(exe.replaced());
}

/// Two servers on one source and one `--dir`, the first the writer.
fn pair() -> (tempfile::TempDir, tempfile::TempDir, Arc<Live>, Arc<Live>) {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    let a = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
    let b = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
    (src, out, a, b)
}

fn finds(wiki: &Wiki, word: &str) -> bool {
    !wiki.search(word, None, 10).is_empty()
}

#[test]
fn second_live_on_one_dir_is_a_reader() {
    let (_src, _out, a, b) = pair();
    assert!(a.is_writer());
    assert!(!b.is_writer());
}

#[test]
fn reader_sees_writer_edit() {
    let (src, _out, _a, b) = pair();
    std::fs::write(src.path().join("a.md"), "# A\n\nzebrafish\n").unwrap();
    let (wiki, stale) = b.fresh();
    assert_eq!(stale, None);
    assert!(finds(&wiki, "zebrafish"));
    assert_eq!(b.compiles.load(Ordering::Relaxed), 0);
}

#[cfg(unix)]
#[test]
fn reader_gets_writer_compile_error() {
    use std::os::unix::fs::PermissionsExt;
    let (src, out, a, b) = pair();
    let _ = a.fresh();
    std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(out.path().join("probe"), "").is_ok() {
        // Root writes anyway: no compile error to share.
        std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        panic!("needs a user that cannot write to a mode 555 directory (not root)");
    }
    std::fs::write(src.path().join("a.md"), "# A\n\nzebrafish\n").unwrap();
    let from_b = b.fresh().1;
    let from_a = a.fresh().1;
    std::fs::set_permissions(out.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(from_b, from_a);
    assert!(from_b.is_some_and(|e| e != RUNNING), "{from_a:?}");
}

#[test]
fn writer_drop_hands_over() {
    let (src, _out, a, b) = pair();
    let _ = a.fresh();
    a.release_writer();
    std::fs::write(src.path().join("a.md"), "# A\n\nzebrafish\n").unwrap();
    let (wiki, stale) = b.fresh();
    assert_eq!(stale, None);
    assert!(finds(&wiki, "zebrafish"));
    assert!(b.is_writer());
}

#[test]
fn reader_starts_on_empty_dir() {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    let (from, into) = (src.path().to_path_buf(), out.path().to_path_buf());
    let first = std::thread::spawn(move || Live::start(&from, &into, CompileOptions::default()));
    // `compile.lock` appears after the writer took `writer.lock`.
    let lock = out.path().join(BARRIER_DIR).join("compile.lock");
    let until = Instant::now() + Duration::from_secs(10);
    while !lock.exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    let second = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
    assert!(!second.is_writer());
    let started = Instant::now();
    let stale = second.fresh().1;
    let took = started.elapsed();
    assert!(took < Duration::from_millis(2500), "{took:?}");
    assert!(
        stale.is_none() || stale.as_deref() == Some(RUNNING),
        "{stale:?}"
    );
    assert!(first.join().unwrap().unwrap().is_writer());
}

#[test]
fn serve_dir_is_clean_after_calls() {
    let (_src, out, a, b) = pair();
    let _ = a.fresh();
    for _ in 0..20 {
        assert_eq!(b.fresh().1, None);
    }
    let mut names: Vec<String> = std::fs::read_dir(out.path().join(BARRIER_DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["compile.lock", "writer.lock"]);
}

#[test]
fn a_later_process_with_a_reused_pid_gets_a_new_id() {
    let first = server_id(0);
    std::thread::sleep(Duration::from_millis(2));
    assert_ne!(first, server_id(0));
}

#[test]
fn reader_call_without_edits_keeps_its_snapshot() {
    let (_src, _out, a, b) = pair();
    let _ = a.fresh();
    let (one, _) = b.fresh();
    let (two, stale) = b.fresh();
    assert_eq!(stale, None);
    assert!(Arc::ptr_eq(&one, &two));
}

#[test]
fn takeover_acks_every_waiting_barrier() {
    let (_src, out, a, b) = pair();
    let _ = a.fresh();
    a.release_writer();
    let serve = out.path().join(BARRIER_DIR);
    // Many, so `read_dir` order (hashed on APFS and ext4) is not token order.
    for token in 1..=12 {
        std::fs::write(serve.join(format!("barrier-1.1.0-{token}")), "").unwrap();
    }
    assert_eq!(b.fresh().1, None);
    for token in 1..=12 {
        assert!(serve.join(format!("ack-1.1.0-{token}")).exists(), "{token}");
    }
}

#[test]
fn a_timed_out_call_forgets_its_token() {
    let (_src, _out, a, b) = pair();
    let _ = a.fresh();
    // The writer stays but never acks: every call times out.
    a.lock().compiling = true;
    b.lock().compiled_gen = 0;
    let (_, stale) = b.fresh();
    assert_eq!(stale.as_deref(), Some(RUNNING));
    // An ack that lands after the call gave up.
    let token = b.lock().barrier_sent;
    let ack = b
        .dir
        .join(BARRIER_DIR)
        .join(format!("ack-{}-{token}", b.id));
    std::fs::write(&ack, "").unwrap();
    std::thread::sleep(watch::DEBOUNCE * 2);
    assert_eq!(b.lock().acked, Vec::<u64>::new());
}

#[test]
fn reader_sees_an_ack_whose_event_was_lost() {
    let (_src, _out, a, b) = pair();
    let _ = a.fresh();
    // The writer never acks; the ack is there before the barrier, so its
    // event finds no barrier and is dropped.
    a.lock().compiling = true;
    let token = b.lock().barrier_sent + 1;
    let ack = b
        .dir
        .join(BARRIER_DIR)
        .join(format!("ack-{}-{token}", b.id));
    std::fs::write(&ack, "").unwrap();
    std::thread::sleep(watch::DEBOUNCE * 2);
    let t = Instant::now();
    assert_eq!(b.fresh().1, None);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn foreign_barrier_takes_only_server_ids() {
    let (_src, out, a, _b) = pair();
    let serve = out.path().canonicalize().unwrap().join(BARRIER_DIR);
    assert_eq!(
        a.foreign_barrier(&serve.join("barrier-1.2.0-4")),
        Some(("1.2.0".to_string(), 4))
    );
    assert_eq!(a.foreign_barrier(&serve.join("barrier-123-4")), None);
    assert_eq!(a.foreign_barrier(&serve.join("barrier-1.x.0-4")), None);
}

#[test]
fn start_does_not_wait_for_a_cli_compile() {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    compile(src.path(), out.path(), &CompileOptions::default()).unwrap();
    std::fs::create_dir(out.path().join(BARRIER_DIR)).unwrap();
    let (release, cli) = crate::dirlock::hold(out.path(), Duration::from_secs(10), || ());
    let t = Instant::now();
    let a = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
    let b = Live::start(src.path(), out.path(), CompileOptions::default()).unwrap();
    // Well below the 10-second hold, loose enough for a loaded parallel run.
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(a.is_writer() && !b.is_writer());
    release.send(()).unwrap();
    cli.join().unwrap();
    assert!(finds(&a.fresh().0, "alpha"));
    assert!(finds(&b.fresh().0, "alpha"));
}

#[test]
fn reader_with_no_barrier_answers_at_once() {
    let (_src, _out, a, b) = pair();
    let _ = a.fresh();
    a.lock().compiling = true;
    // A directory where the next barrier goes: writing it fails.
    let token = b.lock().barrier_sent + 1;
    let barrier = b
        .dir
        .join(BARRIER_DIR)
        .join(format!("barrier-{}-{token}", b.id));
    std::fs::create_dir(&barrier).unwrap();
    let t = Instant::now();
    let stale = b.fresh().1;
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(stale.is_some_and(|e| e != RUNNING));
    std::fs::remove_dir(&barrier).unwrap();
}

#[test]
fn reader_reloads_on_a_new_compile_with_the_same_index() {
    let (_src, _out, a, b) = pair();
    let _ = a.fresh();
    let (one, _) = b.fresh();
    // The writer stalls; an ack from another compile, `index.json` untouched.
    a.lock().compiling = true;
    let token = b.lock().barrier_sent + 1;
    let ack = b
        .dir
        .join(BARRIER_DIR)
        .join(format!("ack-{}-{token}", b.id));
    std::fs::write(&ack, "other 99\n").unwrap();
    let (two, stale) = b.fresh();
    assert_eq!(stale, None);
    assert!(!Arc::ptr_eq(&one, &two));
}

#[test]
fn a_reader_reload_gives_up_at_the_call_deadline() {
    let (_src, _out, a, b) = pair();
    let _ = a.fresh();
    let (one, _) = b.fresh();
    // The writer stalls: no acks of its own.
    a.lock().compiling = true;
    let ack = |token: u64| {
        b.dir
            .join(BARRIER_DIR)
            .join(format!("ack-{}-{token}", b.id))
    };
    // An ack from another compile, waiting for b's next call.
    std::fs::write(ack(b.lock().barrier_sent + 1), "other 99\n").unwrap();
    // A compile holds the lock past the call's deadline.
    let (release, cli) = crate::dirlock::hold(&b.dir, WAIT + Duration::from_secs(3), || ());
    let t = Instant::now();
    let (two, stale) = b.fresh();
    assert!(
        t.elapsed() < WAIT + Duration::from_secs(1),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(stale.as_deref(), Some(RUNNING));
    assert!(Arc::ptr_eq(&one, &two));
    assert_ne!(b.lock().loaded.as_deref(), Some("other 99"));
    release.send(()).unwrap();
    cli.join().unwrap();
    // The next ack still reloads.
    std::fs::write(ack(b.lock().barrier_sent + 1), "other 99\n").unwrap();
    let (three, stale) = b.fresh();
    assert_eq!(stale, None);
    assert!(!Arc::ptr_eq(&one, &three));
}

#[test]
fn writer_scan_removes_orphans() {
    let (_src, out, a, _b) = pair();
    let _ = a.fresh();
    // No ack from the watcher meanwhile: it would be new, and stay.
    a.lock().compiling = true;
    let serve = out.path().join(BARRIER_DIR);
    let old = SystemTime::now() - WAIT * 3;
    for name in [
        "barrier-1.1.0-1",
        "ack-1.1.0-1",
        "tmp-ack-1.1.0-1",
        "ack-2.2.0-2",
    ] {
        let f = std::fs::File::create(serve.join(name)).unwrap();
        f.set_modified(old).unwrap();
    }
    std::fs::write(serve.join("barrier-3.3.0-1"), "").unwrap();
    a.scan_barriers(&mut a.lock(), Duration::ZERO);
    let mut names: Vec<String> = std::fs::read_dir(&serve)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["barrier-3.3.0-1", "compile.lock", "writer.lock"]);
}

#[test]
fn writer_acks_a_barrier_whose_event_was_lost() {
    let (_src, out, a, _b) = pair();
    let _ = a.fresh();
    let serve = out.path().join(BARRIER_DIR);
    // Its event is ignored as already seen, then forgotten: as if lost.
    a.lock().foreign_seen.insert("1.1.0".into(), 5);
    std::fs::write(serve.join("barrier-1.1.0-5"), "").unwrap();
    std::thread::sleep(watch::DEBOUNCE * 2);
    a.lock().foreign_seen.remove("1.1.0");
    let ack = serve.join("ack-1.1.0-5");
    let until = Instant::now() + WAIT;
    while !ack.exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ack.exists());
}

#[test]
fn a_panicking_compile_is_an_error() {
    let r: Result<(), WikiError> = unpanicked(|| panic!("boom"));
    assert_eq!(r.unwrap_err().to_string(), "the compile panicked");
}

/// A writer on a fresh source with `a.md`, its start-up compile done, and
/// the compile count at that point.
fn debounced(
    quiet_ms: u64,
    cap_ms: u64,
) -> (tempfile::TempDir, tempfile::TempDir, Arc<Live>, usize) {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.md"), "# A\n\nalpha\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    let debounce = Debounce {
        quiet: Duration::from_millis(quiet_ms),
        cap: Duration::from_millis(cap_ms),
    };
    let live =
        Live::start_with(src.path(), out.path(), CompileOptions::default(), debounce).unwrap();
    let _ = live.fresh();
    let before = live.compiles.load(Ordering::Relaxed);
    (src, out, live, before)
}

#[test]
fn a_burst_longer_than_the_quiet_window_compiles_once() {
    let (src, _out, live, before) = debounced(1000, 10_000);
    // 25 writes 50 ms apart: a 1.2 s burst, longer than the 1 s window. A
    // stalled machine can stretch a gap; one past half the window is a
    // stall, and only a gap past the whole window compiles. So a stall
    // may add a compile but need not: the count is a range, not a value.
    let mut starts = Vec::new();
    for i in 0..25 {
        starts.push(Instant::now());
        std::fs::write(src.path().join("a.md"), format!("# A\n\nword{i}\n")).unwrap();
        std::thread::sleep(Duration::from_millis(50));
    }
    let gaps: Vec<Duration> = starts.windows(2).map(|w| w[1] - w[0]).collect();
    let stalls = gaps
        .iter()
        .filter(|g| **g > Duration::from_millis(500))
        .count();
    let (wiki, stale) = live.fresh();
    assert_eq!(stale, None);
    assert!(finds(&wiki, "word24"));
    // Anything still pending would compile by now.
    std::thread::sleep(Duration::from_millis(1500));
    let n = live.compiles.load(Ordering::Relaxed) - before;
    assert!(
        (1..=1 + stalls).contains(&n),
        "{n} compiles, {stalls} stalls, largest gap {} ms",
        gaps.iter().max().unwrap().as_millis()
    );
}

#[test]
fn a_constant_writer_still_compiles_within_the_cap() {
    let (src, _out, live, before) = debounced(300, 600);
    // A write every 50 ms for 2 s: the 300 ms window never passes.
    let until = Instant::now() + Duration::from_secs(2);
    let mut i = 0;
    while Instant::now() < until {
        std::fs::write(src.path().join("a.md"), format!("# A\n\nword{i}\n")).unwrap();
        i += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(live.compiles.load(Ordering::Relaxed) > before);
}

#[test]
fn serve_files_do_not_extend_the_quiet_window() {
    let (src, out, live, before) = debounced(300, 10_000);
    std::fs::write(src.path().join("a.md"), "# A\n\nbeta\n").unwrap();
    // `.serve` churn every 50 ms for 1.5 s is not an edit.
    let until = Instant::now() + Duration::from_millis(1500);
    let mut i = 0;
    while Instant::now() < until {
        std::fs::write(out.path().join(BARRIER_DIR).join(format!("x-{i}")), "").unwrap();
        i += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(live.compiles.load(Ordering::Relaxed) > before);
}
