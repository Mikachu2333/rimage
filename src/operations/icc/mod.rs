use lcms2::*;
use std::sync::Mutex;
use zune_core::{bit_depth::BitType, colorspace::ColorSpace};
use zune_image::{
    errors::{ImageErrors, ImageOperationsErrors},
    frame::Frame,
    image::Image,
    traits::OperationsTrait,
};

/// Apply an ICC profile to an image.
pub struct ApplyICC {
    profile: Mutex<Profile<GlobalContext>>,
}

impl ApplyICC {
    /// Create a new ICC profile application operation.
    ///
    /// # Arguments
    /// - `profile`: Destination ICC profile.
    #[must_use]
    pub fn new(profile: Profile<GlobalContext>) -> Self {
        Self {
            profile: Mutex::new(profile),
        }
    }
}

fn profile_signature_for_colorspace(colorspace: ColorSpace) -> Option<ColorSpaceSignature> {
    match colorspace {
        ColorSpace::RGB
        | ColorSpace::RGBA
        | ColorSpace::BGR
        | ColorSpace::BGRA
        | ColorSpace::ARGB => Some(ColorSpaceSignature::RgbData),
        ColorSpace::Luma | ColorSpace::LumaA => Some(ColorSpaceSignature::GrayData),
        ColorSpace::CMYK => Some(ColorSpaceSignature::CmykData),
        ColorSpace::YCbCr => Some(ColorSpaceSignature::YCbCrData),
        ColorSpace::HSV => Some(ColorSpaceSignature::HsvData),
        _ => None,
    }
}

fn destination_colorspace(
    signature: ColorSpaceSignature,
    preserve_alpha: bool,
) -> Option<ColorSpace> {
    match signature {
        ColorSpaceSignature::RgbData => Some(if preserve_alpha {
            ColorSpace::RGBA
        } else {
            ColorSpace::RGB
        }),
        ColorSpaceSignature::GrayData => Some(if preserve_alpha {
            ColorSpace::LumaA
        } else {
            ColorSpace::Luma
        }),
        ColorSpaceSignature::CmykData => Some(ColorSpace::CMYK),
        ColorSpaceSignature::YCbCrData => Some(ColorSpace::YCbCr),
        ColorSpaceSignature::HsvData => Some(ColorSpace::HSV),
        _ => None,
    }
}

fn pixel_format(colorspace: ColorSpace, bit_size: usize) -> Option<PixelFormat> {
    match (colorspace, bit_size) {
        (ColorSpace::RGB, 8) => Some(PixelFormat::RGB_8),
        (ColorSpace::RGB, 16) => Some(PixelFormat::RGB_16),
        (ColorSpace::RGBA, 8) => Some(PixelFormat::RGBA_8),
        (ColorSpace::RGBA, 16) => Some(PixelFormat::RGBA_16),
        (ColorSpace::BGR, 8) => Some(PixelFormat::BGR_8),
        (ColorSpace::BGR, 16) => Some(PixelFormat::BGR_16),
        (ColorSpace::BGRA, 8) => Some(PixelFormat::BGRA_8),
        (ColorSpace::BGRA, 16) => Some(PixelFormat::BGRA_16),
        (ColorSpace::ARGB, 8) => Some(PixelFormat::ARGB_8),
        (ColorSpace::ARGB, 16) => Some(PixelFormat::ARGB_16),
        (ColorSpace::Luma, 8) => Some(PixelFormat::GRAY_8),
        (ColorSpace::Luma, 16) => Some(PixelFormat::GRAY_16),
        (ColorSpace::LumaA, 8) => Some(PixelFormat::GRAYA_8),
        (ColorSpace::LumaA, 16) => Some(PixelFormat::GRAYA_16),
        (ColorSpace::CMYK, 8) => Some(PixelFormat::CMYK_8),
        (ColorSpace::CMYK, 16) => Some(PixelFormat::CMYK_16),
        (ColorSpace::YCbCr, 8) => Some(PixelFormat::YCbCr_8),
        (ColorSpace::YCbCr, 16) => Some(PixelFormat::YCbCr_16),
        (ColorSpace::HSV, 8) => Some(PixelFormat::HSV_8),
        (ColorSpace::HSV, 16) => Some(PixelFormat::HSV_16),
        _ => None,
    }
}

fn operation_error(message: impl Into<String>) -> ImageErrors {
    ImageErrors::OperationsError(ImageOperationsErrors::GenericString(message.into()))
}

impl OperationsTrait for ApplyICC {
    fn name(&self) -> &'static str {
        "apply icc profile"
    }

    fn execute_impl(&self, image: &mut Image) -> Result<(), ImageErrors> {
        let src_profile = match image.metadata().icc_chunk() {
            Some(icc) => Profile::new_icc(icc)
                .map_err(|error| ImageOperationsErrors::GenericString(error.to_string()))?,
            None => Profile::new_srgb(),
        };

        let source_colorspace = image.colorspace();
        let source_signature =
            profile_signature_for_colorspace(source_colorspace).ok_or_else(|| {
                operation_error(format!(
                    "ICC profile application does not support {source_colorspace:?} pixels"
                ))
            })?;
        if src_profile.color_space() != source_signature {
            return Err(operation_error(format!(
                "ICC profile color space {:?} does not match {source_colorspace:?} pixels",
                src_profile.color_space()
            )));
        }

        let profile = self
            .profile
            .lock()
            .map_err(|_| ImageOperationsErrors::GenericString("Mutex poisoned".to_string()))?;
        let preserve_alpha = source_colorspace.has_alpha();
        let destination_colorspace = destination_colorspace(profile.color_space(), preserve_alpha)
            .ok_or_else(|| {
                operation_error(format!(
                    "Destination ICC color space {:?} is not supported",
                    profile.color_space()
                ))
            })?;

        if preserve_alpha && !destination_colorspace.has_alpha() {
            return Err(operation_error(
                "Destination ICC color space cannot preserve the image alpha channel",
            ));
        }

        let bit_size = image.depth().bit_size();
        let source_format = pixel_format(source_colorspace, bit_size).ok_or_else(|| {
            operation_error(format!(
                "ICC profile application does not support {source_colorspace:?} at {bit_size}-bit"
            ))
        })?;
        let destination_format = pixel_format(destination_colorspace, bit_size).ok_or_else(|| {
            operation_error(format!(
                "ICC profile application does not support {destination_colorspace:?} at {bit_size}-bit"
            ))
        })?;

        let flags = if preserve_alpha {
            Flags::NO_CACHE | Flags::COPY_ALPHA
        } else {
            Flags::NO_CACHE
        };
        let transform = Transform::<u8, u8>::new_flags(
            &src_profile,
            source_format,
            &profile,
            destination_format,
            Intent::Perceptual,
            flags,
        )
        .map_err(|error| ImageOperationsErrors::GenericString(error.to_string()))?;

        let target_icc = profile
            .icc()
            .map_err(|error| ImageOperationsErrors::GenericString(error.to_string()))?;
        let bit_type = image.depth().bit_type();
        let (width, height) = image.dimensions();
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| operation_error("ICC pixel count overflow"))?;
        if pixel_count > u32::MAX as usize {
            return Err(operation_error(
                "ICC transformation exceeds the LCMS single-operation pixel limit",
            ));
        }
        let destination_len = pixel_count
            .checked_mul(destination_colorspace.num_components())
            .and_then(|samples| samples.checked_mul(image.depth().size_of()))
            .ok_or_else(|| operation_error("ICC destination buffer size overflow"))?;

        for frame in image.frames_mut() {
            let numerator = frame.numerator();
            let denominator = frame.denominator();
            let source = match bit_type {
                BitType::U8 => frame.flatten::<u8>(),
                BitType::U16 => frame.u16_to_native_endian(),
                bit_type => {
                    return Err(ImageErrors::OperationsError(
                        ImageOperationsErrors::UnsupportedType(self.name(), bit_type),
                    ));
                }
            };
            let mut destination = vec![0u8; destination_len];
            transform.transform_pixels(&source, &mut destination);

            *frame = match bit_type {
                BitType::U8 => {
                    Frame::from_u8(&destination, destination_colorspace, numerator, denominator)
                }
                BitType::U16 => {
                    let samples = destination
                        .chunks_exact(2)
                        .map(|sample| u16::from_ne_bytes([sample[0], sample[1]]))
                        .collect::<Vec<_>>();
                    Frame::from_u16(&samples, destination_colorspace, numerator, denominator)
                }
                _ => unreachable!("unsupported bit type was rejected before transformation"),
            };
        }

        image.metadata_mut().set_colorspace(destination_colorspace);
        image.metadata_mut().set_icc_chunk(target_icc);

        Ok(())
    }

    fn supported_types(&self) -> &'static [BitType] {
        &[BitType::U8, BitType::U16]
    }

    fn supported_colorspaces(&self) -> &'static [ColorSpace] {
        &[
            ColorSpace::RGB,
            ColorSpace::RGBA,
            ColorSpace::YCbCr,
            ColorSpace::Luma,
            ColorSpace::LumaA,
            ColorSpace::CMYK,
            ColorSpace::BGR,
            ColorSpace::BGRA,
            ColorSpace::ARGB,
            ColorSpace::HSV,
        ]
    }
}

/// Convert pixels described by the embedded ICC profile to sRGB.
pub struct ApplySRGB;

impl OperationsTrait for ApplySRGB {
    fn name(&self) -> &'static str {
        "apply srgb profile"
    }

    fn execute_impl(&self, image: &mut Image) -> Result<(), ImageErrors> {
        if image.metadata().icc_chunk().is_none() {
            log::warn!("No icc profile in the image, skipping");
            return Ok(());
        }

        ApplyICC::new(Profile::new_srgb()).execute_impl(image)
    }

    fn supported_types(&self) -> &'static [BitType] {
        &[BitType::U8, BitType::U16]
    }

    fn supported_colorspaces(&self) -> &'static [ColorSpace] {
        &[
            ColorSpace::RGB,
            ColorSpace::RGBA,
            ColorSpace::YCbCr,
            ColorSpace::Luma,
            ColorSpace::LumaA,
            ColorSpace::CMYK,
            ColorSpace::BGR,
            ColorSpace::BGRA,
            ColorSpace::ARGB,
            ColorSpace::HSV,
        ]
    }
}

#[cfg(test)]
mod tests;
