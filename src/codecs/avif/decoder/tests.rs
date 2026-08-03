use std::fs::File;

use super::*;

#[test]
fn decode() {
    let file_content = File::open("tests/files/avif/f1t.avif").unwrap();

    let decoder = AvifDecoder::try_new(file_content).unwrap();

    let result = Image::from_decoder(decoder);

    #[cfg(windows)]
    assert!(result.is_err());

    #[cfg(not(windows))]
    {
        let img = result.unwrap();
        assert_eq!(img.dimensions(), (48, 80));
        assert_eq!(img.colorspace(), ColorSpace::RGBA);
    }
}
