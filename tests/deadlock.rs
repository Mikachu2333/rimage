//! Regression tests that exercise the built binary end to end.
//!
//! These only compile when the binary is built (`--features build-binary`),
//! because `CARGO_BIN_EXE_rimage` is only defined then.

#![cfg(feature = "build-binary")]

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The `ConcurrencyLimiter` used to deadlock with the default `-t 1` setting
/// whenever more than one input file was processed: the main thread blocked on
/// `acquire()` inside `rayon::scope`, but with a single-threaded pool it was
/// the pool's only worker. Regression test: multiple files with `-t 1` must
/// complete.
#[test]
fn multiple_files_with_single_thread_do_not_deadlock() {
    let exe = env!("CARGO_BIN_EXE_rimage");
    let manifest = env!("CARGO_MANIFEST_DIR");
    let source = format!("{manifest}/tests/files/jpg/f1t.jpg");
    let input_dir = format!("{manifest}/target/tmp-deadlock-input");
    let img = format!("{input_dir}/f1t.jpg");
    let second = format!("{input_dir}/f2t.jpg");
    let out = format!("{manifest}/target/tmp-deadlock-test");
    let _ = std::fs::remove_dir_all(&input_dir);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::copy(&source, &img).unwrap();
    std::fs::copy(&source, &second).unwrap();
    std::fs::create_dir_all(&out).unwrap();

    let mut child = Command::new(exe)
        .args(["png", &img, &second, "-d", &out, "-r", "-t", "1", "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("rimage deadlocked with -t 1 and multiple input files");
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    assert!(status.success(), "rimage exited with {status}");

    // The recursive layout must be preserved under the output directory.
    assert!(std::path::Path::new(&out).join("f1t.png").exists());
    assert!(std::path::Path::new(&out).join("f2t.png").exists());
    let _ = std::fs::remove_dir_all(&input_dir);
    let _ = std::fs::remove_dir_all(&out);
}

/// A single input file with `-r -d` must stay under the output directory
/// (regression: the common-path computation used to be performed on the full
/// file path, so a single file produced a broken output path).
#[test]
fn single_file_recursive_stays_under_output_directory() {
    let exe = env!("CARGO_BIN_EXE_rimage");
    let manifest = env!("CARGO_MANIFEST_DIR");
    let img = format!("{manifest}/tests/files/jpg/f1t.jpg");
    let out = format!("{manifest}/target/tmp-recursive-test");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();

    let status = Command::new(exe)
        .args(["png", &img, "-d", &out, "-r", "-t", "2", "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    assert!(status.success(), "rimage exited with {status}");
    assert!(
        std::path::Path::new(&out).join("f1t.png").exists(),
        "expected output under {out}/f1t.png"
    );
    let _ = std::fs::remove_dir_all(&out);
}

/// A same-format, in-place conversion with `--backup` and a suffix that
/// produces the same name as the backup (e.g. `-b -s @backup` on `img.png`)
/// must fail instead of publishing the temp output over the backup holding
/// the original image.
#[test]
fn backup_path_collision_does_not_destroy_original() {
    let exe = env!("CARGO_BIN_EXE_rimage");
    let manifest = env!("CARGO_MANIFEST_DIR");
    let dir = format!("{manifest}/target/tmp-backup-collision");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let input = format!("{dir}/img.png");
    std::fs::copy(format!("{manifest}/tests/files/png/f1t.png"), &input).unwrap();
    let original = std::fs::read(&input).unwrap();

    let status = Command::new(exe)
        .args(["png", &input, "-b", "-s", "@backup", "-t", "2", "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    assert!(!status.success(), "colliding -b/-s must not report success");

    // The original must be untouched and no backup or temp file must remain.
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert!(!std::path::Path::new(&format!("{dir}/img@backup.png")).exists());
    let residue = std::fs::read_dir(&dir)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("rimage-tmp")
        })
        .count();
    assert_eq!(residue, 0, "no temporary output files may remain");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A case-variant of `@backup` (e.g. `-b -s @Backup`) maps the output to
/// `img@Backup.png`, which is the same file as the backup `img@backup.png` on
/// a case-insensitive filesystem. Path comparisons treat names as
/// case-insensitive on every platform, so this must fail instead of
/// publishing over the backup.
#[test]
fn backup_case_variant_collision_does_not_destroy_original() {
    let exe = env!("CARGO_BIN_EXE_rimage");
    let manifest = env!("CARGO_MANIFEST_DIR");
    let dir = format!("{manifest}/target/tmp-backup-case-collision");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let input = format!("{dir}/img.png");
    std::fs::copy(format!("{manifest}/tests/files/png/f1t.png"), &input).unwrap();
    let original = std::fs::read(&input).unwrap();

    let status = Command::new(exe)
        .args(["png", &input, "-b", "-s", "@Backup", "-t", "2", "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    assert!(
        !status.success(),
        "case-variant colliding -b/-s must not report success"
    );
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert!(!std::path::Path::new(&format!("{dir}/img@backup.png")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `--backup` destination left by an earlier run must never be overwritten
/// or deleted: it holds the preserved original image.
#[test]
fn existing_backup_is_never_overwritten() {
    let exe = env!("CARGO_BIN_EXE_rimage");
    let manifest = env!("CARGO_MANIFEST_DIR");
    let dir = format!("{manifest}/target/tmp-existing-backup");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let input = format!("{dir}/img.png");
    std::fs::copy(format!("{manifest}/tests/files/png/f1t.png"), &input).unwrap();
    let original_input = std::fs::read(&input).unwrap();

    let backup = format!("{dir}/img@backup.png");
    let precious = b"pre-existing backup content".to_vec();
    std::fs::write(&backup, &precious).unwrap();

    let status = Command::new(exe)
        .args(["png", &input, "-b", "-t", "2", "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    assert!(
        !status.success(),
        "an existing backup must not report success"
    );
    assert_eq!(
        std::fs::read(&input).unwrap(),
        original_input,
        "input must be untouched"
    );
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        precious,
        "pre-existing backup must be preserved"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Failure to produce every output must be reflected in a non-zero exit code.
#[test]
fn exit_code_is_nonzero_when_a_file_fails() {
    let exe = env!("CARGO_BIN_EXE_rimage");
    let manifest = env!("CARGO_MANIFEST_DIR");
    let missing = format!("{manifest}/tests/files/jpg/does-not-exist.jpg");
    let out = format!("{manifest}/target/tmp-exitcode-test");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();

    let status = Command::new(exe)
        .args(["png", &missing, "-d", &out, "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    assert!(
        !status.success(),
        "a run with no producible output must not report success"
    );
    let _ = std::fs::remove_dir_all(&out);
}

/// A `file.list` input is expanded into the files it lists, one per line, and
/// any other input arguments are ignored.
#[test]
fn file_list_expands_to_its_listed_inputs() {
    let exe = env!("CARGO_BIN_EXE_rimage");
    let manifest = env!("CARGO_MANIFEST_DIR");
    let source = format!("{manifest}/tests/files/jpg/f1t.jpg");
    let input_dir = format!("{manifest}/target/tmp-file-list-input");
    let first = format!("{input_dir}/first.jpg");
    let second = format!("{input_dir}/second.jpg");
    let ignored = format!("{input_dir}/ignored.jpg");
    let list = format!("{input_dir}/file.list");
    let out = format!("{manifest}/target/tmp-file-list-test");
    let _ = std::fs::remove_dir_all(&input_dir);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::copy(&source, &first).unwrap();
    std::fs::copy(&source, &second).unwrap();
    std::fs::copy(&source, &ignored).unwrap();
    std::fs::write(&list, format!("{first}\n{second}\n")).unwrap();
    std::fs::create_dir_all(&out).unwrap();

    let status = Command::new(exe)
        .args(["png", &list, &ignored, "-d", &out, "--quiet"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    assert!(status.success(), "rimage exited with {status}");
    assert!(
        std::path::Path::new(&out).join("first.png").exists(),
        "expected output under {out}/first.png"
    );
    assert!(
        std::path::Path::new(&out).join("second.png").exists(),
        "expected output under {out}/second.png"
    );
    assert!(
        !std::path::Path::new(&out).join("ignored.png").exists(),
        "other input arguments must be ignored when file.list is provided"
    );
    let _ = std::fs::remove_dir_all(&input_dir);
    let _ = std::fs::remove_dir_all(&out);
}
