//! Verify and import the pinned English speech pack without a network client.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash,
    BCryptFinishHash, BCryptHashData,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_READ,
};

pub(super) const MODEL_NAME: &str = "base.en";
pub(super) const MODEL_FORMAT: &str = "Whisper Safetensors";
pub(super) const MODEL_LICENSE: &str = "Apache-2.0";
pub(super) const REVISION: &str = "911407f4214e0e1d82085af863093ec0b66f9cd6";
pub(super) const MODEL_SOURCE: &str =
    "https://huggingface.co/openai/whisper-base.en/tree/911407f4214e0e1d82085af863093ec0b66f9cd6";
pub(super) const WEIGHTS_SHA256: &str =
    "d4dd5542fd6a1d35639e21384238f3bfe6c557c849d392b5905d33ee29e71db5";

struct Asset {
    name: &'static str,
    bytes: u64,
    sha256: &'static str,
}

const ASSETS: [Asset; 3] = [
    Asset {
        name: "config.json",
        bytes: 1_937,
        sha256: "160c1df40a60d4ef5a6014d536fd845c33aabfd1f318458ac7f87cfe91ebbe76",
    },
    Asset {
        name: "tokenizer.json",
        bytes: 2_405_679,
        sha256: "5eb60cec1e77aeeb6869a2bb5a8e01a84c3fe5d072d75369343021fe6f5310d0",
    },
    Asset {
        name: "model.safetensors",
        bytes: 290_401_888,
        sha256: WEIGHTS_SHA256,
    },
];

fn model_root() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("LOCALAPPDATA must name an absolute directory")?;
    Ok(base
        .join("AudioCaptureRust")
        .join("models")
        .join("whisper-base.en"))
}

fn check_directory(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("Expected an ordinary model directory"));
    }
    Ok(())
}

fn create_model_root(root: &Path) -> io::Result<()> {
    // Validate each app-owned component before creating its child.
    let models = root.parent().expect("models directory");
    let app = models.parent().expect("application directory");
    for path in [app, models, root] {
        match fs::create_dir(path) {
            Ok(()) => (),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => (),
            Err(err) => return Err(err),
        }
        check_directory(path)?;
    }
    Ok(())
}

fn hold_directory(path: &Path) -> io::Result<File> {
    let dir = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let meta = dir.metadata()?;
    if !meta.is_dir() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("Expected an ordinary model directory"));
    }
    Ok(dir)
}

fn open_asset(dir: &Path, asset: &Asset) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN)
        .open(dir.join(asset.name))
        .map_err(|err| format!("Cannot open {}: {err}", asset.name))?;
    let meta = file
        .metadata()
        .map_err(|err| format!("Cannot inspect {}: {err}", asset.name))?;
    if !meta.is_file()
        || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || meta.len() != asset.bytes
    {
        return Err(format!(
            "{} has the wrong size or format for Whisper base.en",
            asset.name
        ));
    }
    Ok(file)
}

struct Sha256(BCRYPT_HASH_HANDLE);

impl Sha256 {
    fn new() -> io::Result<Self> {
        let mut handle = null_mut();
        // Windows owns the hash object when the caller supplies an empty object buffer.
        let status = unsafe {
            BCryptCreateHash(
                BCRYPT_SHA256_ALG_HANDLE,
                &mut handle,
                null_mut(),
                0,
                null(),
                0,
                0,
            )
        };
        Self::status(status)?;
        Ok(Self(handle))
    }

    fn status(status: i32) -> io::Result<()> {
        if status < 0 {
            Err(io::Error::other(format!(
                "Windows SHA-256 error {status:#x}"
            )))
        } else {
            Ok(())
        }
    }

    fn update(&mut self, data: &[u8]) -> io::Result<()> {
        let bytes = u32::try_from(data.len())
            .map_err(|_| io::Error::other("SHA-256 block is too large"))?;
        Self::status(unsafe { BCryptHashData(self.0, data.as_ptr(), bytes, 0) })
    }

    fn finish(self) -> io::Result<String> {
        let mut digest = [0_u8; 32];
        Self::status(unsafe {
            BCryptFinishHash(self.0, digest.as_mut_ptr(), digest.len() as u32, 0)
        })?;
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }
}

impl Drop for Sha256 {
    fn drop(&mut self) {
        unsafe { BCryptDestroyHash(self.0) };
    }
}

fn verify_and_copy(
    mut input: impl Read,
    mut output: impl Write,
    asset: &Asset,
    buffer: &mut [u8],
) -> Result<(), String> {
    let result = (|| -> io::Result<()> {
        let mut hash = Sha256::new()?;
        let mut total = 0_u64;
        loop {
            let bytes = input.read(buffer)?;
            if bytes == 0 {
                break;
            }
            total += bytes as u64;
            if total > asset.bytes {
                return Err(io::Error::other("Unexpected length"));
            }
            hash.update(&buffer[..bytes])?;
            output.write_all(&buffer[..bytes])?;
        }
        if total != asset.bytes || hash.finish()? != asset.sha256 {
            return Err(io::Error::other("SHA-256 verification failed"));
        }
        Ok(())
    })();
    result.map_err(|err| format!("Cannot verify {}: {err}", asset.name))
}

/// Keep the pack open while inference opens and reads the verified assets.
pub(super) struct VerifiedPack {
    path: PathBuf,
    _files: Vec<File>,
}

impl VerifiedPack {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn open(root: &Path) -> Result<Self, String> {
        let path = root.join(REVISION);
        let app = root
            .parent()
            .and_then(Path::parent)
            .expect("application directory");
        let mut files = [app, root.parent().expect("models directory"), root, &path]
            .iter()
            .map(|dir| hold_directory(dir))
            .collect::<io::Result<Vec<_>>>()
            .map_err(|err| format!("Cannot open the Whisper base.en pack: {err}"))?;
        let mut buffer = vec![0; 256 * 1024];
        for asset in &ASSETS {
            let file = open_asset(&path, asset)?;
            verify_and_copy(&file, io::sink(), asset, &mut buffer)?;
            files.push(file);
        }
        Ok(Self {
            path,
            _files: files,
        })
    }
}

pub(super) fn installed() -> bool {
    let Ok(root) = model_root() else { return false };
    let path = root.join(REVISION);
    check_directory(&path).is_ok() && ASSETS.iter().all(|asset| open_asset(&path, asset).is_ok())
}

pub(super) fn verified() -> Result<VerifiedPack, String> {
    VerifiedPack::open(&model_root()?)
}

#[allow(dead_code)]
pub(super) fn resolve() -> Result<PathBuf, String> {
    Ok(verified()?.path)
}

struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        for asset in &ASSETS {
            let _ = fs::remove_file(self.0.join(asset.name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

pub(super) fn install(source: &Path) -> Result<PathBuf, String> {
    let root = model_root()?;
    let _source =
        hold_directory(source).map_err(|err| format!("Cannot open model folder: {err}"))?;
    if !source.join("model.safetensors").is_file() && source.join("model.bin").is_file() {
        return Err("This looks like a CTranslate2 model. Import the pinned Whisper base.en Safetensors files.".into());
    }
    create_model_root(&root).map_err(|err| format!("Cannot create model storage: {err}"))?;
    let guard = root.join("import.lock");
    let _lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(guard)
        .map_err(|err| format!("Cannot acquire the model import lock: {err}"))?;
    let destination = root.join(REVISION);
    let stage = root.join(crate::attribution::model::new_id("install")?);
    fs::create_dir(&stage).map_err(|err| format!("Cannot stage model files: {err}"))?;
    let stage = Staging(stage);
    let mut buffer = vec![0; 256 * 1024];
    for asset in &ASSETS {
        let input = open_asset(source, asset)?;
        let mut output = File::create_new(stage.0.join(asset.name))
            .map_err(|err| format!("Cannot stage {}: {err}", asset.name))?;
        verify_and_copy(input, &mut output, asset, &mut buffer)?;
        output
            .sync_all()
            .map_err(|err| format!("Cannot save {}: {err}", asset.name))?;
    }
    if destination.exists() {
        if VerifiedPack::open(&root).is_ok() {
            return Ok(destination);
        }
        // Preserve the invalid copy for diagnostics. The valid pack is already staged.
        let damaged = root.join(crate::attribution::model::new_id("damaged")?);
        fs::rename(&destination, &damaged)
            .map_err(|err| format!("Cannot move the damaged model pack: {err}"))?;
        if let Err(err) = fs::rename(&stage.0, &destination) {
            let _ = fs::rename(&damaged, &destination);
            return Err(format!("Cannot activate the verified model pack: {err}"));
        }
    } else {
        fs::rename(&stage.0, &destination)
            .map_err(|err| format!("Cannot activate the verified model pack: {err}"))?;
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_content_and_rejects_length_or_digest_changes() {
        let asset = Asset {
            name: "fixture",
            bytes: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        let mut buffer = [0; 2];
        let mut output = Vec::new();
        verify_and_copy(&b"abc"[..], &mut output, &asset, &mut buffer).unwrap();
        assert_eq!(output, b"abc");
        for bad in [&b"ab"[..], &b"abd"[..], &b"abcd"[..]] {
            assert!(verify_and_copy(bad, io::sink(), &asset, &mut buffer).is_err());
        }
    }
}
