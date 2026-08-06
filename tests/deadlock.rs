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
