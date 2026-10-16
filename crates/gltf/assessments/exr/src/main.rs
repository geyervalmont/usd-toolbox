use image::{DynamicImage, ImageDecoder, ImageFormat};
use std::io::Cursor;
#[unsafe(no_mangle)]
pub extern "C" fn exr_check() -> u32 {
    let image = image::Rgb32FImage::from_fn(8, 4, |x, y| image::Rgb([x as f32 * 0.25 - 0.5, y as f32 * 2., 0.125]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgb32F(image.clone())
        .write_to(&mut encoded, ImageFormat::OpenExr)
        .unwrap();
    let bytes = encoded.into_inner();
    let reader = image::ImageReader::with_format(Cursor::new(&bytes), ImageFormat::OpenExr);
    let header = reader.into_decoder().unwrap();
    assert_eq!(header.dimensions(), (8, 4));
    assert_eq!(header.total_bytes(), 8 * 4 * 12);
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), ImageFormat::OpenExr);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1024);
    limits.max_image_width = Some(8);
    limits.max_image_height = Some(4);
    reader.limits(limits);
    let decoded = reader.decode().unwrap().to_rgb32f();
    assert_eq!(decoded, image);
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), ImageFormat::OpenExr);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(8);
    reader.limits(limits);
    assert!(reader.decode().is_err());
    bytes.len() as u32
}
fn main() {
    println!("{}", exr_check());
}
