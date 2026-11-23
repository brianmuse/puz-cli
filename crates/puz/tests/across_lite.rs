//! Editing an Across Lite `.puz` file: puz never changes it, and saves the edit to `NAME.ipuz`.

use std::fs;
use std::process::Command;

use puz_core::puzzle::Puzzle;

#[test]
fn editing_a_puz_file_writes_an_ipuz_beside_it() {
    let dir = std::env::temp_dir().join(format!("puz-across-lite-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let p = Puzzle::from_json(r#"{"title":"Mini","grid":["CAT","A.E","TEN"]}"#).unwrap();
    let src = dir.join("mini.puz");
    fs::write(&src, puz_core::formats::to_puz(&p).unwrap()).unwrap();
    let before = fs::read(&src).unwrap();
    let puz = |words: &[&str]| Command::new(env!("CARGO_BIN_EXE_puz")).args(words).env("PUZ_CONFIG_DIR", dir.join("config")).env_remove("PUZ_LIBRARY").output().unwrap();

    // Reading works as for any puzzle file.
    let out = puz(&["show", src.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    // The first edit writes mini.ipuz and says so; the .puz stays as it was.
    let out = puz(&["set", src.to_str().unwrap(), "--title", "First"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("mini.ipuz"));
    assert_eq!(fs::read(&src).unwrap(), before);
    let saved = Puzzle::load(&dir.join("mini.ipuz")).unwrap();
    assert_eq!(saved.title, "First");

    // An existing mini.ipuz is never overwritten: the next edit of the .puz goes to mini-2.ipuz.
    let out = puz(&["set", src.to_str().unwrap(), "--title", "Second"]);
    assert!(out.status.success());
    assert_eq!(Puzzle::load(&dir.join("mini.ipuz")).unwrap().title, "First");
    assert_eq!(Puzzle::load(&dir.join("mini-2.ipuz")).unwrap().title, "Second");
    assert_eq!(fs::read(&src).unwrap(), before);
    let _ = fs::remove_dir_all(&dir);
}
