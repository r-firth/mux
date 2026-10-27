//! Decoding the PNGs programs send as Kitty images, which libghostty leaves
//! to its embedder.

use std::io::Cursor;

/// Asks libghostty for a buffer of `len` bytes, or null if the image is
/// over its limit.
pub type AllocFn = unsafe extern "C" fn(context: *mut std::ffi::c_void, len: usize) -> *mut u8;

/// The decoder handed to the shim.
pub type DecodeFn = unsafe extern "C" fn(
    data: *const u8,
    len: usize,
    alloc: AllocFn,
    alloc_context: *mut std::ffi::c_void,
    out_width: *mut u32,
    out_height: *mut u32,
    out_pixels: *mut *mut u8,
    out_len: *mut usize,
) -> bool;

/// The most a single PNG may take to decode, as Ghostty's own image limit
/// of a quarter of a gigabyte for the decoded pixels.
const DECODE_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// Decode `data` into RGBA pixels in a buffer from `alloc`. A buffer it was
/// given is reported in `out_pixels` and `out_len` even on failure, so the
/// caller can free it.
///
/// # Safety
///
/// `data` must point to `len` readable bytes, the out pointers must be
/// valid for writes, and `alloc` must return null or a buffer of the bytes
/// asked for.
pub unsafe extern "C" fn decode_png(
    data: *const u8,
    len: usize,
    alloc: AllocFn,
    alloc_context: *mut std::ffi::c_void,
    out_width: *mut u32,
    out_height: *mut u32,
    out_pixels: *mut *mut u8,
    out_len: *mut usize,
) -> bool {
    if data.is_null() || out_width.is_null() || out_height.is_null() {
        return false;
    }
    // SAFETY: the caller lends `len` bytes for the duration of this call.
    let data = unsafe { std::slice::from_raw_parts(data, len) };
    // A program can send anything; a broken image is simply rejected.
    let Ok(Some(image)) = std::panic::catch_unwind(|| decode_rgba(data)) else {
        return false;
    };
    // SAFETY: `alloc` returns null or a buffer of exactly the length asked for.
    let buffer = unsafe { alloc(alloc_context, image.rgba.len()) };
    if buffer.is_null() {
        return false;
    }
    // SAFETY: the out pointers are valid for writes, and the buffer holds
    // `image.rgba.len()` bytes that nothing else refers to yet.
    unsafe {
        *out_pixels = buffer;
        *out_len = image.rgba.len();
        std::ptr::copy_nonoverlapping(image.rgba.as_ptr(), buffer, image.rgba.len());
        *out_width = image.width;
        *out_height = image.height;
    }
    true
}

pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A PNG's first frame as 8-bit RGBA, or `None` if it is not a PNG this
/// can read.
pub(crate) fn decode_rgba(data: &[u8]) -> Option<Decoded> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(data),
        png::Limits {
            bytes: DECODE_LIMIT_BYTES,
        },
    );
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let pixels = &buffer[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], u8::MAX])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        png::ColorType::Grayscale => pixels
            .iter()
            .flat_map(|&gray| [gray, gray, gray, u8::MAX])
            .collect(),
        // Expanded away by the transformations above.
        png::ColorType::Indexed => return None,
    };
    let expected = usize::try_from(u64::from(frame.width) * u64::from(frame.height) * 4).ok()?;
    (rgba.len() == expected).then_some(Decoded {
        width: frame.width,
        height: frame.height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(pixels).expect("pixels");
        writer.finish().expect("finish");
        bytes
    }

    #[test]
    fn decodes_rgb_and_gray_pngs_to_rgba() {
        let rgb = encode(2, 1, png::ColorType::Rgb, &[1, 2, 3, 4, 5, 6]);
        let image = decode_rgba(&rgb).expect("rgb png");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);

        let gray = encode(1, 2, png::ColorType::GrayscaleAlpha, &[9, 128, 10, 0]);
        let image = decode_rgba(&gray).expect("gray png");
        assert_eq!(image.rgba, [9, 9, 9, 128, 10, 10, 10, 0]);
    }

    #[test]
    fn rejects_what_is_not_a_png() {
        assert!(decode_rgba(b"not a png").is_none());
        let mut truncated = encode(4, 4, png::ColorType::Rgba, &[7; 64]);
        truncated.truncate(truncated.len() / 2);
        assert!(decode_rgba(&truncated).is_none());
    }
}
