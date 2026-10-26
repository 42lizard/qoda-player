//! Bounded UI activation with no playback authority or backend resurrection.
use crate::{Event, mpris};
use std::{
    ffi::OsString,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Debug)]
pub struct ConfigError(pub &'static str);
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ConfigError {}

fn validate(path: &Path) -> Result<(), ConfigError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| ConfigError("Configured UI launcher is unavailable"))?;
    // SAFETY: getuid has no preconditions.
    let owner = unsafe { libc::getuid() };
    if !path.is_absolute()
        || !metadata.is_file()
        || metadata.uid() != owner
        || metadata.mode() & 0o100 == 0
        || metadata.mode() & 0o022 != 0
    {
        return Err(ConfigError(
            "UI launcher must be an absolute, user-owned executable file",
        ));
    }
    Ok(())
}

pub fn configured(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Controller, ConfigError> {
    let mut arguments = arguments.into_iter();
    let mut path = None;
    while let Some(argument) = arguments.next() {
        if argument == "--ui-launcher" {
            if path.is_some() {
                return Err(ConfigError("Duplicate UI launcher configuration"));
            }
            let value = arguments
                .next()
                .ok_or(ConfigError("UI launcher path is missing"))?;
            let candidate = PathBuf::from(value);
            validate(&candidate)?;
            path = Some(candidate);
        } else if argument != "--credential-stdin" && argument != "--restore-backup" {
            return Err(ConfigError("Unsupported backend argument"));
        }
    }
    Ok(Controller {
        path,
        pending: None,
    })
}
struct Pending {
    id: uuid::Uuid,
    task: JoinHandle<()>,
    requests: Vec<mpris::Request>,
}
pub struct Controller {
    path: Option<PathBuf>,
    pending: Option<Pending>,
}
impl Controller {
    pub fn available(&self) -> bool {
        self.path.is_some()
    }
    pub fn start(
        &mut self,
        instance: &str,
        request: Option<mpris::Request>,
        events: mpsc::Sender<Event>,
    ) {
        if request.as_ref().is_some_and(|request| !request.is_live()) {
            if let Some(request) = request {
                let _ = request.reply.send(Err("UI activation expired".into()));
            }
            return;
        }
        if let Some(pending) = &mut self.pending {
            if let Some(request) = request {
                if pending.requests.len() < 8 {
                    pending.requests.push(request);
                } else {
                    let _ = request
                        .reply
                        .send(Err("UI activation is already busy".into()));
                }
            }
            return;
        }
        let Some(path) = self.path.clone() else {
            if let Some(request) = request {
                let _ = request
                    .reply
                    .send(Err("No UI launcher is configured".into()));
            }
            return;
        };
        let id = uuid::Uuid::new_v4();
        let remaining = request
            .as_ref()
            .map(|request| {
                request
                    .deadline
                    .saturating_duration_since(tokio::time::Instant::now())
            })
            .unwrap_or(std::time::Duration::from_secs(10))
            .min(std::time::Duration::from_secs(10));
        let expires = tokio::time::Instant::now() + remaining;
        let instance = instance.to_owned();
        let task = tokio::spawn(async move {
            let result = async {
                let remaining = expires.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err("UI activation expired");
                }
                validate(&path).map_err(|_| "Configured UI launcher is unavailable")?;
                let mut clock = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                // SAFETY: clock points to an initialized writable timespec.
                if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) } != 0 {
                    return Err("UI activation clock is unavailable");
                }
                let deadline = ((clock.tv_sec as u128) * 1000
                    + (clock.tv_nsec as u128) / 1_000_000
                    + remaining.as_millis())
                .to_string();
                let mut child = tokio::process::Command::new(path)
                    .args([
                        "show",
                        "--expected-instance",
                        &instance,
                        "--deadline",
                        &deadline,
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|_| "Could not start the UI launcher")?;
                let status = tokio::time::timeout_at(expires, child.wait())
                    .await
                    .map_err(|_| "UI activation timed out")?
                    .map_err(|_| "UI launcher did not complete")?;
                if status.success() {
                    Ok(())
                } else {
                    Err("UI launcher refused or failed the activation")
                }
            }
            .await;
            let _ = events.send(Event::UiLaunchDone(id, result)).await;
        });
        self.pending = Some(Pending {
            id,
            task,
            requests: request.into_iter().collect(),
        });
    }
    pub fn complete(&mut self, id: uuid::Uuid) -> Option<Vec<mpris::Request>> {
        if !self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            return None;
        }
        self.pending.take().map(|pending| pending.requests)
    }
    pub fn cancel(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.task.abort();
            for request in pending.requests {
                let _ = request
                    .reply
                    .send(Err("Qoda UI activation was cancelled".into()));
            }
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    struct Fixture {
        root: PathBuf,
        launcher: PathBuf,
    }
    impl Fixture {
        fn new(body: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("qoda-activation-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let launcher = root.join("launcher");
            std::fs::write(&launcher, format!("#!/usr/bin/python3\n{body}\n")).unwrap();
            std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self { root, launcher }
        }
        fn controller(&self) -> Controller {
            configured([
                OsString::from("--ui-launcher"),
                self.launcher.clone().into_os_string(),
            ])
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn invalid_unknown_duplicate_and_writable_launcher_configuration_is_refused() {
        assert!(configured([OsString::from("--unknown")]).is_err());
        assert!(configured([OsString::from("--ui-launcher")]).is_err());
        let fixture = Fixture::new("pass");
        assert!(
            configured([
                OsString::from("--ui-launcher"),
                fixture.launcher.clone().into_os_string(),
                OsString::from("--ui-launcher"),
                fixture.launcher.clone().into_os_string()
            ])
            .is_err()
        );
        std::fs::set_permissions(&fixture.launcher, std::fs::Permissions::from_mode(0o777))
            .unwrap();
        assert!(
            configured([
                OsString::from("--ui-launcher"),
                fixture.launcher.clone().into_os_string()
            ])
            .is_err()
        );
    }
    #[tokio::test]
    async fn activation_is_coalesced_instance_fenced_and_ui_only() {
        let fixture = Fixture::new(
            "import json,sys\nfrom pathlib import Path\nwith Path(__file__).with_name('calls').open('a') as file: file.write(json.dumps(sys.argv[1:])+'\\n')",
        );
        let mut controller = fixture.controller();
        let (events, mut received) = mpsc::channel(8);
        controller.start("fixture", None, events.clone());
        controller.start("fixture", None, events);
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), received.recv())
            .await
            .unwrap()
            .unwrap();
        let Event::UiLaunchDone(id, result) = event else {
            panic!("Wrong event")
        };
        assert!(result.is_ok());
        assert!(controller.complete(id).unwrap().is_empty());
        let text = std::fs::read_to_string(fixture.root.join("calls")).unwrap();
        assert_eq!(text.lines().count(), 1);
        let args: Vec<String> = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(
            &args[..4],
            ["show", "--expected-instance", "fixture", "--deadline"]
        );
        assert!(args[4].parse::<u128>().unwrap() > 0);
    }
    #[tokio::test]
    async fn expired_desktop_request_has_no_launch_authority() {
        let fixture = Fixture::new("raise AssertionError('must not execute')");
        let mut controller = fixture.controller();
        let (reply, response) = tokio::sync::oneshot::channel();
        let request = mpris::Request {
            owner: uuid::Uuid::new_v4(),
            action: mpris::Action::Raise,
            deadline: tokio::time::Instant::now() - std::time::Duration::from_secs(1),
            reply,
        };
        let (events, _received) = mpsc::channel(8);
        controller.start("fixture", Some(request), events);
        assert!(response.await.unwrap().is_err());
        assert!(controller.pending.is_none());
    }
    #[tokio::test]
    async fn cancelling_activation_kills_its_owned_helper() {
        let fixture = Fixture::new(
            "import os,time\nfrom pathlib import Path\nready=Path(__file__).with_name('pid')\nstaged=ready.with_suffix('.ready')\nstaged.write_text(str(os.getpid()))\nstaged.replace(ready)\ntime.sleep(30)",
        );
        let mut controller = fixture.controller();
        let (events, _received) = mpsc::channel(8);
        controller.start("fixture", None, events);
        let started = tokio::time::Instant::now();
        while !fixture.root.join("pid").exists() {
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let pid: i32 = std::fs::read_to_string(fixture.root.join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        controller.cancel();
        loop {
            // SAFETY: signal zero only probes this fixture's recorded helper.
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "Helper survived cancellation"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
