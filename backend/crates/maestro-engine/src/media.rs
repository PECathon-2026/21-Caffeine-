//! Private, content-addressed images. Only opaque references enter transcripts.
use base64::{engine::general_purpose::STANDARD, Engine};
use maestro_types::{provider::ModelMessage, MaestroError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
pub const MAX_BYTES: usize = 12 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAsset {
    pub image_id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub reference: String,
}
fn error(s: impl ToString) -> MaestroError {
    MaestroError::Config(s.to_string())
}
fn valid(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn import(root: &Path, name: &str, bytes: &[u8]) -> Result<ImageAsset> {
    if bytes.len() > MAX_BYTES {
        return Err(error("Image exceeds 12 MiB"));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(error)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(error)?
        .thumbnail(1536, 1536)
        .to_rgb8();
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 85)
        .encode_image(&img)
        .map_err(error)?;
    let id = hex::encode(Sha256::digest(&encoded));
    let dir = directory(root)?;
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{id}.jpg"));
    // Atomic publication avoids readers observing partially written images.
    let tmp = dir.join(format!("{}.{}.tmp", id, maestro_types::new_id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(&encoded)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)?;
    Ok(ImageAsset {
        image_id: id.clone(),
        name: Path::new(name)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .chars()
            .filter(|c| !c.is_control())
            .take(150)
            .collect(),
        width: img.width(),
        height: img.height(),
        reference: format!("[[image:{id}]]"),
    })
}
pub fn upload(root: &Path, name: &str, data: &str) -> Result<ImageAsset> {
    if data.len() > MAX_BYTES * 4 / 3 + 200 {
        return Err(error("Image exceeds 12 MiB"));
    }
    let (header, payload) = data
        .split_once(",")
        .ok_or_else(|| error("Expected base64 image data URL"))?;
    if !matches!(
        header,
        "data:image/png;base64" | "data:image/jpeg;base64" | "data:image/webp;base64"
    ) {
        return Err(error("Supported images: PNG, JPEG, WebP"));
    }
    import(root, name, &STANDARD.decode(payload).map_err(error)?)
}
pub fn image_bytes(root: &Path, id: &str) -> Result<Vec<u8>> {
    if !valid(id) {
        return Err(error("Invalid image reference"));
    }
    let path = directory(root)?.join(format!("{id}.jpg"));
    if path.symlink_metadata()?.file_type().is_symlink() {
        return Err(error("Image references must not be symlinks"));
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(error("Stored image exceeds limit"));
    }
    Ok(bytes)
}
pub fn data_url(root: &Path, id: &str) -> Result<String> {
    Ok(format!(
        "data:image/jpeg;base64,{}",
        STANDARD.encode(image_bytes(root, id)?)
    ))
}
pub fn ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for tail in text.split("[[image:").skip(1) {
        if let Some((id, _)) = tail.split_once("]]") {
            if valid(id) {
                ids.retain(|v| v != id);
                ids.push(id.to_string());
            }
        }
    }
    ids.into_iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}
pub fn message(root: &Path, text: impl Into<String>) -> Result<ModelMessage> {
    let mut m = ModelMessage::user(text);
    m.images = ids(&m.content)
        .iter()
        .map(|id| data_url(root, id))
        .collect::<Result<_>>()?;
    Ok(m)
}

fn directory(root: &Path) -> Result<std::path::PathBuf> {
    for path in [root.join(".maestro"), root.join(".maestro/media")] {
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(error("Image storage must not be a symlink"));
        }
    }
    Ok(root.join(".maestro/media"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_deduplicate_and_hydrate_without_inline_transcript_data() {
        let d = tempfile::tempdir().unwrap();
        let mut input = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(3000, 1000)
            .write_to(&mut input, image::ImageFormat::Png)
            .unwrap();
        let a = import(d.path(), "sample.png", input.get_ref()).unwrap();
        let b = import(d.path(), "other.png", input.get_ref()).unwrap();
        assert_eq!(a.image_id, b.image_id);
        assert_eq!((a.width, a.height), (1536, 512));
        let text = format!("Inspect {}", a.reference);
        let m = message(d.path(), text.clone()).unwrap();
        assert_eq!(m.content, text);
        assert_eq!(m.images.len(), 1);
        assert!(m.images[0].starts_with("data:image/jpeg;base64,"));
        assert!(!serde_json::to_string(&a).unwrap().contains("base64"));
        assert_eq!(
            std::fs::metadata(d.path().join(".maestro/media"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    #[test]
    fn reject_bad_images_and_path_escapes() {
        let d = tempfile::tempdir().unwrap();
        assert!(import(d.path(), "x.png", b"not an image").is_err());
        assert!(upload(d.path(), "x.png", "https://example.com/x").is_err());
        assert!(data_url(d.path(), "../../.env").is_err());
        std::fs::create_dir_all(d.path().join(".maestro")).unwrap();
        std::os::unix::fs::symlink("/tmp", d.path().join(".maestro/media")).unwrap();
        assert!(data_url(d.path(), &"a".repeat(64)).is_err());
    }
    #[test]
    fn references_are_valid_deduplicated_and_bounded() {
        let text = (0..8)
            .map(|i| format!("[[image:{i:064x}]]"))
            .collect::<String>();
        assert_eq!(ids(&text).len(), 4);
        assert_eq!(ids(&text)[0], format!("{:064x}", 4));
        assert!(ids("[[image:../../secret]]").is_empty());
    }
}
