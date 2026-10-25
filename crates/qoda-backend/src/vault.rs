//! Explicit Secret Service operations. One worker orders side effects; credentials
//! travel over stdin/stdout pipes, never arguments, environment or app files.
use qoda_provider::Credential;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
};
use zeroize::Zeroizing;

const ATTRIBUTES: [&str; 6] = [
    "application",
    "qoda-player",
    "credential",
    "plex",
    "schema",
    "1",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Timeout,
    Invalid,
    Incomplete,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self{
  Self::Unavailable=>"The desktop keyring could not complete this action. Check that it is available and unlocked, then retry.",
  Self::Timeout=>"The keyring action timed out. Its saved state is unknown; retry the action.",
  Self::Invalid=>"The saved Plex sign-in is invalid. Forget it and connect again.",
  Self::Incomplete=>"Some Qoda sign-in items remain in the keyring. Unlock it and retry Forget.",
 })
    }
}
pub enum Operation {
    Restore,
    Save(Credential),
    Forget,
}
pub enum Outcome {
    Restored(Option<Credential>),
    Saved,
    Forgotten,
}
pub struct Request {
    pub generation: u64,
    pub auth_generation: u64,
    pub operation: Operation,
}
pub struct Reply {
    pub generation: u64,
    pub auth_generation: u64,
    pub result: Result<Outcome, Error>,
}

pub fn start(events: mpsc::Sender<crate::Event>) -> mpsc::Sender<Request> {
    start_with(
        events,
        Vault {
            tool: PathBuf::from("/usr/bin/secret-tool"),
            timeout: Duration::from_secs(30),
        },
    )
}
fn start_with(events: mpsc::Sender<crate::Event>, vault: Vault) -> mpsc::Sender<Request> {
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            let result = vault.perform(request.operation).await;
            if events
                .send(crate::Event::Vault(Reply {
                    generation: request.generation,
                    auth_generation: request.auth_generation,
                    result,
                }))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    tx
}
#[derive(Clone)]
struct Vault {
    tool: PathBuf,
    timeout: Duration,
}
impl Vault {
    async fn run(
        &self,
        verb: &str,
        input: Option<&str>,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, Error> {
        let mut command = Command::new(&self.tool);
        command.arg(verb);
        if verb == "store" {
            command.args(["--label", "Qoda Player Plex sign-in"]);
        }
        if verb == "search" {
            command.arg("--all");
        }
        command
            .args(ATTRIBUTES)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| Error::Unavailable)?;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take().ok_or(Error::Unavailable)?;
        let mut stderr = child.stderr.take().ok_or(Error::Unavailable)?;
        let result = tokio::time::timeout(self.timeout, async {
            let write = async {
                if let Some(ref mut stdin) = stdin {
                    stdin
                        .write_all(input.unwrap_or_default().as_bytes())
                        .await?;
                    stdin.shutdown().await?;
                }
                drop(stdin);
                Ok::<_, std::io::Error>(())
            };
            let out = async {
                let mut out = Zeroizing::new(Vec::new());
                (&mut stdout).take(65537).read_to_end(&mut out).await?;
                if out.len() > 65536 {
                    return Err(std::io::Error::other("keyring response too large"));
                }
                Ok::<_, std::io::Error>(out)
            };
            let err = async {
                let mut err = Zeroizing::new(Vec::new());
                (&mut stderr).take(4097).read_to_end(&mut err).await?;
                Ok::<_, std::io::Error>(!err.is_empty())
            };
            let (_, out, has_error, status) =
                tokio::try_join!(write, out, err, child.wait()).map_err(|_| Error::Unavailable)?;
            if status.success() {
                Ok(Some(out))
            } else if status.code() == Some(1) && out.is_empty() && !has_error {
                Ok(None)
            } else {
                Err(Error::Unavailable)
            }
        })
        .await;
        match result {
            Ok(result) => result,
            Err(_) => {
                let _ = child.kill().await;
                Err(Error::Timeout)
            }
        }
    }
    async fn load(&self) -> Result<Option<Credential>, Error> {
        let Some(value) = self.run("lookup", None).await? else {
            return Ok(None);
        };
        #[derive(serde::Deserialize)]
        struct Version {
            version: u8,
        }
        if serde_json::from_slice::<Version>(&value)
            .map_err(|_| Error::Invalid)?
            .version
            != 1
        {
            return Err(Error::Invalid);
        }
        serde_json::from_slice(&value)
            .map(Some)
            .map_err(|_| Error::Invalid)
    }
    async fn perform(&self, operation: Operation) -> Result<Outcome, Error> {
        match operation {
            Operation::Restore => Ok(Outcome::Restored(self.load().await?)),
            Operation::Save(credential) => {
                let encoded = credential.encode_for_keyring();
                self.run("store", Some(&encoded))
                    .await?
                    .ok_or(Error::Unavailable)?;
                // Verify the stored envelope, including helper implementations with an input limit.
                let restored = self.load().await?.ok_or(Error::Unavailable)?;
                if restored.encode_for_keyring().as_str() != encoded.as_str() {
                    return Err(Error::Invalid);
                }
                Ok(Outcome::Saved)
            }
            Operation::Forget => {
                // libsecret clear only removes unlocked matches. Search without --unlock
                // afterward so locked leftovers cannot be reported as a successful deletion.
                self.run("clear", None).await?;
                if self
                    .run("search", None)
                    .await?
                    .is_some_and(|v| !v.iter().all(u8::is_ascii_whitespace))
                {
                    return Err(Error::Incomplete);
                }
                Ok(Outcome::Forgotten)
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    struct Fixture {
        dir: PathBuf,
        vault: Vault,
    }
    impl Fixture {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("qoda-vault-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            let tool = dir.join("secret-tool");
            std::fs::write(&tool,r#"#!/usr/bin/python3
import sys, pathlib, time
root=pathlib.Path(__file__).parent
op=sys.argv[1]
args=sys.argv[2:]
if op=='store':
 assert args[:2]==['--label','Qoda Player Plex sign-in']
 args=args[2:]
if op=='search':
 assert args[0]=='--all'
 args=args[1:]
assert args==['application','qoda-player','credential','plex','schema','1']
assert 'fixture-secret' not in repr(sys.argv)
if (root/'hang').exists():time.sleep(5)
if (root/'fail').exists():print('private diagnostic fixture-secret',file=sys.stderr);sys.exit(2)
value=root/'value'
if op=='store':value.write_bytes(sys.stdin.buffer.read())
elif op=='lookup':
 if not value.exists():sys.exit(1)
 sys.stdout.buffer.write(value.read_bytes())
elif op=='clear':
 if not (root/'locked').exists():value.unlink(missing_ok=True)
elif op=='search':
 if value.exists():print('locked item metadata' if (root/'locked').exists() else 'private fixture-secret')
"#).unwrap();
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                dir,
                vault: Vault {
                    tool,
                    timeout: Duration::from_secs(2),
                },
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    #[tokio::test]
    async fn pending_save_finishes_before_later_forget() {
        let fixture = Fixture::new();
        let (events, mut replies) = mpsc::channel(8);
        let worker = start_with(events, fixture.vault.clone());
        let credential = Credential::new("qoda-test".into(), "fixture-secret".into()).unwrap();
        worker
            .send(Request {
                generation: 1,
                auth_generation: 1,
                operation: Operation::Save(credential),
            })
            .await
            .unwrap();
        worker
            .send(Request {
                generation: 2,
                auth_generation: 2,
                operation: Operation::Forget,
            })
            .await
            .unwrap();
        let Some(crate::Event::Vault(first)) = replies.recv().await else {
            panic!("missing save reply")
        };
        assert_eq!(first.generation, 1);
        assert!(matches!(first.result, Ok(Outcome::Saved)));
        let Some(crate::Event::Vault(last)) = replies.recv().await else {
            panic!("missing forget reply")
        };
        assert_eq!(last.generation, 2);
        assert!(matches!(last.result, Ok(Outcome::Forgotten)));
        assert!(!fixture.dir.join("value").exists());
        drop(worker);
    }
    #[tokio::test]
    async fn explicit_save_restore_forget_are_scoped_and_idempotent() {
        let f = Fixture::new();
        assert!(matches!(
            f.vault.perform(Operation::Restore).await.unwrap(),
            Outcome::Restored(None)
        ));
        let c = Credential::new("qoda-test".into(), "fixture-secret".into()).unwrap();
        assert!(matches!(
            f.vault.perform(Operation::Save(c)).await.unwrap(),
            Outcome::Saved
        ));
        assert!(matches!(
            f.vault.perform(Operation::Restore).await.unwrap(),
            Outcome::Restored(Some(_))
        ));
        assert!(matches!(
            f.vault.perform(Operation::Forget).await.unwrap(),
            Outcome::Forgotten
        ));
        assert!(matches!(
            f.vault.perform(Operation::Forget).await.unwrap(),
            Outcome::Forgotten
        ));
    }
    #[tokio::test]
    async fn locked_leftovers_and_malformed_values_are_visible_failures() {
        let f = Fixture::new();
        std::fs::write(f.dir.join("value"), "not a credential fixture-secret").unwrap();
        assert!(matches!(f.vault.load().await, Err(Error::Invalid)));
        for value in [
            r#"{"version":2,"client_id":"qoda-test","token":"fixture-secret"}"#,
            r#"{"client_id":"qoda-test","token":"fixture-secret"}"#,
        ] {
            std::fs::write(f.dir.join("value"), value).unwrap();
            assert!(matches!(f.vault.load().await, Err(Error::Invalid)));
        }
        std::fs::write(f.dir.join("locked"), "").unwrap();
        assert!(matches!(
            f.vault.perform(Operation::Forget).await,
            Err(Error::Incomplete)
        ));
        assert!(f.dir.join("value").exists());
    }
    #[tokio::test]
    async fn diagnostics_are_sanitized_and_hung_helpers_are_stopped() {
        let mut f = Fixture::new();
        std::fs::write(f.dir.join("fail"), "").unwrap();
        let e = f.vault.load().await.err().unwrap();
        assert_eq!(e, Error::Unavailable);
        assert!(!e.to_string().contains("fixture-secret"));
        std::fs::remove_file(f.dir.join("fail")).unwrap();
        std::fs::write(f.dir.join("hang"), "").unwrap();
        f.vault.timeout = Duration::from_millis(100);
        assert!(matches!(f.vault.load().await, Err(Error::Timeout)));
    }
}
