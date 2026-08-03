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
        let frames = image.flatten_to_u8();

        if frames.is_empty() {
            return Err(ImageErrors::EncodeErrors(ImgEncodeErrors::GenericStatic(
                "Cannot encode an image with no frames",
            )));
        }

        if image.is_animated() {
            let mut encoder = webp::AnimEncoder::new(width as u32, height as u32, &self.options);
            encoder.set_bgcolor([0, 0, 0, 0]);
            encoder.set_loop_count(0);

            frames.iter().try_for_each(|frame| {
                let frame = match image.colorspace() {
                    ColorSpace::RGB => {
                        webp::AnimFrame::from_rgb(frame, width as u32, height as u32, 100)
                    }
                    ColorSpace::RGBA => {
                        webp::AnimFrame::from_rgba(frame, width as u32, height as u32, 100)
                    }
                    colorspace => {
                        return Err(ImageErrors::EncodeErrors(
                            ImgEncodeErrors::UnsupportedColorspace(
                                colorspace,
                                self.supported_colorspaces(),
                            ),
                        ));
                    }
                };
                encoder.add_frame(frame);
                Ok(())
            })?;

            let result = encoder.try_encode().map_err(|error| {
                ImgEncodeErrors::ImageEncodeErrors(format!(
                    "WebP animation encoding failed: {error:?}"
                ))
            })?;
            writer.write(&result).map_err(|error| {
                ImageErrors::EncodeErrors(ImgEncodeErrors::ImageEncodeErrors(format!("{error:?}")))
            })?;
        } else {
            let data = &frames[0];
            let encoder = match image.colorspace() {
                ColorSpace::RGB => webp::Encoder::from_rgb(data, width as u32, height as u32),
                ColorSpace::RGBA => webp::Encoder::from_rgba(data, width as u32, height as u32),
                colorspace => {
                    return Err(ImageErrors::EncodeErrors(
                        ImgEncodeErrors::UnsupportedColorspace(
                            colorspace,
                            self.supported_colorspaces(),
                        ),
                    ));
                }
            };

            let result = encoder.encode_advanced(&self.options).map_err(|error| {
                ImgEncodeErrors::ImageEncodeErrors(format!("webp encoding failed: {error:?}"))
            })?;
            writer.write(&result).map_err(|error| {
                ImageErrors::EncodeErrors(ImgEncodeErrors::ImageEncodeErrors(format!("{error:?}")))
            })?;
        }

        Ok(writer.bytes_written())
    }

    fn supported_colorspaces(&self) -> &'static [ColorSpace] {
        &[ColorSpace::RGB, ColorSpace::RGBA]
    }

    fn format(&self) -> ImageFormat {
        ImageFormat::Unknown
    }

    fn supported_bit_depth(&self) -> &'static [BitDepth] {
        &[BitDepth::Eight]
    }

    fn default_depth(&self, _depth: BitDepth) -> BitDepth {
        BitDepth::Eight
    }

    fn supports_animated_images(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
