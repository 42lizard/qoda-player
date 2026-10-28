//! User-owned folder configuration and asynchronous watch lifetime.
use crate::{Event, cache_files};
use qoda_provider::local::{Local, Root};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{sync::mpsc, task::JoinHandle};
const MAX_ROOTS: usize = 16;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u8,
    roots: Vec<Root>,
}
struct Settings {
    roots: Vec<Root>,
    directory: Option<PathBuf>,
    issue: Option<&'static str>,
    defaulted: bool,
}

/// Only HOME expansion is supported; the XDG file is data, never a shell program.
fn safe_default(path: PathBuf, home: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_owned());
    (canonical != home).then_some(canonical)
}
fn music_default(home: &Path, config: &Path) -> Option<PathBuf> {
    let fallback = home.join("Music");
    let read = || -> std::io::Result<Vec<u8>> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(config.join("user-dirs.dirs"))?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("Not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take(65537).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let bytes = match read() {
        Ok(bytes) if bytes.len() <= 65536 => bytes,
        _ => return safe_default(fallback, home),
    };
    let text = std::str::from_utf8(&bytes).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "XDG_MUSIC_DIR" {
            continue;
        }
        let value = value.trim();
        let value = value.strip_prefix('"').and_then(|s| s.strip_suffix('"'))?;
        let mut expanded = String::new();
        let mut chars = value.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' => {
                    let next = chars.next()?;
                    if !matches!(next, '\\' | '"' | '$' | '`') {
                        expanded.push('\\');
                    }
                    expanded.push(next);
                }
                '$' => {
                    let remaining: String = chars.clone().collect();
                    let length = if remaining.starts_with("HOME/") || remaining == "HOME" {
                        4
                    } else if remaining.starts_with("{HOME}/") || remaining == "{HOME}" {
                        6
                    } else {
                        return None;
                    };
                    for _ in 0..length {
                        chars.next();
                    }
                    expanded.push_str(home.to_str()?);
                }
                '`' => return None,
                other => expanded.push(other),
            }
        }
        let path = PathBuf::from(expanded);
        if path == home {
            return None;
        }
        return (path.is_absolute()
            && !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)))
        .then(|| safe_default(path, home))
        .flatten();
    }
    safe_default(fallback, home)
}
fn roots(paths: Vec<PathBuf>) -> Result<Vec<Root>, &'static str> {
    if paths.len() > MAX_ROOTS {
        return Err("Choose at most 16 local folders.");
    }
    let mut roots: Vec<Root> = Vec::new();
    for path in paths {
        if path.as_os_str().len() > 4096 || path.to_str().is_none() || path.as_os_str().is_empty() {
            return Err("Enter an absolute local folder path.");
        }
        let root = Root::new(path).map_err(|_| "Enter an absolute local folder path.")?;
        if roots
            .iter()
            .any(|r| r.path.starts_with(&root.path) || root.path.starts_with(&r.path))
        {
            return Err("Local folders must not duplicate or contain each other.");
        }
        roots.push(root);
    }
    Ok(roots)
}
impl Settings {
    fn load(directory: Option<PathBuf>, defaults: Vec<PathBuf>) -> Self {
        let read = || {
            let dir = cache_files::Directory::open_settings(
                directory
                    .as_deref()
                    .ok_or("Local folder settings directory is unavailable.")?,
            )?;
            let Some(file) = dir.file("local-folders.json")? else {
                return Ok((roots(defaults)?, true));
            };
            if file
                .metadata()
                .map_err(|_| "Local folder settings are unavailable.")?
                .len()
                > 65536
            {
                return Err("Local folder settings are oversized; existing data was preserved.");
            }
            let value: Preferences = serde_json::from_reader(file.take(65537))
                .map_err(|_| "Local folder settings are invalid; existing data was preserved.")?;
            if value.version != 1 {
                return Err(
                    "Newer local folder settings were preserved; use a compatible Qoda version.",
                );
            }
            let expected = roots(value.roots.iter().map(|r| r.path.clone()).collect())?;
            if expected.iter().zip(&value.roots).any(|(a, b)| a.id != b.id) {
                return Err("Local folder identities changed; existing settings were preserved.");
            }
            Ok((expected, false))
        };
        match read() {
            Ok((roots, defaulted)) => Self {
                roots,
                directory,
                issue: None,
                defaulted,
            },
            Err(issue) => Self {
                roots: vec![],
                directory,
                issue: Some(issue),
                defaulted: false,
            },
        }
    }
    fn set(&mut self, paths: Vec<PathBuf>) -> Result<(), &'static str> {
        if let Some(issue) = self.issue {
            return Err(issue);
        }
        let roots = roots(paths)?;
        let dir = cache_files::Directory::open_settings(
            self.directory
                .as_deref()
                .ok_or("Local folder settings directory is unavailable.")?,
        )?;
        // Refuse a concurrently replaced unsupported preference file before writing.
        if let Some(issue) = Self::load(self.directory.clone(), vec![]).issue {
            return Err(issue);
        }
        let bytes = serde_json::to_vec(&Preferences {
            version: 1,
            roots: roots.clone(),
        })
        .map_err(|_| "Local folder settings are unavailable.")?;
        if bytes.len() > 65536 {
            return Err("Local folder settings are oversized; existing data was preserved.");
        }
        dir.atomic("local-folders.json", &bytes)?;
        self.roots = roots;
        self.defaulted = false;
        Ok(())
    }
}
pub struct Reply {
    generation: u64,
    result: Result<bool, qoda_provider::ProviderError>,
}
pub struct Controller {
    settings: Settings,
    provider: Arc<Local>,
    pending: Option<JoinHandle<()>>,
    generation: u64,
    revision: u64,
    phase: &'static str,
    issue: Option<String>,
}
impl Controller {
    pub fn discover() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.as_ref().map(|p| p.join(".config")));
        let defaults = home
            .as_ref()
            .zip(config.as_ref())
            .and_then(|(home, config)| music_default(home, config))
            .into_iter()
            .collect();
        Self::new(Settings::load(config.map(|p| p.join("qoda")), defaults))
    }
    fn new(settings: Settings) -> Self {
        let provider = Arc::new(Local::new(settings.roots.clone()));
        Self {
            settings,
            provider,
            pending: None,
            generation: 0,
            revision: 0,
            phase: "idle",
            issue: None,
        }
    }
    pub fn provider(&self) -> Arc<Local> {
        self.provider.clone()
    }
    pub fn set(&mut self, paths: Vec<PathBuf>) -> Result<(), &'static str> {
        self.settings.set(paths)?;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Local scan generation exhausted.")?;
        if let Some(task) = self.pending.take() {
            task.abort();
        }
        self.provider = Arc::new(Local::new(self.settings.roots.clone()));
        self.issue = None;
        self.phase = "idle";
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("Local scan generation exhausted.")?;
        Ok(())
    }
    pub fn scan(&mut self, force: bool, tx: mpsc::Sender<Event>) {
        if self.settings.issue.is_some() || self.pending.is_some() {
            return;
        }
        if force {
            self.provider.refresh_metadata();
        }
        let provider = self.provider.clone();
        let generation = self.generation;
        self.phase = "scanning";
        self.pending = Some(tokio::spawn(async move {
            let result = provider.scan().await;
            let _ = tx.send(Event::Local(Reply { generation, result })).await;
        }));
    }
    pub fn complete(&mut self, reply: Reply) -> bool {
        if reply.generation != self.generation {
            return false;
        }
        self.pending = None;
        match reply.result {
            Ok(changed) => {
                self.phase = "ready";
                self.issue = None;
                if changed {
                    self.revision = self.revision.saturating_add(1);
                }
                changed
            }
            Err(error) => {
                self.phase = "error";
                self.issue = Some(error.to_string());
                false
            }
        }
    }
    pub fn view(&self) -> Value {
        let statuses = self.provider.status();
        let folders: Vec<_> = self
            .settings
            .roots
            .iter()
            .map(|root| {
                statuses
                    .iter()
                    .find(|status| status.id == root.id)
                    .map(|status| json!(status))
                    .unwrap_or_else(|| {
                        json!({
                            "id": root.id, "path": root.path, "tracks": "0",
                            "message": "Waiting for local scan."
                        })
                    })
            })
            .collect();
        let message = self.settings.issue.map(str::to_owned)
            .or_else(|| self.issue.clone())
            .unwrap_or_else(|| {
                if self.phase == "scanning" {
                    "Scanning local music folders…"
                } else if self.settings.roots.is_empty() {
                    "No local folders configured. Add a folder to enable local music."
                } else {
                    "Local folders are watched every five seconds. Removing a folder never deletes music."
                }.to_owned()
            });
        json!({
            "available": self.settings.issue.is_none(),
            "folders": folders,
            "paths": self.settings.roots.iter().map(|root| root.path.clone()).collect::<Vec<_>>(),
            "defaulted": self.settings.defaulted,
            "phase": self.phase,
            "revision": self.revision.to_string(),
            "message": message
        })
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(task) = self.pending.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let p = std::env::temp_dir().join(format!("qoda-folders-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        p
    }
    #[test]
    fn xdg_music_defaults_are_data_and_home_disables_scanning() {
        let dir = fixture();
        let home = dir.join("home");
        assert_eq!(music_default(&home, &dir), Some(home.join("Music")));
        std::fs::write(
            dir.join("user-dirs.dirs"),
            "XDG_MUSIC_DIR=\"$HOME/Musik Ä\"\n",
        )
        .unwrap();
        assert_eq!(music_default(&home, &dir), Some(home.join("Musik Ä")));
        std::fs::write(
            dir.join("user-dirs.dirs"),
            "XDG_MUSIC_DIR=\"${HOME}/Music\"\n",
        )
        .unwrap();
        assert_eq!(music_default(&home, &dir), Some(home.join("Music")));
        for value in [
            "$HOME",
            "$(touch /tmp/qoda-must-not-execute)",
            "`touch /tmp/qoda-must-not-execute`",
            "relative",
            "$OTHER/music",
        ] {
            std::fs::write(
                dir.join("user-dirs.dirs"),
                format!("XDG_MUSIC_DIR=\"{value}\"\n"),
            )
            .unwrap();
            assert_eq!(music_default(&home, &dir), None);
        }
        std::fs::create_dir(&home).unwrap();
        let link = dir.join("music-link");
        std::os::unix::fs::symlink(&home, &link).unwrap();
        std::fs::write(
            dir.join("user-dirs.dirs"),
            format!("XDG_MUSIC_DIR=\"{}\"\n", link.display()),
        )
        .unwrap();
        assert_eq!(music_default(&home, &dir), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn private_preferences_preserve_empty_config_and_reject_overlaps_and_future_versions() {
        let dir = fixture();
        let music = dir.join("Music");
        let config = dir.join("config");
        let mut prefs = Settings::load(Some(config.clone()), vec![music.clone()]);
        assert!(prefs.defaulted);
        assert_eq!(prefs.roots.len(), 1);
        assert!(
            prefs
                .set(vec![music.clone(), music.join("nested")])
                .is_err()
        );
        prefs.set(vec![music.clone()]).unwrap();
        let id = prefs.roots[0].id;
        assert_eq!(Settings::load(Some(config.clone()), vec![]).roots[0].id, id);
        prefs.set(vec![]).unwrap();
        let empty = Settings::load(Some(config.clone()), vec![music]);
        assert!(empty.roots.is_empty());
        assert!(!empty.defaulted);
        let path = config.join("local-folders.json");
        std::fs::write(&path, b"{\"version\":99,\"roots\":[]}").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut future = Settings::load(Some(config), vec![]);
        assert!(future.set(vec![]).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn late_scan_completion_cannot_replace_reconfigured_folders() {
        let dir = fixture();
        let mut owner = Controller::new(Settings::load(
            Some(dir.join("config")),
            vec![dir.join("Music")],
        ));
        let (tx, _) = mpsc::channel(8);
        owner.scan(false, tx);
        owner.set(vec![]).unwrap();
        let before = owner.view();
        assert!(!owner.complete(Reply {
            generation: 0,
            result: Ok(true)
        }));
        assert_eq!(owner.view(), before);
        assert!(owner.provider().roots().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn polling_scan_publishes_additions_removals_and_restored_folders() {
        use qoda_provider::MusicProvider;
        let dir = fixture();
        let music = dir.join("Music");
        std::fs::create_dir(&music).unwrap();
        let mut owner = Controller::new(Settings::load(
            Some(dir.join("config")),
            vec![music.clone()],
        ));
        let source = owner.provider().roots()[0].id;
        let (tx, mut rx) = mpsc::channel(8);
        async fn scan(
            owner: &mut Controller,
            tx: mpsc::Sender<Event>,
            rx: &mut mpsc::Receiver<Event>,
        ) {
            owner.scan(false, tx);
            let Some(Event::Local(reply)) =
                tokio::time::timeout(std::time::Duration::from_secs(8), rx.recv())
                    .await
                    .unwrap()
            else {
                panic!("missing local scan");
            };
            owner.complete(reply);
        }
        scan(&mut owner, tx.clone(), &mut rx).await;
        assert!(
            owner
                .provider()
                .tracks(source, 0, String::new())
                .await
                .unwrap()
                .tracks
                .is_empty()
        );
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=48000:cl=stereo",
                "-t",
                "2",
            ])
            .arg(music.join("track.wav"))
            .status()
            .unwrap();
        assert!(status.success());
        scan(&mut owner, tx.clone(), &mut rx).await;
        assert_eq!(
            owner
                .provider()
                .tracks(source, 0, String::new())
                .await
                .unwrap()
                .tracks
                .len(),
            1
        );
        let moved = dir.join("offline");
        std::fs::rename(&music, &moved).unwrap();
        scan(&mut owner, tx.clone(), &mut rx).await;
        assert!(
            owner
                .provider()
                .tracks(source, 0, String::new())
                .await
                .unwrap()
                .tracks
                .is_empty()
        );
        std::fs::rename(moved, &music).unwrap();
        scan(&mut owner, tx.clone(), &mut rx).await;
        assert_eq!(
            owner
                .provider()
                .tracks(source, 0, String::new())
                .await
                .unwrap()
                .tracks
                .len(),
            1
        );
        std::fs::remove_file(music.join("track.wav")).unwrap();
        scan(&mut owner, tx, &mut rx).await;
        assert!(
            owner
                .provider()
                .tracks(source, 0, String::new())
                .await
                .unwrap()
                .tracks
                .is_empty()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
