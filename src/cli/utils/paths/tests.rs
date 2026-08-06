use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;

fn unique_test_dir(name: &str) -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rimage-{name}-{}-{id}", std::process::id()))
}

#[test]
fn find_common_path() {
    let paths = vec![
        PathBuf::from("/path/to/image1.jpg"),
        PathBuf::from("/path/to/image2.jpg"),
        PathBuf::from("/path/to/image3.jpg"),
    ];
    assert_eq!(get_common_path(&paths), Some(PathBuf::from("/path/to")));

    let paths = vec![
        PathBuf::from("/path/to/test/image1.jpg"),
        PathBuf::from("/path/image2.jpg"),
        PathBuf::from("/path/to/image3.jpg"),
    ];
    assert_eq!(get_common_path(&paths), Some(PathBuf::from("/path")));
}

#[test]
fn common_path_is_none_for_empty_input() {
    assert_eq!(get_common_path(&[]), None);
}

#[cfg(windows)]
#[test]
fn common_path_is_none_for_unrelated_windows_drives() {
    let paths = vec![
        PathBuf::from(r"C:\projects\a\image1.jpg"),
        PathBuf::from(r"D:\projects\a\image2.jpg"),
    ];
    assert_eq!(get_common_path(&paths), None);
}

#[cfg(windows)]
#[test]
fn common_path_and_strip_prefix_ignore_ascii_case_on_windows() {
    let paths = vec![PathBuf::from(r"C:\Photos\a"), PathBuf::from(r"c:\photos\b")];
    let common = get_common_path(&paths).unwrap();
    assert_eq!(common, PathBuf::from(r"C:\Photos"));
    assert_eq!(
        strip_prefix_components(Path::new(r"c:\photos\b"), &common),
        Some(PathBuf::from("b"))
    );
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn paths_equivalent_ignores_ascii_case_on_case_insensitive_filesystems() {
    // Regression: a same-format conversion with `--backup` and a suffix such
    // as `@Backup` maps the output to `img@Backup.png` while the backup is
    // `img@backup.png`. On a case-insensitive filesystem those are the same
    // file, so the collision must be detected before publishing over it.
    assert!(paths_equivalent(
        Path::new("/img@Backup.png"),
        Path::new("/img@backup.png")
    ));
    assert!(paths_equivalent(
        Path::new("/photos/a/B.JPG"),
        Path::new("/PHOTOS/A/b.jpg")
    ));
    assert!(!paths_equivalent(Path::new("/a.png"), Path::new("/b.png")));
}

#[cfg(unix)]
#[test]
fn paths_equivalent_resolves_symlink_aliases() {
    use std::os::unix::fs::symlink;

    let root = unique_test_dir("path-equiv-symlink");
    fs::create_dir_all(&root).unwrap();
    let real = root.join("real");
    fs::create_dir_all(&real).unwrap();
    let file = real.join("a.png");
    fs::write(&file, b"x").unwrap();
    let link = root.join("link");
    symlink(&real, &link).unwrap();

    assert!(paths_equivalent(&link.join("a.png"), &file));
    assert!(!paths_equivalent(&link.join("b.png"), &file));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recursive_single_file_stays_under_output_directory() {
    let root = unique_test_dir("recursive-single");
    let input_dir = root.join("input/photos");
    let output_root = root.join("output");
    fs::create_dir_all(&input_dir).unwrap();
    let input = input_dir.join("image.jpg");
    fs::write(&input, b"test").unwrap();

    let paths = get_paths(vec![input.clone()], Some(output_root.clone()), None, true).unwrap();

    assert_eq!(paths, vec![(input, output_root.join("image"))]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recursive_files_preserve_relative_parent_structure() {
    let root = unique_test_dir("recursive-multiple");
    let input_root = root.join("input");
    let output_root = root.join("output");
    fs::create_dir_all(input_root.join("a")).unwrap();
    fs::create_dir_all(input_root.join("b")).unwrap();
    let first = input_root.join("a/image.jpg");
    let second = input_root.join("b/image.jpg");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();

    let paths = get_paths(vec![first, second], Some(output_root.clone()), None, true).unwrap();

    assert_eq!(paths[0].1, output_root.join("a/image"));
    assert_eq!(paths[1].1, output_root.join("b/image"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_outputs_are_rejected() {
    let root = unique_test_dir("duplicate-output");
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    let first = root.join("a/image.jpg");
    let second = root.join("b/image.png");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();

    let error = get_paths(vec![first, second], Some(root.join("out")), None, false).unwrap_err();
    assert!(error.contains("same output path"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_suffixes_are_rejected() {
    for bad in [
        "",
        "..\\evil",
        "a/b",
        "..",
        ".",
        "a\\b",
        "out:2x",
        "na*me",
        "bad\u{1f}",
        "tail.",
        "tail ",
    ] {
        assert!(!valid_suffix(bad), "suffix {bad:?} must be rejected");
    }
    for ok in ["2x", "updated", "v2.1", "a..b"] {
        assert!(valid_suffix(ok), "suffix {ok:?} must be accepted");
    }
}

#[test]
fn normalize_lexically_removes_dot_components() {
    assert_eq!(
        normalize_lexically(Path::new("/a/./b/../c")),
        PathBuf::from("/a/c")
    );
    assert_eq!(normalize_lexically(Path::new("a/../b")), PathBuf::from("b"));
}

#[test]
fn collect_files_expands_globs_but_prefers_existing_literals() {
    let root = unique_test_dir("glob");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.jpg"), b"x").unwrap();
    fs::write(root.join("b.jpg"), b"x").unwrap();
    fs::write(root.join("photo[1].jpg"), b"literal").unwrap();
    fs::write(root.join("photo1.jpg"), b"pattern match").unwrap();

    let pattern = root.join("*.jpg");
    let mut files = collect_files(std::slice::from_ref(&pattern));
    files.sort();
    assert_eq!(files.len(), 4);

    let literal = root.join("photo[1].jpg");
    assert_eq!(collect_files(std::slice::from_ref(&literal)), vec![literal]);
    assert!(collect_files(&[root.join("nomatch_*.jpg")]).is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn file_symlinks_are_accepted() {
    use std::os::unix::fs::symlink;

    let root = unique_test_dir("symlink");
    fs::create_dir_all(&root).unwrap();
    let target = root.join("target.jpg");
    let link = root.join("link.jpg");
    fs::write(&target, b"x").unwrap();
    symlink(&target, &link).unwrap();

    assert!(check_input_file(&link).is_ok());
    fs::remove_dir_all(root).unwrap();
}
