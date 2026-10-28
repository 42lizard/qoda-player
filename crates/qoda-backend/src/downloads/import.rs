//! Explicit import writes, separate from the read-only local provider.
use super::*;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
};
pub(super) const MANAGED: &str = qoda_provider::local::DOWNLOAD_DIRECTORY;
pub(super) const STAGING: &str = qoda_provider::local::DOWNLOAD_STAGING;
pub(super) struct Destination {
    root: Root,
    directory: cache_files::Directory,
    staging: cache_files::Directory,
    anchor: File,
}
pub(super) struct Partial {
    pub file: File,
    directory: File,
    name: String,
}
impl Drop for Partial {
    fn drop(&mut self) {
        let path =
            PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd())).join(&self.name);
        if let (Ok(open), Ok(current)) = (self.file.metadata(), std::fs::symlink_metadata(&path))
            && current.is_file()
            && open.dev() == current.dev()
            && open.ino() == current.ino()
            && current.nlink() == 1
        {
            let _ = std::fs::remove_file(path);
        }
    }
}
fn fail(_: impl std::fmt::Debug) -> String {
    "Download storage is unavailable or full.".into()
}
fn identity(file: &File) -> Result<(u64, u64), String> {
    let m = file.metadata().map_err(fail)?;
    Ok((m.dev(), m.ino()))
}
pub(super) fn fingerprint(file: &File) -> Result<String, String> {
    let m = file.metadata().map_err(fail)?;
    Ok(hash(
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec()
        )
        .as_bytes(),
    ))
}
pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && Path::new(name).components().count() == 1
        && matches!(
            Path::new(name).components().next(),
            Some(std::path::Component::Normal(_))
        )
}
impl Destination {
    pub fn open(root: Root) -> Result<Self, String> {
        Self::open_mode(root, true)
    }
    pub fn existing(root: Root) -> Result<Self, String> {
        Self::open_mode(root, false)
    }
    fn open_mode(root: Root, create: bool) -> Result<Self, String> {
        if !root.has_valid_identity() {
            return Err("Invalid download destination.".into());
        }
        let mut anchor = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open("/")
            .map_err(fail)?;
        for component in root.path.components() {
            let std::path::Component::Normal(name) = component else {
                continue;
            };
            let path = PathBuf::from(format!("/proc/self/fd/{}", anchor.as_raw_fd())).join(name);
            if create && std::fs::symlink_metadata(&path).is_err() {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&path)
                    .map_err(fail)?;
            }
            anchor = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(fail)?;
        }
        let m = anchor.metadata().map_err(fail)?;
        if m.uid() != unsafe { libc::getuid() } || m.mode() & 0o022 != 0 {
            return Err(
                "Choose a music folder owned by you and not writable by other users.".into(),
            );
        }
        let anchored = PathBuf::from(format!("/proc/self/fd/{}", anchor.as_raw_fd()));
        let open_directory = |path: &Path| -> Result<cache_files::Directory, String> {
            if create {
                return cache_files::Directory::open(path).map_err(str::to_owned);
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(fail)?;
            let m = file.metadata().map_err(fail)?;
            if m.uid() != unsafe { libc::getuid() } || m.mode() & 0o077 != 0 {
                return Err("Download directory must remain private.".into());
            }
            Ok(cache_files::Directory(file))
        };
        let directory = open_directory(&anchored.join(MANAGED))?;
        let staging = open_directory(&directory.path().join(STAGING))?;
        let this = Self {
            root,
            directory,
            staging,
            anchor,
        };
        this.check()?;
        Ok(this)
    }
    fn check(&self) -> Result<(), String> {
        let current = std::fs::symlink_metadata(&self.root.path).map_err(fail)?;
        let opened = self.anchor.metadata().map_err(fail)?;
        if !current.is_dir()
            || current.dev() != opened.dev()
            || current.ino() != opened.ino()
            || self.directory.resolved_path().map_err(str::to_owned)?
                != self.root.path.join(MANAGED)
            || self.staging.resolved_path().map_err(str::to_owned)?
                != self.root.path.join(MANAGED).join(STAGING)
        {
            return Err("The download folder changed. Select its destination again.".into());
        }
        Ok(())
    }
    pub fn cleanup(&self, id: &str) -> Result<(), String> {
        uuid::Uuid::parse_str(id).map_err(fail)?;
        self.check()?;
        let name = format!("{id}.part");
        if self.staging.file(&name).map_err(str::to_owned)?.is_some() {
            std::fs::remove_file(self.staging.path().join(name)).map_err(fail)?;
        }
        Ok(())
    }
    pub fn partial(&self, id: &str) -> Result<Partial, String> {
        uuid::Uuid::parse_str(id).map_err(fail)?;
        self.check()?;
        let name = format!("{id}.part");
        // A previous attempt is never resumed without source validation. Remove
        // only a private regular staging file in the app-owned staging directory.
        if self.staging.file(&name).map_err(str::to_owned)?.is_some() {
            std::fs::remove_file(self.staging.path().join(&name)).map_err(fail)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.staging.path().join(&name))
            .map_err(fail)?;
        Ok(Partial {
            file,
            directory: self.staging.0.try_clone().map_err(fail)?,
            name,
        })
    }
    pub fn space(&self, bytes: u64) -> Result<(), String> {
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: stat points to writable storage; the descriptor remains open.
        if unsafe { libc::fstatvfs(self.directory.0.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err("Download free space could not be checked.".into());
        }
        // SAFETY: successful fstatvfs initialized stat.
        let stat = unsafe { stat.assume_init() };
        if (stat.f_bavail as u128) * (stat.f_frsize as u128) < u128::from(bytes) + 16 * 1024 * 1024
        {
            return Err("Not enough free space for this download.".into());
        }
        Ok(())
    }
    pub fn planned(
        &self,
        part: &Partial,
        job: &Job,
        version: &str,
        extension: &str,
        checksum: String,
        bytes: u64,
    ) -> Result<ImportedCopy, String> {
        let key = import_key(&job.track.reference, version);
        let title: String = job
            .track
            .title
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
            .take(40)
            .collect();
        let title = if title.trim().is_empty() {
            "track"
        } else {
            title.trim()
        };
        let ext = if !extension.is_empty()
            && extension.len() <= 12
            && extension.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            extension
        } else {
            "audio"
        };
        let mut name = format!("{title}-{}.{}", &key[..16], ext);
        if std::fs::symlink_metadata(self.directory.path().join(&name)).is_ok() {
            name = format!("{title}-{}-{}.{}", &key[..16], &job.id[..8], ext);
        }
        let (device, inode) = identity(&part.file)?;
        let relative = Path::new(MANAGED).join(&name);
        let mut track = job.track.clone();
        track.reference = TrackRef {
            source: self.root.id,
            key: hash(relative.as_os_str().as_encoded_bytes())
                .try_into()
                .unwrap(),
        };
        Ok(ImportedCopy {
            cover_warning: None,
            track,
            origin: job.track.reference.clone(),
            version: version.to_owned(),
            key,
            root: self.root.clone(),
            filename: name,
            checksum,
            bytes,
            device,
            inode,
            fingerprint: String::new(),
            reference: TrackRef {
                source: self.root.id,
                key: hash(relative.as_os_str().as_encoded_bytes())
                    .try_into()
                    .unwrap(),
            },
        })
    }
    pub fn publish(
        &self,
        part: &Partial,
        copy: &mut ImportedCopy,
        gate: &Mutex<u64>,
        generation: u64,
    ) -> Result<(), String> {
        self.check()?;
        if !valid_name(&copy.filename) {
            return Err("Invalid download filename.".into());
        }
        part.file.sync_all().map_err(fail)?;
        {
            let live = gate.lock().unwrap_or_else(|e| e.into_inner());
            if *live != generation {
                return Err(
                    "Download cancelled because its account or destination changed.".into(),
                );
            }
            self.check()?;
            self.rename(part, &copy.filename)?;
        }
        self.directory.0.sync_all().map_err(fail)?;
        self.staging.0.sync_all().map_err(fail)?;
        copy.fingerprint = fingerprint(&part.file)?;
        Ok(())
    }
    fn rename(&self, part: &Partial, name: &str) -> Result<(), String> {
        if !valid_name(name) {
            return Err("Invalid download filename.".into());
        }
        let source = std::ffi::CString::new(part.name.as_bytes()).map_err(fail)?;
        let target = std::ffi::CString::new(name.as_bytes()).map_err(fail)?;
        // SAFETY: open directory descriptors and validated, terminated component names.
        if unsafe {
            libc::renameat2(
                self.staging.0.as_raw_fd(),
                source.as_ptr(),
                self.directory.0.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(
                "The download target already exists or is unavailable; nothing was overwritten."
                    .into(),
            );
        }
        Ok(())
    }
    pub fn cover(
        &self,
        copy: &ImportedCopy,
        bytes: &[u8],
        id: &str,
        gate: &Mutex<u64>,
        generation: u64,
    ) -> Result<(), String> {
        self.verify(copy, false)?;
        if !qoda_provider::artwork::bounded_jpeg(bytes) {
            return Err("Invalid download cover.".into());
        }
        let name = qoda_provider::local::cover_sidecar(Path::new(&copy.filename));
        let name = name.to_str().ok_or("Invalid download filename.")?;
        if let Some(mut file) = self.directory.file(name).map_err(str::to_owned)? {
            let mut existing = Vec::new();
            Read::by_ref(&mut file)
                .take(256 * 1024 + 1)
                .read_to_end(&mut existing)
                .map_err(fail)?;
            return if qoda_provider::artwork::bounded_jpeg(&existing) {
                Ok(())
            } else {
                Err("Invalid download cover.".into())
            };
        }
        self.space(bytes.len() as u64)?;
        let mut part = self.partial(id)?;
        Self::write(&mut part, bytes)?;
        part.file.sync_all().map_err(fail)?;
        {
            let live = gate.lock().unwrap_or_else(|e| e.into_inner());
            if *live != generation {
                return Err(
                    "Download cancelled because its account or destination changed.".into(),
                );
            }
            self.verify(copy, false)?;
            self.rename(&part, name)?;
        }
        self.directory.0.sync_all().map_err(fail)?;
        self.staging.0.sync_all().map_err(fail)?;
        Ok(())
    }
    pub fn verify(&self, copy: &ImportedCopy, full: bool) -> Result<ImportedCopy, String> {
        self.verify_while(copy, full, || true)
    }
    pub fn verify_while(
        &self,
        copy: &ImportedCopy,
        full: bool,
        live: impl Fn() -> bool,
    ) -> Result<ImportedCopy, String> {
        self.check()?;
        if copy.root.id != self.root.id || !valid_name(&copy.filename) {
            return Err("Local download identity changed.".into());
        }
        let mut file = self
            .directory
            .file(&copy.filename)
            .map_err(str::to_owned)?
            .ok_or("The downloaded file is missing.")?;
        if identity(&file)? != (copy.device, copy.inode)
            || file.metadata().map_err(fail)?.len() != copy.bytes
        {
            return Err("The downloaded file changed.".into());
        }
        let before = fingerprint(&file)?;
        if !full && before != copy.fingerprint {
            return Err("The downloaded file changed.".into());
        }
        if full {
            let mut digest = Sha256::new();
            let mut bytes = [0; 65536];
            loop {
                if !live() {
                    return Err("Local copy verification was cancelled.".into());
                }
                let count = file.read(&mut bytes).map_err(fail)?;
                if count == 0 {
                    break;
                }
                digest.update(&bytes[..count]);
            }
            if format!("{:x}", digest.finalize()) != copy.checksum || fingerprint(&file)? != before
            {
                return Err("The downloaded file changed.".into());
            }
        }
        let mut verified = copy.clone();
        verified.fingerprint = before;
        Ok(verified)
    }
    pub fn write(part: &mut Partial, bytes: &[u8]) -> Result<(), String> {
        part.file.write_all(bytes).map_err(fail)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_unsafe_and_full_destinations_refuse_writes_without_touching_other_files() {
        use std::os::unix::fs::symlink;
        let path = std::env::temp_dir().join(format!("qoda-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let root = Root::new(path.clone()).unwrap();
        let missing = path.with_extension("missing");
        let missing_root = Root::new(missing.clone()).unwrap();
        assert!(Destination::existing(missing_root).is_err());
        assert!(!missing.exists());
        let unrelated = path.with_extension("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        std::fs::write(unrelated.join("retained"), b"untouched").unwrap();
        symlink(&unrelated, path.join(MANAGED)).unwrap();
        assert!(Destination::open(root.clone()).is_err());
        std::fs::remove_file(path.join(MANAGED)).unwrap();
        std::fs::create_dir(path.join(MANAGED)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path.join(MANAGED), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        symlink(&unrelated, path.join(MANAGED).join(STAGING)).unwrap();
        assert!(Destination::open(root.clone()).is_err());
        std::fs::remove_file(path.join(MANAGED).join(STAGING)).unwrap();
        std::fs::write(path.join(MANAGED).join(STAGING), b"unrelated regular file").unwrap();
        assert!(Destination::open(root.clone()).is_err());
        std::fs::remove_file(path.join(MANAGED).join(STAGING)).unwrap();
        let destination = Destination::open(root).unwrap();
        assert!(destination.space(u64::MAX).is_err());
        assert_eq!(
            std::fs::read(unrelated.join("retained")).unwrap(),
            b"untouched"
        );
        assert_eq!(std::fs::read_dir(&unrelated).unwrap().count(), 1);
        std::fs::remove_dir_all(path).unwrap();
        std::fs::remove_dir_all(unrelated).unwrap();
    }
    #[test]
    fn imports_publish_original_bytes_without_overwriting_or_following_replaced_roots() {
        let path = std::env::temp_dir().join(format!("qoda-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let root = Root::new(path.clone()).unwrap();
        let destination = Destination::open(root.clone()).unwrap();
        let job = Job::new(
            Track {
                reference: TrackRef {
                    source: qoda_core::SourceId::new(),
                    key: "1".to_owned().try_into().unwrap(),
                },
                title: "../Original Ä".into(),
                artist: String::new(),
                album: String::new(),
                duration_ms: None,
            },
            root,
        );
        let mut part = destination.partial(&job.id).unwrap();
        Destination::write(&mut part, b"fLaC original bytes").unwrap();
        let mut copy = destination
            .planned(
                &part,
                &job,
                "version1",
                "flac",
                hash(b"fLaC original bytes"),
                19,
            )
            .unwrap();
        // Store the prepublication intent in the real job journal before publish;
        // verify(full=true) also recovers a crash before fingerprint persistence.
        assert!(
            destination
                .publish(&part, &mut copy, &Mutex::new(2), 1)
                .is_err()
        );
        let target = path.join(MANAGED).join(&copy.filename);
        assert!(!target.exists());
        std::fs::write(&target, b"unrelated existing music").unwrap();
        assert!(
            destination
                .publish(&part, &mut copy, &Mutex::new(1), 1)
                .is_err()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"unrelated existing music");
        std::fs::remove_file(&target).unwrap();
        destination
            .publish(&part, &mut copy, &Mutex::new(1), 1)
            .unwrap();
        assert!(destination.verify(&copy, true).is_ok());
        let cover = include_bytes!("../../../qoda-provider/tests/fixtures/cover.jpg");
        let image = path
            .join(MANAGED)
            .join(qoda_provider::local::cover_sidecar(Path::new(
                &copy.filename,
            )));
        assert!(
            destination
                .cover(&copy, cover, &job.id, &Mutex::new(2), 1)
                .is_err()
        );
        assert!(!image.exists());
        std::os::unix::fs::symlink("/does-not-exist", &image).unwrap();
        assert!(
            destination
                .cover(&copy, cover, &job.id, &Mutex::new(1), 1)
                .is_err()
        );
        std::fs::remove_file(&image).unwrap();
        std::fs::write(&image, b"unrelated cover").unwrap();
        assert!(
            destination
                .cover(&copy, cover, &job.id, &Mutex::new(1), 1)
                .is_err()
        );
        assert_eq!(std::fs::read(&image).unwrap(), b"unrelated cover");
        std::fs::remove_file(&image).unwrap();
        assert!(
            destination
                .cover(&copy, b"not an image", &job.id, &Mutex::new(1), 1)
                .is_err()
        );
        destination
            .cover(&copy, cover, &job.id, &Mutex::new(1), 1)
            .unwrap();
        assert_eq!(std::fs::read(&image).unwrap(), cover);
        assert!(
            destination
                .publish(&part, &mut copy, &Mutex::new(1), 1)
                .is_err()
        );
        assert_eq!(
            std::fs::read(path.join(MANAGED).join(&copy.filename)).unwrap(),
            b"fLaC original bytes"
        );
        let moved = path.with_extension("moved");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(destination.verify(&copy, true).is_err());
        drop(part);
        std::fs::remove_dir_all(path).unwrap();
        std::fs::remove_dir_all(moved).unwrap();
    }
}
