mod common;

use std::path::Path;
use std::process::{Command, Output};

fn plaz(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_plaz"))
        .args(["setup", "--source"])
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    common::write_files(
        d.path(),
        &[("src/a.rs", "pub fn f(x: u8) { if x > 1 {} }\n")],
    );
    d
}

fn git_init(dir: &Path) {
    let ok = Command::new("git")
        .args(["init", "-q"])
        .arg(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn setup_repo_builds_the_map_and_ignores_the_folder() {
    let d = project();
    let o = plaz(d.path(), &["--scope", "repo"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(d.path().join(".kulman/map/src/a_rs.md").is_file());
    assert_eq!(read(d.path().join(".gitignore")), ".kulman/\n");
}

#[test]
fn setup_adds_the_line_once() {
    let d = project();
    plaz(d.path(), &["--scope", "repo"]);
    plaz(d.path(), &["--scope", "repo"]);
    assert_eq!(read(d.path().join(".gitignore")), ".kulman/\n");

    let d = project();
    std::fs::write(d.path().join(".gitignore"), "target").unwrap();
    plaz(d.path(), &["--scope", "repo"]);
    assert_eq!(read(d.path().join(".gitignore")), "target\n.kulman/\n");
}

#[test]
fn setup_local_writes_the_exclude_file() {
    let d = project();
    git_init(d.path());
    let o = plaz(d.path(), &["--scope", "local"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(read(d.path().join(".git/info/exclude")).ends_with(".kulman/\n"));
    assert!(!d.path().join(".gitignore").exists());

    // a line in the other file counts, and a second run adds nothing
    let before = read(d.path().join(".git/info/exclude"));
    plaz(d.path(), &["--scope", "local"]);
    assert_eq!(read(d.path().join(".git/info/exclude")), before);

    let d = project();
    git_init(d.path());
    std::fs::write(d.path().join(".gitignore"), " .kulman/ \n").unwrap();
    let before = read(d.path().join(".git/info/exclude"));
    plaz(d.path(), &["--scope", "local"]);
    assert_eq!(read(d.path().join(".git/info/exclude")), before);

    // and the other way round: repo scope sees a line in the exclude file
    let d = project();
    git_init(d.path());
    plaz(d.path(), &["--scope", "local"]);
    plaz(d.path(), &["--scope", "repo"]);
    assert!(!d.path().join(".gitignore").exists());
}

#[test]
fn setup_local_outside_git_says_so() {
    let d = project();
    let o = plaz(d.path(), &["--scope", "local"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).contains("not a git repository: nothing to exclude"));
    assert!(d.path().join(".kulman/map/src/a_rs.md").is_file());
    assert!(!d.path().join(".gitignore").exists());
}

#[test]
fn setup_without_scope_is_a_usage_error() {
    let d = project();
    for args in [&[][..], &["--scope", "world"], &["--scope"], &["--bogus"]] {
        let o = plaz(d.path(), args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&o.stderr).starts_with("usage: plaz"));
    }
    assert!(!d.path().join(".kulman").exists());
    assert!(!d.path().join(".gitignore").exists());
}

#[test]
fn remove_takes_only_the_maps_pages() {
    let d = project();
    let o = plaz(d.path(), &["--scope", "repo"]);
    let written: usize = stdout(&o)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let kulman = d.path().join(".kulman");
    let foreign = [
        ("map/note.md", "hand-written\n"),
        (
            "map/old.md",
            "---\ntags: [\"logic-map\"]\nkind: map\n---\nold\n",
        ),
        ("index.md", "# index\n"),
    ];
    common::write_files(&kulman, &foreign);

    let o = plaz(d.path(), &["--remove"]);
    assert_eq!(o.status.code(), Some(0));
    // `write` counts a foreign file as left alone only where it would
    // overwrite one; with no pages to write it skips them silently.
    assert_eq!(stdout(&o), format!("map: {written} removed\n"));
    for (path, text) in foreign {
        assert_eq!(read(kulman.join(path)), text);
    }
    assert!(!kulman.join("map/src").exists());
    assert_eq!(read(d.path().join(".gitignore")), ".kulman/\n");

    // with nothing else in map/, the folder goes too
    std::fs::remove_file(kulman.join("map/note.md")).unwrap();
    std::fs::remove_file(kulman.join("map/old.md")).unwrap();
    plaz(d.path(), &["--scope", "repo"]);
    plaz(d.path(), &["--remove"]);
    assert!(!kulman.join("map").exists());
    assert!(kulman.join("index.md").is_file());
}

#[test]
fn remove_with_nothing_built_is_fine() {
    let d = project();
    let o = plaz(d.path(), &["--remove"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(stdout(&o), "map: 0 removed\n");
    assert!(!d.path().join(".kulman").exists());
}
