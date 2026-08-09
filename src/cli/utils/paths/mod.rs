use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
};

/// Maps each accessible input file to a unique output path.
///
/// With `recursive` + `out_dir`, the relative parent structure under the
/// common input parent is preserved. Recursive inputs without a common root and
/// inputs that map to the same output are rejected instead of being flattened
/// or raced. The suffix and final file name are validated before work starts.
pub fn get_paths(
    files: Vec<PathBuf>,
    out_dir: Option<PathBuf>,
    suffix: Option<String>,
    recursive: bool,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    if let Some(suffix) = suffix.as_deref()
        && !valid_suffix(suffix)
    {
        return Err(format!(
            "Invalid suffix {suffix:?}: it must be a plain file-name fragment"
        ));
    }

    let files = files
        .into_iter()
        .filter(|path| match check_input_file(path) {
            Ok(()) => true,
            Err(InputPathError::MissingOrNotFile(reason)) => {
                log::warn!("{path:?}: {reason}");
                false
            }
            Err(InputPathError::Io(error)) => {
                log::error!("{path:?}: cannot be accessed: {error}");
                false
            }
        })
        .collect::<Vec<_>>();

    let common_path = if recursive {
        let parents = files
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let common = get_common_path(&parents);
        if !files.is_empty() && common.is_none() {
            return Err(
                "Recursive inputs do not have a common filesystem root; split them into separate commands"
                    .to_string(),
            );
        }
        common
    } else {
        None
    };

    let mut mapped: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(files.len());
    for path in files {
        let file_stem = path
            .file_stem()
            .unwrap_or_else(|| OsStr::new("optimized_image"));
        let file_name = match suffix.as_deref() {
            Some(suffix) => file_name_with_suffix(file_stem, suffix),
            None => file_stem.to_os_string(),
        };
        validate_output_file_name(&file_name)?;

        let mut out_path = match &out_dir {
            Some(dir) => match &common_path {
                Some(common) => {
                    let parent = path.parent().ok_or_else(|| {
                        format!("Input path {path:?} does not have a parent directory")
                    })?;
                    let relative = strip_prefix_components(parent, common).ok_or_else(|| {
                        format!("Cannot preserve directory structure for {path:?} under {common:?}")
                    })?;
                    dir.join(relative)
                }
                None => dir.clone(),
            },
            None => path.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        out_path.push(file_name);
        let out_path = normalize_lexically(&out_path);

        if mapped
            .iter()
            .any(|(_, existing)| paths_equivalent(existing, &out_path))
        {
            return Err(format!(
                "Multiple inputs map to the same output path: {}",
                out_path.display()
            ));
        }
        mapped.push((path, out_path));
    }

    Ok(mapped)
}

enum InputPathError {
    MissingOrNotFile(&'static str),
    Io(std::io::Error),
}

fn check_input_file(path: &Path) -> Result<(), InputPathError> {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(()),
        Ok(_) => Err(InputPathError::MissingOrNotFile("is not a file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(
            InputPathError::MissingOrNotFile("does not exist or is a dangling symbolic link"),
        ),
        Err(error) => Err(InputPathError::Io(error)),
    }
}

fn valid_suffix(suffix: &str) -> bool {
    !suffix.is_empty()
        && !suffix.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        && !suffix.chars().any(|c| c <= '\u{1f}')
        && !suffix.ends_with([' ', '.'])
        && Path::new(suffix)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn file_name_with_suffix(file_name: &OsStr, suffix: &str) -> OsString {
    let mut name = file_name.to_os_string();
    name.push(suffix);
    name
}

fn validate_output_file_name(name: &OsStr) -> Result<(), String> {
    #[cfg(not(windows))]
    let _ = name;

    #[cfg(windows)]
    {
        let text = name
            .to_str()
            .ok_or_else(|| "Output file name is not valid Unicode on Windows".to_string())?;
        if text.is_empty()
            || text.ends_with([' ', '.'])
            || text.chars().any(|c| {
                c <= '\u{1f}' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            })
        {
            return Err(format!("Output file name {text:?} is not valid on Windows"));
        }
        let base = text.split('.').next().unwrap_or_default();
        let reserved = matches!(
            base.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        );
        if reserved {
            return Err(format!("Output file name {text:?} is reserved on Windows"));
        }
    }
    Ok(())
}

fn get_common_path(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut common = paths.first()?.clone();
    for path in paths.iter().skip(1) {
        common = common
            .components()
            .zip(path.components())
            .take_while(|(a, b)| component_eq(a.as_os_str(), b.as_os_str()))
            .map(|(component, _)| component.as_os_str())
            .collect();
    }
    (!common.as_os_str().is_empty()).then_some(common)
}

fn strip_prefix_components(path: &Path, base: &Path) -> Option<PathBuf> {
    let mut path_components = path.components();
    for base_component in base.components() {
        let path_component = path_components.next()?;
        if !component_eq(path_component.as_os_str(), base_component.as_os_str()) {
            return None;
        }
    }
    Some(
        path_components
            .map(|component| component.as_os_str())
            .collect(),
    )
}

/// Compares two path components, treating names as case-insensitive on every
/// platform.
///
/// This is a deliberate fail-safe default. Case-insensitive filesystems
/// (Windows, macOS, and case-insensitive Linux mounts such as SMB/NAS) can
/// hold `img@Backup.png` and `img@backup.png` as the same file. Treating them
/// as equivalent even on genuinely case-sensitive filesystems only makes
/// collision checks stricter; missing the equivalence could let a publish
/// overwrite the `--backup` copy holding the original image.
fn component_eq(a: &OsStr, b: &OsStr) -> bool {
    if a == b {
        return true;
    }
    match (a.to_str(), b.to_str()) {
        // Use Unicode-aware folding where possible; APFS folds more than ASCII.
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// Compares two paths for filesystem-level equivalence.
///
/// Paths that already exist are canonicalized first: that resolves symlink
/// aliasing, `..` components, on-disk case, Unicode normalization, and
/// Windows verbatim prefixes. Planned outputs usually do not exist yet, so
/// the fallback is a component-wise comparison that treats names as
/// case-insensitive by default (see [`component_eq`]).
pub(crate) fn paths_equivalent(a: &Path, b: &Path) -> bool {
    if let (Ok(canonical_a), Ok(canonical_b)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        return canonical_a == canonical_b;
    }

    let mut a = a.components();
    let mut b = b.components();
    loop {
        match (a.next(), b.next()) {
            (Some(a), Some(b)) if component_eq(a.as_os_str(), b.as_os_str()) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    log::debug!("path {} escapes its root; clamped", path.display());
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Expands quoted glob patterns consistently while giving an existing literal
/// path precedence. This preserves valid Unix file names containing glob
/// metacharacters and follows file symlinks consistently.
#[inline]
pub fn collect_files<P: AsRef<Path>>(input: &[P]) -> Vec<PathBuf> {
    input.iter().flat_map(apply_glob_pattern).collect()
}

fn apply_glob_pattern<P: AsRef<Path>>(path: P) -> Vec<PathBuf> {
    let path = path.as_ref();
    if path.exists() || !contains_glob_meta(path) {
        return vec![path.to_path_buf()];
    }

    let Some(pattern) = path.to_str() else {
        log::warn!("Glob pattern {path:?} is not valid UTF-8; treating it literally");
        return vec![path.to_path_buf()];
    };
    let Ok(paths) = glob::glob(pattern) else {
        log::warn!("Invalid glob pattern {pattern:?}; treating it literally");
        return vec![path.to_path_buf()];
    };

    let mut matches = Vec::new();
    for entry in paths {
        match entry {
            Ok(path) => matches.push(path),
            Err(error) => log::error!("Failed while expanding glob {pattern:?}: {error}"),
        }
    }
    if matches.is_empty() {
        log::warn!("No files matched glob pattern {pattern:?}");
    }
    matches
}

fn contains_glob_meta(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|s| s.contains(['*', '?', '[', ']']))
}

/// File name of the UTF-8 file list that is expanded into its listed inputs.
const FILE_LIST_NAME: &str = "file.list";

/// Expands `file.list` inputs into the files they list.
///
/// When at least one input is a `file.list`, all other input arguments are
/// ignored and only the file lists are expanded. Each `file.list` (compared
/// case-insensitively as a fail-safe default) is read as a UTF-8 text file
/// containing one file path per line. Blank lines and surrounding whitespace
/// are ignored. Every listed path is passed through `normalize` (tilde
/// expansion and current-directory joining) and then expanded with
/// [`collect_files`], so glob patterns work inside the list. Nested
/// `file.list` entries are not expanded recursively.
pub fn expand_file_lists<F>(inputs: &[PathBuf], normalize: F) -> Result<Vec<PathBuf>, String>
where
    F: Fn(&Path) -> PathBuf,
{
    let has_file_list = inputs.iter().any(|input| is_file_list_path(input));
    if has_file_list {
        let ignored = inputs
            .iter()
            .filter(|input| !is_file_list_path(input))
            .collect::<Vec<_>>();
        if !ignored.is_empty() {
            log::warn!(
                "file.list input(s) provided; ignoring {} other input argument(s)",
                ignored.len()
            );
            log::debug!("Ignored input arguments: {ignored:#?}");
        }
    }

    let mut files = Vec::new();
    for input in inputs {
        if is_file_list_path(input) {
            for entry in read_file_list_entries(input)? {
                let normalized = normalize(&entry);
                files.extend(collect_files(std::slice::from_ref(&normalized)));
            }
        } else if !has_file_list {
            files.extend(collect_files(std::slice::from_ref(input)));
        }
    }
    Ok(files)
}

fn is_file_list_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(FILE_LIST_NAME))
}

fn read_file_list_entries(path: &Path) -> Result<Vec<PathBuf>, String> {
    let content = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read file list {} as UTF-8: {error}",
            path.display()
        )
    })?;
    Ok(content
        .strip_prefix('\u{FEFF}')
        .unwrap_or(&content)
        .lines()
        .map(|s| {
            let s = s.trim().trim_matches(['"', '\'', '/', '\\']);
            if s.contains("\u{FFFD}") {
                log::warn!(
                    "Line {s} in `file.list` {} may not encoding with valid UTF-8.",
                    path.display()
                );
            }
            s
        })
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

#[cfg(test)]
mod tests;
