use zune_core::{
    bit_depth::BitDepth,
    bytestream::{ZByteWriterTrait, ZWriter},
    colorspace::ColorSpace,
};
use zune_image::{
    codecs::ImageFormat,
    errors::{ImageErrors, ImgEncodeErrors},
    image::Image,
    traits::EncoderTrait,
};

/// Alias to [`webp::WebPConfig`]
pub type WebPOptions = webp::WebPConfig;

/// A WebP encoder
pub struct WebPEncoder {
    options: WebPOptions,
}

impl Default for WebPEncoder {
    fn default() -> Self {
        Self {
            options: WebPOptions::new().unwrap(),
        }
    }
}

impl WebPEncoder {
    /// Create a new encoder
    pub fn new() -> WebPEncoder {
        WebPEncoder::default()
    }

    /// Create a new encoder with specified options
    pub fn new_with_options(options: WebPOptions) -> WebPEncoder {
        WebPEncoder { options }
    }
}

impl EncoderTrait for WebPEncoder {
    fn name(&self) -> &'static str {
        "webp"
    }

    fn encode_inner<T: ZByteWriterTrait>(
        &mut self,
        image: &Image,
        sink: T,
    ) -> Result<usize, ImageErrors> {
        let (width, height) = image.dimensions();

        let mut writer = ZWriter::new(sink);

        if image.is_animated() {
            log::warn!(
                "WebP animation encoding is not supported reliably; only the first frame will be encoded"
            );
        }

        let frames = image.flatten_to_u8();
        let data = frames.first().ok_or({
            ImageErrors::EncodeErrors(ImgEncodeErrors::GenericStatic(
                "Cannot encode an image with no frames",
            ))
        })?;

        let encoder = match image.colorspace() {
            ColorSpace::RGB => webp::Encoder::from_rgb(data, width as u32, height as u32),
            ColorSpace::RGBA => webp::Encoder::from_rgba(data, width as u32, height as u32),
            cs => {
                return Err(ImageErrors::EncodeErrors(
                    ImgEncodeErrors::UnsupportedColorspace(cs, self.supported_colorspaces()),
                ));
            }
        };

        let res = encoder.encode_advanced(&self.options).map_err(|e| {
            ImgEncodeErrors::ImageEncodeErrors(format!("webp encoding failed: {e:?}"))
        })?;

        writer.write(&res).map_err(|e| {
            ImageErrors::EncodeErrors(ImgEncodeErrors::ImageEncodeErrors(format!("{e:?}")))
        })?;

        Ok(writer.bytes_written())
    }

    fn supported_colorspaces(&self) -> &'static [ColorSpace] {
        &[ColorSpace::RGB, ColorSpace::RGBA]
    }

    // TODO: update when new version with custom image format is released.
    fn format(&self) -> ImageFormat {
        ImageFormat::Unknown
    }

    fn supported_bit_depth(&self) -> &'static [BitDepth] {
        &[BitDepth::Eight]
    }

    fn default_depth(&self, _depth: BitDepth) -> BitDepth {
        BitDepth::Eight
    }
}

#[cfg(test)]
mod tests;
