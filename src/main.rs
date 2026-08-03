use std::{
    collections::HashSet,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use cli::{
    cli,
    pipeline::{decode, operations},
    utils::paths::{collect_files, get_paths},
};
use console::{Term, style};
use indicatif::{DecimalBytes, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use indicatif_log_bridge::LogWrapper;
use little_exif::metadata::Metadata as ExifMetadata;
use rimage::operations::icc::ApplySRGB;
use serde::{Deserialize, Serialize};
use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace};
use zune_image::{
    core_filters::{colorspace::ColorspaceConv, depth::Depth},
    traits::OperationsTrait,
};
use zune_imageprocs::auto_orient::AutoOrient;

use crate::cli::pipeline::encoder;

mod cli;

macro_rules! handle_error {
    ( $path:expr, $e:expr ) => {
        match $e {
            Ok(v) => v,
            Err(e) => {
                log::error!("{}: {e}", $path.display());
                return;
            }
        }
    };
}

const SUPPORTS_EXIF: &[&str; 7] = &[
    "mozjpeg", "oxipng", "png", "jpeg", "jpeg_xl", "tiff", "webp",
];
const SUPPORTS_ICC: &[&str; 2] = &["mozjpeg", "oxipng"];

struct Result {
    output: PathBuf,
    input_size: u64,
    output_size: u64,
}

struct ProcessingState {
    results: Vec<Result>,
    metadata: Option<Metadata>,
}

impl ProcessingState {
    fn new() -> Self {
        Self {
            results: vec![],
            metadata: None,
        }
    }
}

/// RAII guard that updates progress bars on drop, ensuring they advance
/// even when a worker returns early due to an error.
struct FinishGuard {
    pb: ProgressBar,
    pb_main: ProgressBar,
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        self.pb.finish_and_clear();
        self.pb_main.inc(1);
    }
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TemporaryOutput {
    path: PathBuf,
    published: bool,
}

impl TemporaryOutput {
    fn new(target: &Path) -> std::io::Result<(Self, File)> {
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        let stem = target
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("image");
        let extension = target.extension().and_then(|value| value.to_str());

        for _ in 0..100 {
            let id = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut name = format!(".{stem}.rimage-{}-{id}", std::process::id());
            if let Some(extension) = extension {
                name.push('.');
                name.push_str(extension);
            }
            let path = parent.join(name);
            match File::create_new(&path) {
                Ok(file) => {
                    return Ok((
                        Self {
                            path,
                            published: false,
                        },
                        file,
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary output file",
        ))
    }

    #[cfg(not(windows))]
    fn publish(mut self, target: &Path) -> std::io::Result<()> {
        fs::rename(&self.path, target)?;
        self.published = true;
        Ok(())
    }

    #[cfg(windows)]
    fn publish(mut self, target: &Path) -> std::io::Result<()> {
        if !target.exists() {
            fs::rename(&self.path, target)?;
            self.published = true;
            return Ok(());
        }

        let (mut previous, previous_file) = Self::new(target)?;
        drop(previous_file);
        fs::remove_file(&previous.path)?;
        fs::rename(target, &previous.path)?;

        match fs::rename(&self.path, target) {
            Ok(()) => {
                self.published = true;
                previous.published = true;
                if let Err(error) = fs::remove_file(&previous.path) {
                    log::warn!(
                        "Failed to remove replaced output {}: {error}",
                        previous.path.display()
                    );
                }
                Ok(())
            }
            Err(publish_error) => match fs::rename(&previous.path, target) {
                Ok(()) => {
                    previous.published = true;
                    Err(publish_error)
                }
                Err(restore_error) => {
                    previous.published = true;
                    Err(std::io::Error::new(
                        publish_error.kind(),
                        format!(
                            "publish failed: {publish_error}; restoring the previous output also failed: {restore_error}; previous output remains at {}",
                            previous.path.display()
                        ),
                    ))
                }
            },
        }
    }
}

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn append_output_extension(path: &mut PathBuf, extension: &str) {
    if let Some(current) = path.extension() {
        let mut combined = current.to_os_string();
        combined.push(".");
        combined.push(extension);
        path.set_extension(combined);
    } else {
        path.set_extension(extension);
    }
}

fn output_path_key(path: &Path) -> String {
    let key = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

fn valid_suffix(suffix: &str) -> bool {
    !suffix.is_empty()
        && suffix != "."
        && suffix != ".."
        && !suffix.ends_with(['.', ' '])
        && !suffix.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
}

fn size_ratio(output_size: u64, input_size: u64) -> f64 {
    if input_size == 0 {
        0.0
    } else {
        output_size as f64 / input_size as f64
    }
}

fn space_saved(input_size: u64, output_size: u64) -> i64 {
    let difference = i128::from(input_size) - i128::from(output_size);
    difference.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Metadata {
    #[serde(rename = "inputSize")]
    input_size: u64,
    #[serde(rename = "outputSize")]
    output_size: u64,
    #[serde(rename = "totalImages")]
    total_images: usize,
    #[serde(rename = "compressionRatio")]
    compression_ratio: f64,
    #[serde(rename = "spaceSaved")]
    space_saved: i64,
    timestamp: u64,
    images: Vec<ImageMetadata>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ImageMetadata {
    // File paths
    input: PathBuf,
    output: PathBuf,

    // File information
    #[serde(rename = "inputSize")]
    input_size: u64,
    #[serde(rename = "outputSize")]
    output_size: u64,
    #[serde(rename = "compressionRatio")]
    compression_ratio: f64,
    #[serde(rename = "spaceSaved")]
    space_saved: i64,

    // Image properties
    width: u32,
    height: u32,
    #[serde(rename = "pixelCount")]
    pixel_count: u64,
    #[serde(rename = "aspectRatio")]
    aspect_ratio: f64,

    // zune-image specific properties
    #[serde(rename = "bitDepth")]
    bit_depth: String,
    #[serde(rename = "colorSpace")]
    color_space: String,
    #[serde(rename = "hasAlpha")]
    has_alpha: bool,
    #[serde(rename = "isAnimated")]
    is_animated: bool,
    #[serde(rename = "frameCount")]
    frame_count: usize,
    channels: usize,

    // Format information
    #[serde(rename = "inputFormat")]
    input_format: Option<String>,
    #[serde(rename = "outputFormat")]
    output_format: String,

    // Processing information
    #[serde(rename = "processedAt")]
    processed_at: u64,
    #[serde(rename = "processingTimeMs")]
    processing_time_ms: u128,

    // File timestamps
    #[serde(rename = "inputModified")]
    input_modified: Option<u64>,
    #[serde(rename = "outputCreated")]
    output_created: u64,
}

fn get_file_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_lowercase())
}

fn get_file_modified_time(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn bit_depth_to_string(depth: &BitDepth) -> String {
    match depth {
        BitDepth::Eight => "8-bit".to_string(),
        BitDepth::Sixteen => "16-bit".to_string(),
        BitDepth::Float32 => "32-bit float".to_string(),
        _ => "Unknown".to_string(),
    }
}

fn colorspace_to_string(colorspace: &ColorSpace) -> String {
    match colorspace {
        ColorSpace::RGB => "RGB".to_string(),
        ColorSpace::RGBA => "RGBA".to_string(),
        ColorSpace::Luma => "Grayscale".to_string(),
        ColorSpace::LumaA => "Grayscale with Alpha".to_string(),
        ColorSpace::YCbCr => "YCbCr".to_string(),
        ColorSpace::YCCK => "YCCK".to_string(),
        ColorSpace::CMYK => "CMYK".to_string(),
        ColorSpace::BGR => "BGR".to_string(),
        ColorSpace::BGRA => "BGRA".to_string(),
        ColorSpace::HSL => "HSL".to_string(),
        ColorSpace::HSV => "HSV".to_string(),
        _ => "Unknown".to_string(),
    }
}

/// Normalize a user-provided file path into its canonical absolute form.
///
/// Handles:
/// - Expanding `~` to the home directory
/// - Resolving relative paths (`.`, `..`) against the current directory
///   using component-level joining — avoids `Path::join` which leaves
///   `./` and `../` literals in the joined result
/// - Canonicalizing existing paths (resolves symlinks, case, remaining `..`)
/// - Preserving UNC/verbatim prefixes on Windows
fn normalize_path(path: &Path, current_dir: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        return path.to_path_buf();
    }

    // Expand ~ in the path string
    let path = expand_tilde_in_path(path);

    // Detect absolute paths (including Windows UNC/verbatim prefixes)
    let is_absolute = path.is_absolute()
        || path
            .components()
            .next()
            .is_some_and(|c| matches!(c, std::path::Component::Prefix(_)));

    let path = if is_absolute {
        path
    } else {
        join_normalized(current_dir, &path)
    };

    // Canonicalize if the path exists (resolves symlinks, remaining .., case).
    // Non-existent paths (output dirs) keep the component-normalized form.
    path.canonicalize().unwrap_or(path)
}

/// Join a base path with a relative path, normalizing `.` and `..` components.
///
/// Unlike `Path::join`, this resolves `./foo` to `base/foo` instead of
/// `base/./foo`, and correctly handles `../` by popping the parent.
fn join_normalized(base: &Path, relative: &Path) -> PathBuf {
    let mut result = base.to_path_buf();
    for c in relative.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// Expand a leading `~` in a path to the user's home directory.
///
/// On Unix: uses `HOME` env var.
/// On Windows: uses `USERPROFILE` env var.
/// Falls back to the original path if the env var is unset.
fn expand_tilde_in_path(path: &Path) -> PathBuf {
    let s = match path.to_str() {
        Some(s) => s,
        None => return path.to_path_buf(),
    };

    if !s.starts_with('~') {
        return path.to_path_buf();
    }

    #[cfg(windows)]
    let home_var = "USERPROFILE";
    #[cfg(not(windows))]
    let home_var = "HOME";

    let home_dir = match std::env::var(home_var) {
        Ok(h) => PathBuf::from(h),
        Err(_) => return path.to_path_buf(),
    };

    if s == "~" {
        return home_dir;
    }

    // "~/..." → home_dir/...
    let after_tilde = &s[1..]; // skip '~'
    let trimmed = after_tilde.trim_start_matches(['/', '\\']);
    if trimmed.len() < after_tilde.len() {
        join_normalized(&home_dir, Path::new(trimmed))
    } else {
        // "~something" is not a home directory reference
        path.to_path_buf()
    }
}

fn main() {
    let logger = pretty_env_logger::formatted_builder()
        .parse_default_env()
        .filter_module("little_exif", log::LevelFilter::Off)
        .build();
    let level = logger.filter();

    let multi = MultiProgress::new();
    let sty_main = ProgressStyle::with_template("{bar:40.green/yellow} {pos:>4}/{len:4}")
        .unwrap()
        .progress_chars("▬▬▬");
    let sty_aux_decode = ProgressStyle::with_template("{spinner:.blue} {msg}").unwrap();
    let sty_aux_operations = ProgressStyle::with_template("{spinner:.yellow} {msg}").unwrap();
    let sty_aux_encode = ProgressStyle::with_template("{spinner:.green} {msg}").unwrap();

    LogWrapper::new(multi.clone(), logger).try_init().unwrap();
    log::set_max_level(level);

    let current_dir = std::env::current_dir().unwrap_or_default();
    let matches = cli().get_matches_from(std::env::args());

    let state: Arc<Mutex<ProcessingState>> = Arc::new(Mutex::new(ProcessingState::new()));

    match matches.subcommand() {
        Some((subcommand, matches)) => {
            let threads = matches.get_one::<u8>("threads").copied().unwrap_or(1) as usize;
            let thread_pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                Ok(pool) => pool,
                Err(error) => {
                    log::error!("Failed to create image worker pool: {error}");
                    return;
                }
            };

            // Normalize file paths: resolve ~, relative paths, canonicalize
            let files: Vec<PathBuf> = matches
                .get_many::<PathBuf>("files")
                .expect("`files` is required")
                .map(|p| normalize_path(p, &current_dir))
                .collect();
            let files = collect_files(&files);
            log::debug!("Resolved files: {files:#?}");

            let file_count = files.iter().filter(|f| f.is_file()).count() as u64;

            if file_count == 0 {
                log::error!("No input files found. Check the file paths.");
                if log::log_enabled!(log::Level::Debug) {
                    log::debug!("Resolved files: {files:#?}");
                }
                return;
            }

            let out_dir = matches
                .get_one::<PathBuf>("directory")
                .map(|p| normalize_path(p, &current_dir));

            let recursive = matches.get_flag("recursive");
            let backup = matches.get_flag("backup");
            let strip_metadata = matches.get_flag("strip");
            let quiet = matches.get_flag("quiet");
            let no_progress = matches.get_flag("no-progress");
            let output_metadata = matches.contains_id("metadata");
            let metadata_path = matches
                .get_one::<PathBuf>("metadata")
                .map(|p| normalize_path(p, &current_dir))
                .unwrap_or(PathBuf::from("metadata.json"));

            let suffix = matches.get_one::<String>("suffix").cloned();
            if suffix
                .as_deref()
                .is_some_and(|suffix| !valid_suffix(suffix))
            {
                log::error!("Suffix must be a valid single filename component");
                return;
            }

            if quiet || no_progress {
                multi.set_draw_target(ProgressDrawTarget::hidden());
            }

            let pb_main = multi.add(ProgressBar::new(file_count));
            pb_main.set_style(sty_main);
            if file_count <= 1 {
                pb_main.set_draw_target(ProgressDrawTarget::hidden());
            }

            let output_extension = match encoder(subcommand, matches) {
                Ok(encoder) => encoder.to_extension(),
                Err(error) => {
                    log::error!("Failed to initialize encoder: {error}");
                    return;
                }
            };
            let paths = get_paths(files, out_dir, suffix, recursive)
                .map(|(input, mut output)| {
                    append_output_extension(&mut output, output_extension);
                    (input, output)
                })
                .collect::<Vec<_>>();

            let input_paths = paths
                .iter()
                .map(|(input, _)| output_path_key(input))
                .collect::<HashSet<_>>();
            let mut output_paths = HashSet::with_capacity(paths.len());
            for (input, output) in &paths {
                let input_key = output_path_key(input);
                let output_key = output_path_key(output);
                if !output_paths.insert(output_key.clone()) {
                    log::error!(
                        "Multiple input files resolve to the same output path: {}",
                        output.display()
                    );
                    return;
                }
                if input_key != output_key && input_paths.contains(&output_key) {
                    log::error!(
                        "Output path would overwrite another input file: {}",
                        output.display()
                    );
                    return;
                }
            }
            if output_metadata {
                let metadata_key = output_path_key(&metadata_path);
                if input_paths.contains(&metadata_key) || output_paths.contains(&metadata_key) {
                    log::error!(
                        "Metadata path conflicts with an input or output image: {}",
                        metadata_path.display()
                    );
                    return;
                }
            }

            thread_pool.install(|| {
                for batch in paths.chunks(threads) {
                    rayon::scope(|s| {
                        for (input, output) in batch.iter().cloned() {
                    let pb_main = pb_main.clone();
                    let multi = multi.clone();
                    let sty_aux_decode = sty_aux_decode.clone();
                    let sty_aux_operations = sty_aux_operations.clone();
                    let sty_aux_encode = sty_aux_encode.clone();
                    let state = Arc::clone(&state);
                    let current_dir = current_dir.clone();
                    s.spawn(move |_| {
                        let image_start_time = std::time::Instant::now();

                        let pb = multi.add(ProgressBar::new_spinner());
                        pb.set_style(sty_aux_decode.clone());
                        pb.set_message(format!("{}", input.display()));
                        pb.enable_steady_tick(Duration::from_millis(100));

                        // Advance progress bars on all exit paths (including early
                        // returns from handle_error!).
                        let _finish = FinishGuard {
                            pb: pb.clone(),
                            pb_main: pb_main.clone(),
                        };

                        let mut ops: Vec<Box<dyn OperationsTrait>> = Vec::new();

                        let input_size = handle_error!(input, input.metadata()).len();
                        let input_format = get_file_extension(&input);
                        let input_modified = get_file_modified_time(&input);

                        let mut img = handle_error!(input, decode(&input));
                        let exif_metadata: Option<ExifMetadata> = ExifMetadata::new_from_path(&input)
                            .ok()
                            .filter(|_| {
                                !strip_metadata && SUPPORTS_EXIF.contains(&subcommand)
                            });

                        pb.set_style(sty_aux_operations.clone());

                        // Extract zune-image properties
                        let (w, h) = img.dimensions();
                        let pixel_count = (w as u64) * (h as u64);
                        let aspect_ratio = w as f64 / h as f64;
                        let colorspace = img.colorspace();
                        let is_animated = img.is_animated();
                        let frame_count = img.frames_len();
                        let has_alpha = colorspace.has_alpha();
                        let channels = colorspace.num_components();

                        let original_bit_depth = img.depth();

                        let mut available_encoder =
                            handle_error!(input, encoder(subcommand, matches));
                        let output_format = available_encoder.to_extension().to_string();

                        if strip_metadata || !SUPPORTS_ICC.contains(&subcommand) {
                            ops.push(Box::new(ApplySRGB));
                        }

                        if strip_metadata || !SUPPORTS_EXIF.contains(&subcommand) {
                            ops.push(Box::new(AutoOrient));
                        }

                        operations(matches, &img)
                            .into_iter()
                            .for_each(|(_, operations)| match operations.name() {
                                "quantize" => {
                                    ops.push(Box::new(Depth::new(BitDepth::Eight)));
                                    ops.push(Box::new(ColorspaceConv::new(ColorSpace::RGBA)));
                                    ops.push(operations);
                                }
                                _ => {
                                    ops.push(operations);
                                }
                            });

                        for op in ops {
                            handle_error!(input, op.execute(&mut img));
                        }

                        pb.set_style(sty_aux_encode.clone());

                        if let Some(parent) = output.parent() {
                            handle_error!(output, fs::create_dir_all(parent));
                        }
                        let (temporary, output_file) =
                            handle_error!(output, TemporaryOutput::new(&output));

                        handle_error!(
                            temporary.path,
                            available_encoder.encode(&img, output_file)
                        );

                        if let Some(actual_metadata) = exif_metadata {
                            handle_error!(
                                temporary.path,
                                actual_metadata.write_to_file(&temporary.path)
                            );
                        }

                        if backup {
                            let backup_name = format!(
                                "{}@backup.{}",
                                input
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("backup"),
                                input.extension().and_then(|s| s.to_str()).unwrap_or("bak"),
                            );
                            let backup_path = input.with_file_name(&backup_name);
                            handle_error!(input, fs::hard_link(&input, &backup_path));
                            if let Err(error) = temporary.publish(&output) {
                                if let Err(cleanup_error) = fs::remove_file(&backup_path) {
                                    log::error!(
                                        "{}: publish failed ({error}); removing the new backup also failed: {cleanup_error}",
                                        output.display()
                                    );
                                } else {
                                    log::error!("{}: {error}", output.display());
                                }
                                return;
                            }
                            if output_path_key(&input) != output_path_key(&output)
                                && let Err(error) = fs::remove_file(&input)
                            {
                                log::error!(
                                    "{}: output was published and backup created, but the original input could not be removed: {error}",
                                    input.display()
                                );
                            }
                        } else {
                            handle_error!(output, temporary.publish(&output));
                        }

                        let output_size = handle_error!(output, output.metadata()).len();
                        let processing_time = image_start_time.elapsed().as_millis();
                        let compression_ratio = size_ratio(output_size, input_size);
                        let space_saved = space_saved(input_size, output_size);
                        let processed_at = get_current_timestamp();
                        let output_created = get_current_timestamp();

                        let mut state = state.lock().unwrap();

                        let absolute_input_path = normalize_path(&input, &current_dir);
                        let absolute_output_path = normalize_path(&output, &current_dir);

                        state.results.push(Result {
                            output: output.to_path_buf(),
                            input_size,
                            output_size,
                        });

                        let metadata = state.metadata.get_or_insert(Metadata {
                            input_size: 0,
                            output_size: 0,
                            total_images: 0,
                            compression_ratio: 0.0,
                            space_saved: 0,
                            timestamp: get_current_timestamp(),
                            images: vec![],
                        });

                        metadata.input_size = metadata.input_size.saturating_add(input_size);
                        metadata.output_size = metadata.output_size.saturating_add(output_size);
                        metadata.total_images = metadata.total_images.saturating_add(1);
                        metadata.space_saved = metadata.space_saved.saturating_add(space_saved);

                        metadata.images.push(ImageMetadata {
                            input: absolute_input_path,
                            output: absolute_output_path,
                            input_size,
                            output_size,
                            compression_ratio,
                            space_saved,
                            width: w as u32,
                            height: h as u32,
                            pixel_count,
                            aspect_ratio,
                            bit_depth: bit_depth_to_string(&original_bit_depth),
                            color_space: colorspace_to_string(&colorspace),
                            has_alpha,
                            is_animated,
                            frame_count,
                            channels,
                            input_format,
                            output_format,
                            processed_at,
                            processing_time_ms: processing_time,
                            input_modified,
                            output_created,
                        });
                            });
                        }
                    });
                }
            });

            let mut state = state.lock().unwrap();

            // Update final metadata calculations
            if let Some(ref mut meta) = state.metadata.as_mut() {
                meta.compression_ratio = if meta.input_size > 0 {
                    meta.output_size as f64 / meta.input_size as f64
                } else {
                    0.0
                };
            }

            state
                .results
                .sort_by_key(|b| std::cmp::Reverse(b.output_size));

            let path_width = state
                .results
                .iter()
                .map(|r| r.output.display().to_string().len())
                .max()
                .unwrap_or(0);

            if !quiet {
                let term = Term::stdout();

                if state.results.len() > 1 {
                    term.write_line(&format!(
                        "{:<path_width$} {}",
                        style("File").bold(),
                        style("Size").bold(),
                    ))
                    .unwrap();

                    for result in state.results.iter() {
                        let difference = size_ratio(result.output_size, result.input_size) * 100.0;

                        term.write_line(&format!(
                            "{:<path_width$} {} > {} {}",
                            result.output.display(),
                            style(DecimalBytes(result.input_size)).blue(),
                            style(DecimalBytes(result.output_size)).blue(),
                            if difference > 100.0 {
                                style(format!("{:.2}%", difference - 100.0)).red()
                            } else {
                                style(format!("{:.2}%", difference - 100.0)).green()
                            },
                        ))
                        .unwrap();
                    }
                }

                let total_input_size = state.results.iter().fold(0u64, |total, result| {
                    total.saturating_add(result.input_size)
                });
                let total_output_size = state.results.iter().fold(0u64, |total, result| {
                    total.saturating_add(result.output_size)
                });

                if !state.results.is_empty() {
                    let difference = size_ratio(total_output_size, total_input_size) * 100.0;

                    if let Err(error) = term.write_line(&format!(
                        "Total: {} > {} {}",
                        style(DecimalBytes(total_input_size)).blue(),
                        style(DecimalBytes(total_output_size)).blue(),
                        if difference > 100.0 {
                            style(format!("{:.2}%", difference - 100.0)).red()
                        } else {
                            style(format!("{:.2}%", difference - 100.0)).green()
                        },
                    )) {
                        log::error!("Failed to write output summary: {error}");
                    }
                }
            }

            let rust_log_hint = if cfg!(windows) {
                r#"$env:RUST_LOG="debug""#
            } else {
                "RUST_LOG=debug"
            };
            let succeeded = state.results.len() as u64;
            if succeeded < file_count {
                log::error!(
                    "{}/{} file(s) failed. Run with `{}` for details.",
                    file_count - succeeded,
                    file_count,
                    rust_log_hint
                );
            }

            if output_metadata && let Some(metadata) = state.metadata.as_ref() {
                match serde_json::to_string_pretty(metadata) {
                    Ok(json) => {
                        if let Some(parent) = metadata_path
                            .parent()
                            .filter(|parent| !parent.as_os_str().is_empty())
                            && let Err(error) = fs::create_dir_all(parent)
                        {
                            log::error!(
                                "Failed to create metadata directory {}: {error}",
                                parent.display()
                            );
                            return;
                        }
                        match TemporaryOutput::new(&metadata_path) {
                            Ok((temporary, mut file)) => {
                                if let Err(error) = file
                                    .write_all(json.as_bytes())
                                    .and_then(|_| file.flush())
                                    .and_then(|_| temporary.publish(&metadata_path))
                                {
                                    log::error!(
                                        "Failed to write metadata {}: {error}",
                                        metadata_path.display()
                                    );
                                }
                            }
                            Err(error) => log::error!(
                                "Failed to create metadata output {}: {error}",
                                metadata_path.display()
                            ),
                        }
                    }
                    Err(error) => log::error!("Failed to serialize metadata: {error}"),
                }
            }
        }
        None => unreachable!("clap ensures a subcommand is always provided"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_base() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"D:\projects\rimage")
        } else {
            PathBuf::from("/projects/rimage")
        }
    }

    fn test_base_src() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"D:\projects\rimage\src")
        } else {
            PathBuf::from("/projects/rimage/src")
        }
    }

    #[test]
    fn join_normalized_strips_curdir() {
        let base = test_base();
        let rel = Path::new("./1.jpg");
        let result = join_normalized(&base, rel);
        assert_eq!(result, base.join("1.jpg"));
    }

    #[test]
    fn join_normalized_resolves_parentdir() {
        let base = test_base_src();
        let rel = Path::new("../tests/1.jpg");
        let result = join_normalized(&base, rel);
        let expected = base.parent().unwrap().join("tests/1.jpg");
        assert_eq!(result, expected);
    }

    #[test]
    fn join_normalized_handles_deep_path() {
        let base = test_base();
        let rel = Path::new("subdir/./other/../1.jpg");
        let result = join_normalized(&base, rel);
        let expected = base.join("subdir").join("1.jpg");
        assert_eq!(result, expected);
    }
}
