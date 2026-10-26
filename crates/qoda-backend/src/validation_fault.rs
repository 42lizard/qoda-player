//! VALIDATION ONLY: one controlled proxy-body interruption, never merge.
use futures_util::StreamExt;
use qoda_provider::{ByteRange, MediaResponse, MediaSource, ProviderError, ProviderFuture};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static ARMED: AtomicBool = AtomicBool::new(false);
static FIRED: AtomicBool = AtomicBool::new(false);
static OPENS: AtomicU64 = AtomicU64::new(0);
static RANGES: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
pub fn arm() {
    if !FIRED.load(Ordering::Acquire) {
        ARMED.store(true, Ordering::Release);
    }
}
pub fn view() -> serde_json::Value {
    serde_json::json!({"armed":ARMED.load(Ordering::Acquire),"fired":FIRED.load(Ordering::Acquire),
        "proxy_body_opens":OPENS.load(Ordering::Acquire),"nonzero_range_opens":RANGES.load(Ordering::Acquire),
        "delivered_bytes":BYTES.load(Ordering::Acquire),"sink":"null"})
}
struct Fault {
    inner: Box<dyn MediaSource>,
}
pub fn wrap(inner: Box<dyn MediaSource>) -> Box<dyn MediaSource> {
    Box::new(Fault { inner })
}
impl MediaSource for Fault {
    fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
        Box::pin(async move {
            if !head {
                OPENS.fetch_add(1, Ordering::AcqRel);
                if range.as_ref().is_some_and(|range| {
                    range
                        .header()
                        .strip_prefix("bytes=")
                        .and_then(|s| s.split_once('-'))
                        .and_then(|(start, _)| start.parse::<u64>().ok())
                        .is_some_and(|start| start > 0)
                }) {
                    RANGES.fetch_add(1, Ordering::AcqRel);
                }
            }
            let mut response = self.inner.open(range, head).await?;
            if head {
                return Ok(response);
            }
            let stream = futures_util::stream::unfold(
                (response.body, bytes::Bytes::new(), false),
                |(mut upstream, mut pending, done)| async move {
                    if done {
                        return None;
                    }
                    if ARMED.load(Ordering::Acquire) && !FIRED.swap(true, Ordering::AcqRel) {
                        ARMED.store(false, Ordering::Release);
                        return Some((Err(ProviderError::Network), (upstream, pending, true)));
                    }
                    if pending.is_empty() {
                        match upstream.next().await? {
                            Ok(bytes) => pending = bytes,
                            Err(error) => return Some((Err(error), (upstream, pending, true))),
                        }
                    }
                    let chunk = pending.split_to(pending.len().min(1024));
                    if !FIRED.load(Ordering::Acquire) {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                    BYTES.fetch_add(chunk.len() as u64, Ordering::AcqRel);
                    Some((Ok(chunk), (upstream, pending, false)))
                },
            );
            response.body = stream.boxed();
            Ok(response)
        })
    }
}
