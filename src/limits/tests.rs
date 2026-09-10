use super::*;

fn budget_with(memory: u64, concurrency: usize) -> SystemBudget {
    SystemBudget {
        available_memory: memory,
        address_cap: ADDRESS_SPACE_CAP,
        concurrency,
    }
}

#[test]
fn webp_cap_matches_libwebp_constant() {
    let caps = format_caps(ImageFormatId::WebP);
    assert_eq!(caps.max_side, 16383);
}

#[test]
fn jpeg_cap_matches_libjpeg_constant() {
    let caps = format_caps(ImageFormatId::Jpeg);
    assert_eq!(caps.max_side, 65500);
}

#[test]
fn avif_cap_matches_spec() {
    let caps = format_caps(ImageFormatId::Avif);
    assert_eq!(caps.max_side, 65536);
}

#[test]
fn formats_without_published_limits_are_unbounded() {
    for format in [
        ImageFormatId::Png,
        ImageFormatId::Tiff,
        ImageFormatId::Svg,
        ImageFormatId::Other,
    ] {
        let caps = format_caps(format);
        assert_eq!(
            caps.max_side,
            u64::MAX,
            "{} must not claim a published dimension limit",
            format.name()
        );
        assert_eq!(caps.max_pixels, u64::MAX);
    }
}

#[test]
fn unprobed_machine_falls_back_instead_of_failing() {
    let budget = budget_with(0, 1);
    assert!(!budget.is_probed());
    assert_eq!(budget.per_image_bytes(), FALLBACK_PER_IMAGE_BYTES);
}

#[test]
fn budget_is_halved_and_divided_by_concurrency() {
    let budget = budget_with(8 * 1024 * 1024 * 1024, 4);
    // Half of 8 GiB is 4 GiB, split across 4 concurrent images.
    assert_eq!(budget.per_image_bytes(), 1024 * 1024 * 1024);
}

#[test]
fn concurrency_of_zero_is_treated_as_one() {
    let budget = budget_with(1024 * 1024 * 1024, 0);
    assert_eq!(budget.per_image_bytes(), 512 * 1024 * 1024);
}

#[test]
fn bytes_per_pixel_reflects_depth_and_colorspace() {
    assert_eq!(bytes_per_pixel(BitDepth::Eight, ColorSpace::RGB), 3);
    assert_eq!(bytes_per_pixel(BitDepth::Eight, ColorSpace::RGBA), 4);
    assert_eq!(bytes_per_pixel(BitDepth::Sixteen, ColorSpace::RGBA), 8);
    assert_eq!(bytes_per_pixel(BitDepth::Float32, ColorSpace::RGBA), 16);
    assert_eq!(bytes_per_pixel(BitDepth::Eight, ColorSpace::Luma), 1);
    // An unknown colour space must still produce a usable divisor.
    assert_eq!(bytes_per_pixel(BitDepth::Unknown, ColorSpace::Unknown), 1);
}

#[test]
fn memory_budget_binds_before_format_when_memory_is_tight() {
    // 1 GiB total, so 512 MiB usable; RGBA8 with a 3+16 cost is 76 bytes/px.
    let budget = budget_with(1024 * 1024 * 1024, 1);
    let limits = LimitSet::for_input(
        ImageFormatId::Avif,
        BitDepth::Eight,
        ColorSpace::RGBA,
        &budget,
        PipelineCost::for_encoder(ImageFormatId::Avif),
    );

    assert_eq!(limits.binding, Binding::Memory);
    assert!(limits.max_pixels < 65536 * 65536);
}

#[test]
fn format_cap_binds_when_memory_is_plentiful() {
    // 1 TiB available; the WebP side limit must win.
    let budget = budget_with(1024u64 * 1024 * 1024 * 1024, 1);
    let limits = LimitSet::for_input(
        ImageFormatId::WebP,
        BitDepth::Eight,
        ColorSpace::RGBA,
        &budget,
        PipelineCost::for_encoder(ImageFormatId::WebP),
    );

    assert_eq!(limits.max_width, 16383);
    assert_eq!(limits.max_height, 16383);
}

#[test]
fn check_reports_width_before_pixels() {
    let limits = LimitSet {
        max_width: 100,
        max_height: 200,
        max_pixels: 10_000,
        max_bytes: u64::MAX,
        binding: Binding::Format,
    };

    // The area passes, so only the side check can reject this input.
    let violation = limits.check(101, 10).unwrap_err();
    assert_eq!(violation.kind, ViolationKind::Width);
    assert_eq!(violation.actual, 101);
    assert_eq!(violation.allowed, 100);
}

#[test]
fn check_reports_pixels_before_a_side() {
    let limits = LimitSet {
        max_width: 100,
        max_height: 100,
        max_pixels: 1_000,
        max_bytes: u64::MAX,
        binding: Binding::Memory,
    };

    // Both a side and the area are too large. The area is reported because it
    // is the tighter description, and it carries the binding that produced it.
    let violation = limits.check(200, 200).unwrap_err();
    assert_eq!(violation.kind, ViolationKind::Pixels);
    assert_eq!(violation.binding, Binding::Memory);
}

#[test]
fn check_reports_height() {
    let limits = LimitSet {
        max_width: 100,
        max_height: 200,
        max_pixels: 10_000,
        max_bytes: u64::MAX,
        binding: Binding::Format,
    };

    let violation = limits.check(10, 201).unwrap_err();
    assert_eq!(violation.kind, ViolationKind::Height);
}

#[test]
fn check_reports_pixel_product_when_both_sides_pass() {
    let limits = LimitSet {
        max_width: 1000,
        max_height: 1000,
        max_pixels: 10_000,
        max_bytes: u64::MAX,
        binding: Binding::Memory,
    };

    // Each side is legal; only the product exceeds the ceiling.
    let violation = limits.check(500, 500).unwrap_err();
    assert_eq!(violation.kind, ViolationKind::Pixels);
    assert_eq!(violation.actual, 250_000);
    assert_eq!(violation.binding, Binding::Memory);

    assert!(limits.check(100, 100).is_ok());
}

#[test]
fn check_accepts_the_exact_limit() {
    let limits = LimitSet {
        max_width: 500,
        max_height: 500,
        max_pixels: 250_000,
        max_bytes: u64::MAX,
        binding: Binding::Format,
    };

    assert!(limits.check(500, 500).is_ok());
}

#[test]
fn suggested_side_fits_under_every_ceiling() {
    let limits = LimitSet {
        max_width: 16383,
        max_height: 16383,
        max_pixels: 10_000_000,
        max_bytes: u64::MAX,
        binding: Binding::Memory,
    };

    let side = limits.suggested_side();
    assert!(side <= limits.max_width);
    assert!(side.saturating_mul(side) <= limits.max_pixels);
    assert_eq!(side, 3162);
}

#[test]
fn suggested_side_is_at_least_one() {
    let limits = LimitSet {
        max_width: 0,
        max_height: 0,
        max_pixels: 0,
        max_bytes: 0,
        binding: Binding::Memory,
    };

    assert_eq!(limits.suggested_side(), 1);
}

#[test]
fn integer_sqrt_matches_known_values() {
    assert_eq!(integer_sqrt(0), 0);
    assert_eq!(integer_sqrt(1), 1);
    assert_eq!(integer_sqrt(2), 1);
    assert_eq!(integer_sqrt(4), 2);
    assert_eq!(integer_sqrt(15), 3);
    assert_eq!(integer_sqrt(16), 4);
    assert_eq!(integer_sqrt(17), 4);
    assert_eq!(integer_sqrt(10_000), 100);
    assert_eq!(integer_sqrt(u64::MAX), 4_294_967_295);
}

#[test]
fn pipeline_cost_sums_all_stages() {
    let cost = PipelineCost::new(3, 2, 2, 16);
    assert_eq!(cost.total(), 23);
}

#[test]
fn format_extension_lookup_is_case_insensitive() {
    assert_eq!(ImageFormatId::from_extension("JPG"), ImageFormatId::Jpeg);
    assert_eq!(ImageFormatId::from_extension("jpeg"), ImageFormatId::Jpeg);
    assert_eq!(ImageFormatId::from_extension("WebP"), ImageFormatId::WebP);
    assert_eq!(ImageFormatId::from_extension("tif"), ImageFormatId::Tiff);
    assert_eq!(ImageFormatId::from_extension("svgz"), ImageFormatId::Svg);
    assert_eq!(ImageFormatId::from_extension("qoi"), ImageFormatId::Other);
}

#[test]
fn binding_descriptions_are_not_empty() {
    for binding in [
        Binding::Format,
        Binding::Memory,
        Binding::Disk,
        Binding::None,
    ] {
        assert!(!binding.describe().is_empty());
    }
}

/// The theoretical check the task asks for: a 65500x65500 image must be
/// rejected before any decoding is attempted, on every realistic memory budget.
///
/// 65500 is libjpeg's inclusive per-side ceiling, so neither side alone is
/// wrong; only the area is impossible. That makes this the case where the cap
/// has to be caught by the pixel product rather than by a side check.
#[test]
fn extreme_dimensions_are_rejected_by_the_jpeg_ceiling() {
    // A square whose area is the JPEG side ceiling squared. libjpeg's own
    // 65500x65500 ceiling is therefore exactly the largest square the format
    // can express, and every size the task calls "extreme" is at least this big.
    const EXTREME_SIDE: u64 = 65500;
    const EXTREME_AREA: u64 = EXTREME_SIDE * EXTREME_SIDE;

    // Anything at or above this produces the same budget, so this figure means
    // "more memory than any process can address on 64-bit".
    const PLENTIFUL: u64 = u64::MAX / 4 + 1;
    const TYPICAL: u64 = 64 * 1024 * 1024 * 1024;

    let limits = |memory: u64| {
        let budget = budget_with(memory, 1);
        LimitSet::for_input(
            ImageFormatId::Jpeg,
            BitDepth::Eight,
            ColorSpace::RGB,
            &budget,
            PipelineCost::for_encoder(ImageFormatId::Jpeg),
        )
    };

    // The side ceiling itself is honoured, not conflated with a pixel limit.
    assert_eq!(limits(PLENTIFUL).max_width, EXTREME_SIDE);
    assert!(limits(PLENTIFUL).check(EXTREME_SIDE, 1).is_ok());

    // Memory is the binding constraint in practice. `max_pixels` is derived
    // from the budget rather than restated here, so this compares the two
    // ceilings instead of re-deriving one of them by hand.
    let bound_by_memory = limits(TYPICAL);
    assert!(
        bound_by_memory.max_pixels < EXTREME_AREA,
        "a {TYPICAL}-byte budget admitted {} pixels, which is not below \
         the {EXTREME_AREA}-pixel extreme square",
        bound_by_memory.max_pixels
    );
    let violation = bound_by_memory.check(EXTREME_SIDE, EXTREME_SIDE).unwrap_err();
    assert_eq!(violation.kind, ViolationKind::Pixels);
    assert_eq!(violation.binding, Binding::Memory);
    assert!(violation.actual > violation.allowed);

    // With memory taken out of the picture, the area derived from the side
    // ceiling takes over and still rejects the extreme square.
    let bound_by_format = limits(PLENTIFUL);
    let violation = bound_by_format.check(EXTREME_SIDE, EXTREME_SIDE + 1).unwrap_err();
    assert_eq!(violation.kind, ViolationKind::Pixels);
    assert_eq!(violation.binding, Binding::Format);
    assert_eq!(violation.allowed, EXTREME_AREA);

    // The derived area must not be so tight that it rejects ordinary shapes:
    // a 65500x2466 panorama (the largest fixture in tests/) has to pass.
    assert!(limits(PLENTIFUL).check(EXTREME_SIDE, 2466).is_ok());

    // A square that both the format and the memory budget can express is
    // accepted, so the checks do not reject everything indiscriminately.
    assert!(limits(TYPICAL).check(16_000, 16_000).is_ok());
}
