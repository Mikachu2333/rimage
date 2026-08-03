use crate::test_utils::*;

use super::*;

#[test]
fn apply_icc_profile() {
    let mut image = create_test_image_u8(100, 100, ColorSpace::RGB);

    let src_profile = Profile::new_srgb().icc().unwrap();
    image.metadata_mut().set_icc_chunk(src_profile);

    let target_profile = Profile::new_file("tests/files/icc/tinysrgb.icc").unwrap();

    let icc = target_profile.icc().unwrap();

    // Apply ICC profile
    let apply_icc = ApplyICC::new(target_profile);
    apply_icc.execute_impl(&mut image).unwrap();

    // Assert ICC profile is set correctly
    let stored = image.metadata().icc_chunk().unwrap();
    assert_eq!(stored.as_slice(), icc.as_slice());
}

#[test]
fn skip_icc_profile() {
    let mut image = create_test_image_u8(100, 100, ColorSpace::RGB);

    let apply_icc = ApplySRGB;
    let result = apply_icc.execute_impl(&mut image);

    assert!(result.is_ok());
    assert_eq!(image.metadata().icc_chunk(), None);
}

#[test]
fn apply_srgb_profile() {
    let mut image = create_test_image_u8(100, 100, ColorSpace::RGB);

    let src_profile = Profile::new_file("tests/files/icc/tinysrgb.icc")
        .unwrap()
        .icc()
        .unwrap();
    image.metadata_mut().set_icc_chunk(src_profile);

    let icc = Profile::new_srgb().icc().unwrap();

    // Apply ICC profile
    let apply_icc = ApplySRGB;
    apply_icc.execute_impl(&mut image).unwrap();

    // Assert ICC profile is set correctly
    assert_eq!(*image.metadata().icc_chunk().unwrap(), icc);
}

#[test]
fn apply_icc_profile_to_u16_without_panicking() {
    let mut image = create_test_image_u16(8, 8, ColorSpace::RGB);
    image
        .metadata_mut()
        .set_icc_chunk(Profile::new_srgb().icc().unwrap());

    let result = ApplyICC::new(Profile::new_srgb()).execute(&mut image);

    assert!(result.is_ok());
    assert_eq!(image.depth().bit_type(), BitType::U16);
    assert_eq!(image.dimensions(), (8, 8));
}

#[test]
fn converts_gray_profile_pixels_to_srgb() {
    let mut image = create_test_image_u8(8, 8, ColorSpace::Luma);
    let white_point = white_point_from_temp(6504.0).unwrap();
    let curve = ToneCurve::new(2.2);
    let gray_profile = Profile::new_gray(&white_point, &curve).unwrap();
    image
        .metadata_mut()
        .set_icc_chunk(gray_profile.icc().unwrap());

    ApplySRGB.execute(&mut image).unwrap();

    assert_eq!(image.colorspace(), ColorSpace::RGB);
    assert_eq!(image.channels_ref(true).len(), 3);
}

#[test]
fn preserves_alpha_when_converting_to_srgb() {
    let mut image = create_test_image_u8(8, 8, ColorSpace::RGBA);
    image
        .metadata_mut()
        .set_icc_chunk(Profile::new_srgb().icc().unwrap());
    let alpha_before = image.frames_ref()[0].flatten::<u8>()[3..]
        .iter()
        .step_by(4)
        .copied()
        .collect::<Vec<_>>();

    ApplySRGB.execute(&mut image).unwrap();

    let alpha_after = image.frames_ref()[0].flatten::<u8>()[3..]
        .iter()
        .step_by(4)
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(image.colorspace(), ColorSpace::RGBA);
    assert_eq!(alpha_after, alpha_before);
}

#[test]
fn rejects_profile_and_pixel_layout_mismatch() {
    let mut image = create_test_image_u8(8, 8, ColorSpace::CMYK);
    image
        .metadata_mut()
        .set_icc_chunk(Profile::new_srgb().icc().unwrap());

    let result = ApplySRGB.execute(&mut image);

    assert!(result.is_err());
}
