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

    let common_path = get_common_path(&paths);
    assert_eq!(common_path, Some(PathBuf::from("/path/to")));

    let paths = vec![
        PathBuf::from("/path/to/test/image1.jpg"),
        PathBuf::from("/path/to/image2.jpg"),
        PathBuf::from("/path/to/image3.jpg"),
    ];

    let common_path = get_common_path(&paths);
    assert_eq!(common_path, Some(PathBuf::from("/path/to")));

    let paths = vec![
        PathBuf::from("/path/to/test/image1.jpg"),
        PathBuf::from("/path/image2.jpg"),
        PathBuf::from("/path/to/image3.jpg"),
    ];

    let common_path = get_common_path(&paths);
    assert_eq!(common_path, Some(PathBuf::from("/path")));
}

#[test]
fn recursive_single_file_stays_under_output_directory() {
    let root = unique_test_dir("recursive-single");
    let input_dir = root.join("input/photos");
    let output_root = root.join("output");
    fs::create_dir_all(&input_dir).unwrap();
    let input = input_dir.join("image.jpg");
    fs::write(&input, b"test").unwrap();

    let paths =
        get_paths(vec![input.clone()], Some(output_root.clone()), None, true).collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![(input, output_root.join("image"))],
        "an absolute input path must not replace the selected output directory"
    );
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

    let paths =
        get_paths(vec![first, second], Some(output_root.clone()), None, true).collect::<Vec<_>>();

    assert_eq!(paths[0].1, output_root.join("a/image"));
    assert_eq!(paths[1].1, output_root.join("b/image"));
    fs::remove_dir_all(root).unwrap();
}
