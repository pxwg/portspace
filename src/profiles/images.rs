use super::pi::{ImageData, Output};
use crate::{ErrorCode, Result, WorkspaceError};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader};
use std::io::Cursor;

fn error(code: ErrorCode, message: &str) -> WorkspaceError {
    WorkspaceError::new(code, message)
}

pub fn read_image(bytes: &[u8]) -> Result<Option<Output>> {
    let Ok(format) = image::guess_format(bytes) else {
        return Ok(None);
    };
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Bmp
    ) {
        return Err(error(
            ErrorCode::Unsupported,
            "supported image formats: PNG, JPEG, GIF, WebP, BMP",
        ));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    let mut header = ImageReader::with_format(Cursor::new(bytes), format);
    header.limits(limits.clone());
    let (width, height) = header
        .into_dimensions()
        .map_err(|_| error(ErrorCode::InvalidInput, "invalid or oversized image header"))?;
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 16_000_000
    {
        return Err(error(
            ErrorCode::LimitExceeded,
            "image exceeds 16 megapixels or 16384 pixels per dimension",
        ));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| {
        error(
            ErrorCode::InvalidInput,
            "image decoding failed or exceeded resource limits",
        )
    })?;
    let resize = width > 2000 || height > 2000;
    let convert = resize || matches!(format, ImageFormat::Gif | ImageFormat::Bmp);
    let (data, mime) = if convert {
        let image = if resize {
            decoded.thumbnail(2000, 2000)
        } else {
            decoded
        };
        let mut out = Cursor::new(Vec::new());
        image
            .write_to(&mut out, ImageFormat::Png)
            .map_err(|_| error(ErrorCode::InvalidInput, "image encoding failed"))?;
        if out.get_ref().len() > crate::local::FILE_LIMIT {
            return Err(error(
                ErrorCode::LimitExceeded,
                "converted image exceeds 4 MiB; use a smaller image",
            ));
        }
        (out.into_inner(), "image/png")
    } else {
        (
            bytes.to_vec(),
            match format {
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::WebP => "image/webp",
                _ => "image/png",
            },
        )
    };
    let mut text = format!("Read image file [{mime}]");
    if resize {
        text.push_str(&format!(
            "\n[Resized from {width}x{height} to fit 2000x2000]"
        ));
    }
    if format == ImageFormat::Gif {
        text.push_str("\n[GIF: first frame only]");
    }
    Ok(Some(Output {
        text,
        image: Some(ImageData {
            data: STANDARD.encode(data),
            mime: mime.into(),
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_magic_conversion_and_resize() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Bmp,
        ] {
            let image = image::DynamicImage::new_rgb8(12, 8);
            let mut out = Cursor::new(Vec::new());
            image.write_to(&mut out, format).unwrap();
            let result = read_image(out.get_ref()).unwrap().unwrap();
            assert!(result.image.is_some());
        }
        let image = image::DynamicImage::new_rgb8(2001, 2);
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        assert!(
            read_image(out.get_ref())
                .unwrap()
                .unwrap()
                .text
                .contains("Resized")
        );
        assert!(read_image(b"ordinary text").unwrap().is_none());
        assert!(read_image(b"\x89PNG\r\n\x1a\ntruncated").is_err());
    }
}
