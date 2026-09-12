use std::{num::NonZeroUsize, thread};

use clap::ArgMatches;

/// Parallelism the machine reports: the most concurrency that can ever pay off.
///
/// Used both as the `--threads` ceiling and as the value an over-large request
/// falls back to.
pub fn num_threads() -> usize {
    thread::available_parallelism()
        .unwrap_or(NonZeroUsize::new(4).unwrap())
        .get()
}

/// Concurrency asked for through `-t/--threads`, before any clamping.
///
/// Defaults to 1, matching the flag's own default. A `0` is raised to 1 here
/// anyway: this number is a divisor for the memory budget, and a zero divisor
/// would either panic or silently mean "unlimited".
///
/// Pair it with [`clamp`]; every caller wants the clamped value, and reading
/// the raw one is only useful for telling the user what was reduced.
pub fn requested(matches: &ArgMatches) -> usize {
    matches
        .get_one::<u16>("threads")
        .copied()
        .map(|threads| threads as usize)
        .unwrap_or(1)
        .max(1)
}

/// Reduce a requested concurrency to what this machine can actually run.
///
/// Asking for more workers than there are CPUs does not buy parallelism, but
/// it does shrink the memory budget: every concurrent image is assumed to hold
/// its own set of buffers, so the per-image ceiling is the available memory
/// divided by this number. Without the clamp, a request like `-t 64` on an
/// 8-core machine both wastes the budget and rejects images that a smaller
/// `-t` would have accepted — with an error message that blames the image.
///
/// Clamping rather than rejecting keeps a fixed script portable: the same
/// command line runs on a 4-core laptop and a 64-core workstation, taking the
/// parallelism each one can offer instead of failing on the smaller machine.
pub fn clamp(requested: usize) -> usize {
    requested.clamp(1, num_threads())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The machine's own parallelism is accepted unchanged, so a request the
    /// host can actually honour is never silently reduced.
    #[test]
    fn the_machines_own_parallelism_survives() {
        assert_eq!(clamp(num_threads()), num_threads());
    }

    /// Anything above it collapses onto it. This is the whole point of the
    /// clamp: an oversized `-t` must not divide the memory budget further.
    #[test]
    fn an_over_large_request_falls_back_to_the_machine() {
        assert_eq!(clamp(num_threads() + 1), num_threads());
        assert_eq!(clamp(usize::MAX), num_threads());
    }

    /// The lower bound holds even though the parser rejects `0` already: this
    /// number divides the memory budget, and a zero divisor is not a value the
    /// rest of the pipeline should have to defend against.
    #[test]
    fn the_request_is_never_below_one() {
        assert_eq!(clamp(0), 1);
        assert_eq!(clamp(1), 1);
    }
}
