mod albums;
mod artists;
mod playlists;
mod reporting;
mod transcode;
use crate::*;
use futures_util::StreamExt;
use reqwest::{
    Client, Url,
    header::{ACCEPT, HeaderMap, HeaderValue, RANGE},
};
use roxmltree::{Document, Node};
use serde::de::Error;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Credential {
    client_id: String,
    token: Zeroizing<String>,
}
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential([redacted])")
    }
}
impl Credential {
    pub fn new(client_id: String, token: String) -> Result<Self, ProviderError> {
        if client_id.is_empty()
            || client_id.len() > 128
            || token.is_empty()
            || token.len() > 8192
            || !client_id.bytes().all(|b| b.is_ascii_graphic())
            || HeaderValue::from_str(&token).is_err()
        {
            return Err(ProviderError::InvalidCredential);
        }
        Ok(Self {
            client_id,
            token: Zeroizing::new(token),
        })
    }
    pub fn encode_for_keyring(&self) -> Zeroizing<String> {
        #[derive(Serialize)]
        struct Stored<'a> {
            version: u8,
            client_id: &'a str,
            token: &'a str,
        }
        Zeroizing::new(
            serde_json::to_string(&Stored {
                version: 1,
                client_id: &self.client_id,
                token: &self.token,
            })
            .expect("serializing strings"),
        )
    }
}
impl<'de> Deserialize<'de> for Credential {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            #[serde(default)]
            version: Option<u8>,
            #[serde(alias = "client")]
            client_id: String,
            token: String,
        }
        let s = Stored::deserialize(d)?;
        if s.version.is_some_and(|v| v != 1) {
            return Err(D::Error::custom("unsupported credential version"));
        }
        Self::new(s.client_id, s.token).map_err(D::Error::custom)
    }
}
fn headers(client: &str, token: Option<&str>) -> Result<HeaderMap, ProviderError> {
    let mut h = HeaderMap::new();
    for (k, v) in [
        ("X-Plex-Product", "Qoda Player"),
        ("X-Plex-Version", env!("CARGO_PKG_VERSION")),
        ("X-Plex-Platform", "Linux"),
        ("X-Plex-Client-Identifier", client),
    ] {
        h.insert(
            reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v).map_err(|_| ProviderError::InvalidCredential)?,
        );
    }
    h.insert(ACCEPT, HeaderValue::from_static("application/xml"));
    if let Some(token) = token {
        let mut v = HeaderValue::from_str(token).map_err(|_| ProviderError::InvalidCredential)?;
        v.set_sensitive(true);
        h.insert("X-Plex-Token", v);
    }
    Ok(h)
}
fn client() -> Result<Client, ProviderError> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(4))
        .read_timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| ProviderError::Network)
}
async fn checked(response: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    match response.status().as_u16() {
        200..=299 => Ok(response),
        401 => Err(ProviderError::SignInRequired),
        403 => Err(ProviderError::AccessDenied),
        404 => Err(ProviderError::MissingTrack),
        _ => Err(ProviderError::Network),
    }
}
async fn text(response: reqwest::Response) -> Result<String, ProviderError> {
    let mut response = checked(response).await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ProviderError::Network)? {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(ProviderError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| ProviderError::InvalidReply)
}
async fn xml(client: &Client, url: Url, h: HeaderMap) -> Result<String, ProviderError> {
    text(
        client
            .get(url)
            .headers(h)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?,
    )
    .await
}
fn attr(node: Node<'_, '_>, key: &str) -> String {
    node.attribute(key).unwrap_or_default().to_owned()
}
fn secure_base(value: &str) -> Result<Url, ProviderError> {
    let u = Url::parse(value).map_err(|_| ProviderError::InvalidReply)?;
    if u.scheme() != "https"
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
    {
        return Err(ProviderError::InvalidReply);
    }
    Ok(u)
}
fn relative(base: &Url, path: &str) -> Result<Url, ProviderError> {
    if !path.starts_with('/') || path.starts_with("//") {
        return Err(ProviderError::InvalidReply);
    }
    let u = base.join(path).map_err(|_| ProviderError::InvalidReply)?;
    if u.origin() != base.origin()
        || u.query_pairs()
            .any(|(k, _)| k.eq_ignore_ascii_case("X-Plex-Token"))
        || u.fragment().is_some()
    {
        return Err(ProviderError::InvalidReply);
    }
    Ok(u)
}
fn path(base: &Url, parts: &[&str]) -> Url {
    let mut u = base.clone();
    u.path_segments_mut()
        .expect("HTTPS base")
        .clear()
        .extend(parts);
    u
}

pub struct BrowserPin {
    id: u64,
    code: String,
    client_id: String,
    pub url: String,
    pub expires_in: u64,
}
impl BrowserPin {
    pub async fn begin(client_id: String) -> Result<Self, ProviderError> {
        let http = client()?;
        let mut h = headers(&client_id, None)?;
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let response = http
            .post("https://plex.tv/api/v2/pins")
            .headers(h)
            .form(&[("strong", "true")])
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        let value: serde_json::Value = serde_json::from_str(&text(response).await?)
            .map_err(|_| ProviderError::InvalidReply)?;
        let id = value["id"].as_u64().ok_or(ProviderError::InvalidReply)?;
        let code = value["code"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(ProviderError::InvalidReply)?
            .to_owned();
        let query = Url::parse_with_params(
            "https://app.plex.tv/",
            [
                ("clientID", client_id.as_str()),
                ("code", code.as_str()),
                ("context[device][product]", "Qoda Player"),
            ],
        )
        .unwrap()
        .query()
        .unwrap()
        .to_owned();
        Ok(Self {
            id,
            code,
            client_id,
            url: format!("https://app.plex.tv/auth#?{query}"),
            expires_in: value["expiresIn"].as_u64().unwrap_or(900).min(900),
        })
    }
    pub async fn poll(&self) -> Result<Option<Credential>, ProviderError> {
        let mut h = headers(&self.client_id, None)?;
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let r = client()?
            .get(format!("https://plex.tv/api/v2/pins/{}", self.id))
            .headers(h)
            .query(&[("code", &self.code)])
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        let v: serde_json::Value =
            serde_json::from_str(&text(r).await?).map_err(|_| ProviderError::InvalidReply)?;
        v["authToken"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|token| Credential::new(self.client_id.clone(), token.to_owned()))
            .transpose()
    }
}
#[derive(Clone)]
struct Source {
    server_id: String,
    view: Library,
    base: Url,
    headers: HeaderMap,
    key: String,
}
pub struct PlexProvider {
    timeline_capabilities: std::sync::Mutex<HashMap<SourceId, reporting::Capability>>,
    account_scope: String,
    unavailable_sources: HashSet<SourceId>,
    unavailable_servers: usize,
    http: Client,
    sources: HashMap<SourceId, Source>,
}
impl PlexProvider {
    /// Stable noncredential account partition for disposable cached metadata.
    pub fn account_scope(&self) -> &str {
        &self.account_scope
    }
    /// Only sources on account-entitled servers whose discovery was unreachable.
    pub fn unavailable_sources(&self) -> &HashSet<SourceId> {
        &self.unavailable_sources
    }
    pub fn unavailable_server_count(&self) -> usize {
        self.unavailable_servers
    }
    pub async fn connect(
        credential: Credential,
        mut identities: HashMap<String, SourceId>,
    ) -> Result<(Self, HashMap<String, SourceId>), ProviderError> {
        let http = client()?;
        let common = headers(&credential.client_id, Some(&credential.token))?;
        let mut user_headers = common.clone();
        user_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let user_response = http
            .get("https://plex.tv/api/v2/user")
            .headers(user_headers)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        let user: serde_json::Value = serde_json::from_str(&text(user_response).await?)
            .map_err(|_| ProviderError::InvalidReply)?;
        let account = user["uuid"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| user["id"].as_u64().map(|id| id.to_string()))
            .ok_or(ProviderError::InvalidReply)?;
        let resource_text = xml(
            &http,
            Url::parse("https://plex.tv/api/resources?includeHttps=1").unwrap(),
            common.clone(),
        )
        .await?;
        let document = Document::parse(&resource_text).map_err(|_| ProviderError::InvalidReply)?;
        if !discovery_container(&document) {
            return Err(ProviderError::InvalidReply);
        }
        struct Server {
            name: String,
            id: String,
            token: String,
            connections: Vec<String>,
        }
        let servers: Vec<_> = document
            .descendants()
            .filter(|n| {
                n.has_tag_name("Device")
                    && n.attribute("provides")
                        .unwrap_or_default()
                        .split(',')
                        .any(|x| x == "server")
            })
            .filter_map(|node| {
                let token = node.attribute("accessToken")?.to_owned();
                Some(Server {
                    name: attr(node, "name"),
                    id: attr(node, "clientIdentifier"),
                    token,
                    connections: node
                        .children()
                        .filter(|c| {
                            c.has_tag_name("Connection") && c.attribute("protocol") == Some("https")
                        })
                        .map(|c| attr(c, "uri"))
                        .collect(),
                })
            })
            .collect();
        let mut sources = HashMap::new();
        let mut unavailable = HashSet::new();
        for server in servers {
            let mut discovered = false;
            let mut denied = false;
            for connection in server.connections {
                let Ok(base) = secure_base(&connection) else {
                    continue;
                };
                let Ok(identity_text) = xml(
                    &http,
                    path(&base, &["identity"]),
                    headers(&credential.client_id, None)?,
                )
                .await
                else {
                    continue;
                };
                let Ok(identity) = Document::parse(&identity_text) else {
                    continue;
                };
                if identity.root_element().attribute("machineIdentifier")
                    != Some(server.id.as_str())
                {
                    continue;
                }
                let h = headers(&credential.client_id, Some(&server.token))?;
                let section_text =
                    match xml(&http, path(&base, &["library", "sections"]), h.clone()).await {
                        Ok(text) => text,
                        Err(ProviderError::AccessDenied | ProviderError::SignInRequired) => {
                            denied = true;
                            continue;
                        }
                        Err(_) => continue,
                    };
                let Ok(sections) = Document::parse(&section_text) else {
                    continue;
                };
                if !discovery_container(&sections) {
                    continue;
                }
                discovered = true;
                for node in sections.descendants().filter(|n| {
                    n.has_tag_name("Directory") && n.attribute("type") == Some("artist")
                }) {
                    let key = attr(node, "key");
                    if key.is_empty() {
                        continue;
                    }
                    let scoped = serde_json::to_string(&[
                        account.as_str(),
                        server.id.as_str(),
                        key.as_str(),
                    ])
                    .unwrap();
                    let id = *identities.entry(scoped).or_default();
                    sources.insert(
                        id,
                        Source {
                            server_id: server.id.clone(),
                            view: Library {
                                id,
                                title: attr(node, "title"),
                                server: server.name.clone(),
                            },
                            base: base.clone(),
                            headers: h.clone(),
                            key,
                        },
                    );
                }
                break;
            }
            if !discovered && !denied {
                unavailable.insert(server.id);
            }
        }
        // A verified empty discovery is meaningful: removed/denied libraries
        // must replace the former allowlist instead of looking like an outage.
        Ok((
            Self {
                timeline_capabilities: Default::default(),
                unavailable_sources: retained_sources(&identities, &account, &unavailable),
                unavailable_servers: unavailable.len(),
                account_scope: format!(
                    "{:x}",
                    Sha256::digest(format!("plex\0{account}").as_bytes())
                ),
                http,
                sources,
            },
            identities,
        ))
    }
}
fn discovery_container(document: &Document<'_>) -> bool {
    document.root_element().has_tag_name("MediaContainer")
}
// Resource entitlement is checked first; absence or explicit denial never
// retains a server merely because it once appeared in a cached identity file.
fn retained_sources(
    ids: &HashMap<String, SourceId>,
    account: &str,
    unavailable: &HashSet<String>,
) -> HashSet<SourceId> {
    ids.iter()
        .filter_map(|(key, id)| {
            let [owner, server, _]: [String; 3] = serde_json::from_str(key).ok()?;
            (owner == account && unavailable.contains(&server)).then_some(*id)
        })
        .collect()
}
// Bound display metadata retained by the session, preserving UTF-8 boundaries.
fn display_attr(node: Node<'_, '_>, key: &str) -> String {
    let value = node.attribute(key).unwrap_or_default();
    let mut end = value.len().min(1024);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn parse_track(node: Node<'_, '_>, source: SourceId) -> Result<Track, ProviderError> {
    let key_value = attr(node, "ratingKey");
    if key_value.len() > 512 {
        return Err(ProviderError::InvalidReply);
    }
    let key = key_value
        .try_into()
        .map_err(|_| ProviderError::InvalidReply)?;
    Ok(Track {
        reference: TrackRef { source, key },
        title: display_attr(node, "title"),
        artist: display_attr(node, "grandparentTitle"),
        album: display_attr(node, "parentTitle"),
        duration_ms: node
            .attribute("duration")
            .and_then(|s| s.parse().ok())
            .map(Counter::new),
    })
}
impl MusicProvider for PlexProvider {
    fn prepare_transcoded(&self, request: TranscodeRequest) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(self.transcode(request))
    }
    fn report_playback(&self, report: playback::Report) -> ProviderFuture<'_, playback::Outcome> {
        Box::pin(self.timeline(report))
    }
    fn artists(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, ArtistPage> {
        Box::pin(self.artist_list(source, offset, query))
    }
    fn artist_albums(
        &self,
        reference: ArtistRef,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(self.albums_by_artist(reference, offset, query))
    }
    fn cover(&self, reference: AlbumRef) -> ProviderFuture<'_, Vec<u8>> {
        Box::pin(self.album_cover(reference))
    }
    fn albums(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(self.album_list(source, offset, query))
    }
    fn album(&self, reference: AlbumRef) -> ProviderFuture<'_, AlbumSnapshot> {
        Box::pin(self.album_snapshot(reference))
    }
    fn playlists(&self, source: SourceId, offset: u64) -> ProviderFuture<'_, PlaylistPage> {
        Box::pin(self.playlist_list(source, offset))
    }
    fn playlist(&self, reference: PlaylistRef) -> ProviderFuture<'_, PlaylistSnapshot> {
        Box::pin(self.playlist_snapshot(reference))
    }
    fn edit_playlist(&self, change: PlaylistChange) -> ProviderFuture<'_, PlaylistOutcome> {
        Box::pin(self.playlist_change(change))
    }

    fn libraries(&self) -> Vec<Library> {
        let mut v: Vec<_> = self.sources.values().map(|s| s.view.clone()).collect();
        v.sort_by(|a, b| (&a.server, &a.title).cmp(&(&b.server, &b.title)));
        v
    }
    fn tracks(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, CatalogPage> {
        Box::pin(async move {
            let s = self
                .sources
                .get(&source)
                .ok_or(ProviderError::UnknownSource)?;
            let mut h = s.headers.clone();
            h.insert(
                "X-Plex-Container-Start",
                HeaderValue::from_str(&offset.to_string()).unwrap(),
            );
            h.insert("X-Plex-Container-Size", HeaderValue::from_static("100"));
            let mut url = if query.is_empty() {
                path(&s.base, &["library", "sections", &s.key, "all"])
            } else {
                path(&s.base, &["hubs", "search"])
            };
            url.query_pairs_mut().append_pair("type", "10");
            if !query.is_empty() {
                url.query_pairs_mut()
                    .append_pair("query", &query)
                    .append_pair("sectionId", &s.key)
                    .append_pair("limit", "100");
            }
            let body = xml(&self.http, url, h).await?;
            let d = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
            let root = d.root_element();
            let returned = root
                .attribute("offset")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            if query.is_empty() && returned != offset {
                return Err(ProviderError::InvalidReply);
            }
            let mut seen = std::collections::HashSet::new();
            let mut tracks = Vec::new();
            for n in d.descendants().filter(|n| n.has_tag_name("Track")) {
                let track = parse_track(n, source)?;
                if seen.insert(String::from(track.reference.key.clone())) {
                    tracks.push(track);
                }
                if tracks.len() >= 100 {
                    break;
                }
            }
            let total = root
                .attribute("totalSize")
                .and_then(|v| v.parse::<u64>().ok());
            let more = query.is_empty()
                && total.is_some_and(|n| offset.saturating_add(tracks.len() as u64) < n);
            Ok(CatalogPage {
                source,
                offset: Counter::new(offset),
                total: total.map(Counter::new),
                query,
                tracks,
                has_more: more,
            })
        })
    }
    fn prepare(
        &self,
        source: SourceId,
        key: ProviderTrackKey,
    ) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async move {
            let s =
                self.sources
                    .get(&source)
                    .ok_or(if self.unavailable_sources.contains(&source) {
                        ProviderError::Network
                    } else {
                        ProviderError::UnknownSource
                    })?;
            let key = String::from(key);
            let mut metadata_url = path(&s.base, &["library", "metadata", &key]);
            metadata_url
                .query_pairs_mut()
                .append_pair("includeMedia", "1");
            let body = xml(&self.http, metadata_url, s.headers.clone()).await?;
            let d = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
            let node = d
                .descendants()
                .find(|n| n.has_tag_name("Track"))
                .ok_or(ProviderError::MissingTrack)?;
            if node
                .attribute("librarySectionID")
                .or_else(|| d.root_element().attribute("librarySectionID"))
                .is_some_and(|id| id != s.key)
            {
                return Err(ProviderError::MissingTrack);
            }
            let track = parse_track(node, source)?;
            let media = node
                .children()
                .find(|n| n.has_tag_name("Media"))
                .ok_or(ProviderError::UnsupportedMedia)?;
            let parts: Vec<_> = media
                .children()
                .filter(|n| n.has_tag_name("Part"))
                .collect();
            if parts.len() != 1 {
                return Err(ProviderError::UnsupportedMedia);
            }
            let part = parts[0];
            let part_key = attr(part, "key");
            if !part_key.starts_with("/library/parts/") {
                return Err(ProviderError::UnsupportedMedia);
            }
            let url = relative(&s.base, &part_key)?;
            Ok(PreparedMedia {
                track,
                version: format!("{}:{}", attr(media, "id"), attr(part, "id")),
                stream: Box::new(PlexStream {
                    http: self.http.clone(),
                    url,
                    headers: s.headers.clone(),
                    etag: Arc::new(std::sync::Mutex::new(None)),
                }),
            })
        })
    }
}
struct PlexStream {
    http: Client,
    url: Url,
    headers: HeaderMap,
    etag: Arc<std::sync::Mutex<Option<String>>>,
}
impl MediaSource for PlexStream {
    fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
        Box::pin(async move {
            let mut request = if head {
                self.http.head(self.url.clone())
            } else {
                self.http.get(self.url.clone())
            }
            .headers(self.headers.clone());
            if let Some(range) = range {
                request = request.header(RANGE, range.header());
            }
            let r = request.send().await.map_err(|_| ProviderError::Network)?;
            let status = r.status().as_u16();
            if ![200, 206, 416].contains(&status) {
                return Err(match status {
                    401 => ProviderError::SignInRequired,
                    403 => ProviderError::AccessDenied,
                    _ => ProviderError::Network,
                });
            }
            let header = |key: &str| {
                r.headers()
                    .get(key)
                    .and_then(|h| h.to_str().ok())
                    .map(str::to_owned)
            };
            let etag = header("etag");
            {
                let mut current = self.etag.lock().unwrap();
                if current
                    .as_ref()
                    .zip(etag.as_ref())
                    .is_some_and(|(a, b)| a != b)
                {
                    return Err(ProviderError::UnsupportedMedia);
                }
                if current.is_none() {
                    *current = etag.clone();
                }
            }
            let content_type = header("content-type");
            let content_range = header("content-range");
            let content_length = r.content_length();
            Ok(MediaResponse {
                status,
                content_type,
                content_length,
                content_range,
                etag,
                body: Box::pin(
                    r.bytes_stream()
                        .map(|chunk| chunk.map_err(|_| ProviderError::Network)),
                ),
            })
        })
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn malformed_discovery_cannot_be_treated_as_an_authoritative_empty_library_list() {
        assert!(discovery_container(
            &Document::parse("<MediaContainer size='0'/>").unwrap()
        ));
        assert!(!discovery_container(&Document::parse("<html/>").unwrap()));
        assert!(!discovery_container(
            &Document::parse("<Unexpected><Directory/></Unexpected>").unwrap()
        ));
    }
    #[tokio::test]
    async fn known_unreachable_source_pauses_as_network_failure_without_preparing_media() {
        let source = SourceId::new();
        let provider = PlexProvider {
            timeline_capabilities: Default::default(),
            http: Client::new(),
            sources: HashMap::new(),
            account_scope: "a".repeat(64),
            unavailable_sources: HashSet::from([source]),
            unavailable_servers: 1,
        };
        assert!(matches!(
            provider
                .prepare(source, "1".to_owned().try_into().unwrap())
                .await,
            Err(ProviderError::Network)
        ));
        assert!(matches!(
            provider
                .prepare(SourceId::new(), "1".to_owned().try_into().unwrap())
                .await,
            Err(ProviderError::UnknownSource)
        ));
    }
    #[test]
    fn cached_source_retention_is_scoped_to_the_current_account_and_entitled_unreachable_servers() {
        let a = SourceId::new();
        let b = SourceId::new();
        let c = SourceId::new();
        let ids = HashMap::from([
            (serde_json::to_string(&["A", "dead", "1"]).unwrap(), a),
            (serde_json::to_string(&["B", "dead", "1"]).unwrap(), b),
            (serde_json::to_string(&["A", "removed", "1"]).unwrap(), c),
        ]);
        assert_eq!(
            retained_sources(&ids, "A", &HashSet::from(["dead".into()])),
            HashSet::from([a])
        );
        assert!(retained_sources(&ids, "A", &HashSet::new()).is_empty());
    }
    use super::*;
    #[test]
    fn retained_display_fields_are_bounded_without_truncating_identity() {
        let title = "♫".repeat(1000);
        let xml = format!("<Track ratingKey='42' title='{title}'/>");
        let doc = Document::parse(&xml).unwrap();
        let track = parse_track(doc.root_element(), SourceId::new()).unwrap();
        assert!(track.title.len() <= 1024);
        assert!(track.title.chars().all(|c| c == '♫'));
        assert_eq!(String::from(track.reference.key), "42");
        let xml = format!("<Track ratingKey='{}'/>", "k".repeat(513));
        assert!(
            parse_track(
                Document::parse(&xml).unwrap().root_element(),
                SourceId::new()
            )
            .is_err()
        );
    }
    #[test]
    fn unsafe_endpoint_and_cross_origin_part_paths_are_rejected() {
        for u in [
            "http://server/",
            "https://user:secret@server/",
            "https://server/?token=x",
            "https://server/path",
        ] {
            assert!(secure_base(u).is_err())
        }
        let base = secure_base("https://server.example/").unwrap();
        for p in [
            "//other/part",
            "https://other/part",
            "/part?X-Plex-Token=secret",
            "/part#fragment",
        ] {
            assert!(relative(&base, p).is_err())
        }
    }
    #[test]
    fn credential_debug_and_public_track_dtos_do_not_expose_secrets() {
        let credential =
            Credential::new("fixture-client".into(), "fixture-secret-not-real".into()).unwrap();
        assert!(!format!("{credential:?}").contains("fixture-secret"));
        let h = headers("fixture", Some("fixture-secret-not-real")).unwrap();
        assert!(h["X-Plex-Token"].is_sensitive());
        assert!(!format!("{h:?}").contains("fixture-secret"));
    }
    #[test]
    fn parses_only_display_metadata_and_source_scoped_identity() {
        let doc=Document::parse("<Track ratingKey='42' title='Fixture' grandparentTitle='Artist' parentTitle='Album' duration='123456'/>").unwrap();
        let track = parse_track(doc.root_element(), SourceId::new()).unwrap();
        let value = serde_json::to_value(track).unwrap();
        assert_eq!(value["duration_ms"], "123456");
        assert_eq!(value["title"], "Fixture");
        assert!(value.get("url").is_none());
    }
}
