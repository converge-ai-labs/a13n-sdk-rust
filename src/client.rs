use crate::{
    ProtocolError, ProtocolKind, TransportError, TransportStage,
    generated::apis::configuration::Configuration,
};
use reqwest::{Method, Url, header};
use serde::{Serialize, Serializer, de::DeserializeOwned};
use serde_json::Value;
use std::{
    fmt,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub use crate::generated::apis::Response;

/// Credentials serialize only through the authenticated transport.
#[derive(Clone, Default)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("[REDACTED]")
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub details: Value,
    pub request_id: Option<String>,
    pub retry_after: Option<String>,
    pub headers: header::HeaderMap,
}
/// An accepted Entry that settled without incorporation. The snapshot is
/// retained for inspection; its payload is never included in formatted errors.
#[derive(Debug)]
pub struct SubmissionError {
    pub thread_id: String,
    pub entry_id: String,
    pub entry: Response<crate::generated::models::EntryView>,
}
#[derive(Debug)]
pub enum Error {
    Api(Box<ApiError>),
    Transport(TransportError),
    Protocol(ProtocolError),
    Submission(Box<SubmissionError>),
    Timeout,
    InvalidInput,
    Closed,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(e) => write!(f, "{}: {} ({})", e.code, e.message, e.status),
            Self::Transport(error) => error.fmt(f),
            Self::Protocol(error) => error.fmt(f),
            Self::Submission(error) => write!(
                f,
                "Entry {} settled as {} without incorporation",
                error.entry_id, error.entry.data.status
            ),
            Self::Timeout => {
                f.write_str("Local observation deadline elapsed; remote work may continue")
            }
            Self::InvalidInput => f.write_str(
                "Invalid Service URL, resource identifier, content type, or precondition",
            ),
            Self::Closed => f.write_str("Client is closed"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}
#[derive(Debug)]
pub enum CallError<E> {
    Closed,
    Operation(E),
}
impl<E: fmt::Display> fmt::Display for CallError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("Client is closed"),
            Self::Operation(e) => e.fmt(f),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for CallError<E> {}

type CsrfSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// One pool and local cancellation lifetime. References borrow this owner.
/// Dropping an operation future cancels local work, never durable server Runs.
pub struct Client {
    base_url: Url,
    http: Mutex<Option<reqwest::Client>>,
    pub(crate) shutdown: CancellationToken,
    csrf: Option<CsrfSource>,
    workspace: Option<header::HeaderValue>,
    response_limit: usize,
}

pub struct ClientBuilder {
    base_url: String,
    token: Secret,
    workspace: Option<String>,
    http: reqwest::ClientBuilder,
    csrf: Option<CsrfSource>,
    session: Option<Arc<reqwest::cookie::Jar>>,
    response_limit: usize,
}
impl ClientBuilder {
    pub fn bearer(mut self, token: Secret) -> Self {
        self.token = token;
        self
    }
    /// Select the workspace of a login session for workspace-scoped requests.
    /// API keys already carry their workspace; leave this unset for API keys.
    pub fn workspace(mut self, id: impl Into<String>) -> Self {
        self.workspace = Some(id.into());
        self
    }
    /// Customize TLS and connection settings before SDK redirect/retry policy.
    pub fn http_builder(mut self, http: reqwest::ClientBuilder) -> Self {
        self.http = http;
        self
    }
    /// The caller updates the CSRF token after login; the callback may run concurrently.
    pub fn session(
        mut self,
        jar: Arc<reqwest::cookie::Jar>,
        csrf: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.csrf = Some(Arc::new(csrf));
        self.session = Some(jar);
        self
    }
    pub fn response_limit(mut self, bytes: usize) -> Self {
        self.response_limit = bytes;
        self
    }
    pub fn build(self) -> Result<Client, Error> {
        let base_url = Url::parse(&self.base_url).map_err(|_| Error::InvalidInput)?;
        if !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || self.response_limit == 0
            || (self.session.is_some() && !self.token.0.is_empty())
            || self.workspace.as_ref().is_some_and(String::is_empty)
        {
            return Err(Error::InvalidInput);
        }
        let mut headers = header::HeaderMap::new();
        if !self.token.0.is_empty() {
            let mut value = header::HeaderValue::from_str(&format!("Bearer {}", self.token.0))
                .map_err(|_| Error::InvalidInput)?;
            value.set_sensitive(true);
            headers.insert(header::AUTHORIZATION, value);
        }
        let workspace = self
            .workspace
            .as_deref()
            .map(header::HeaderValue::from_str)
            .transpose()
            .map_err(|_| Error::InvalidInput)?;
        let http = match self.session {
            Some(jar) => self.http.cookie_provider(jar),
            None => self.http,
        };
        let http = http
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|error| TransportError::classify(error, TransportStage::Build))?;
        Ok(Client {
            base_url,
            http: Mutex::new(Some(http)),
            shutdown: CancellationToken::new(),
            csrf: self.csrf,
            workspace,
            response_limit: self.response_limit,
        })
    }
}
impl Client {
    pub fn builder(base_url: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            base_url: base_url.into(),
            token: Secret::default(),
            workspace: None,
            http: reqwest::Client::builder().no_proxy(),
            csrf: None,
            session: None,
            response_limit: 16 << 20,
        }
    }
    pub fn new(base_url: &str, token: Secret) -> Result<Self, Error> {
        Self::builder(base_url).bearer(token).build()
    }
    pub fn resources(&self) -> crate::resources::ServiceResources<'_> {
        crate::resources::ServiceResources(crate::resources::Binding {
            client: self,
            ids: Vec::new(),
        })
    }
    pub(crate) fn http(&self) -> Result<reqwest::Client, Error> {
        self.http
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(Error::Closed)
    }
    pub fn close(&self) {
        self.shutdown.cancel();
        self.http.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
    pub(crate) async fn observe<T>(
        &self,
        work: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        tokio::select! { biased; _=self.shutdown.cancelled()=>Err(Error::Closed),result=work=>result }
    }
    /// Advanced generated protocol access. Consume raw bodies inside this closure
    /// when parent shutdown must cover their delivery. The generated parser is unbounded.
    pub async fn execute<T, E>(
        &self,
        operation: impl AsyncFnOnce(&Configuration) -> Result<T, E>,
    ) -> Result<T, CallError<E>> {
        let http = self.http().map_err(|_| CallError::Closed)?;
        // Configuration carries the current CSRF key for generated session mutations.
        let csrf = self.csrf.as_ref().and_then(|source| source());
        let configuration = Configuration {
            base_path: self.base_url.as_str().trim_end_matches('/').into(),
            client: http,
            user_agent: None,
            basic_auth: None,
            oauth_access_token: None,
            bearer_access_token: None,
            api_key: csrf
                .map(|key| crate::generated::apis::configuration::ApiKey { key, prefix: None }),
        };
        tokio::select! { biased; _=self.shutdown.cancelled()=>Err(CallError::Closed),result=operation(&configuration)=>result.map_err(CallError::Operation) }
    }
    pub(crate) fn request(
        &self,
        method: Method,
        path: &str,
        ids: &[String],
    ) -> Result<reqwest::RequestBuilder, Error> {
        let mut url = self.base_url.clone();
        let mut segments = url.path_segments_mut().map_err(|_| Error::InvalidInput)?;
        segments.pop_if_empty();
        let mut values = ids.iter();
        for segment in path.trim_start_matches('/').split('/') {
            let value = if segment.starts_with('{') {
                values.next().ok_or(Error::InvalidInput)?.as_str()
            } else {
                segment
            };
            if value.is_empty() || value == "." || value == ".." {
                return Err(Error::InvalidInput);
            }
            segments.push(value);
        }
        drop(segments);
        let mut request = self.http()?.request(method.clone(), url);
        if !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS)
            && let Some(csrf) = self.csrf.as_ref().and_then(|source| source())
        {
            request = request.header("X-CSRF-Token", csrf);
        }
        Ok(request)
    }
    /// Resolve operation scope before sending: an explicit header replaces the
    /// session default rather than appending a second, conflicting header.
    pub(crate) fn scoped_request(
        &self,
        request: reqwest::RequestBuilder,
        explicit: Option<&str>,
    ) -> reqwest::RequestBuilder {
        if let Some(value) = explicit {
            request.header("X-Workspace-ID", value)
        } else if let Some(value) = &self.workspace {
            request.header("X-Workspace-ID", value)
        } else {
            request
        }
    }
    pub(crate) async fn send(
        &self,
        request: reqwest::RequestBuilder,
        statuses: &[u16],
    ) -> Result<BinaryResponse, Error> {
        self.observe(async {
            let mut response = request
                .send()
                .await
                .map_err(|error| TransportError::classify(error, TransportStage::Request))?;
            if !statuses.contains(&response.status().as_u16()) {
                let status = response.status().as_u16();
                let headers = response.headers().clone();
                let raw = read_bounded(&mut response, self.response_limit).await?;
                let value: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
                let e = &value["error"];
                return Err(Error::Api(Box::new(ApiError {
                    status,
                    code: e["code"].as_str().unwrap_or("http_error").into(),
                    message: e["message"]
                        .as_str()
                        .unwrap_or("Service request failed")
                        .into(),
                    details: e["details"].clone(),
                    request_id: e["request_id"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| text_header(&headers, "x-request-id").map(str::to_owned)),
                    retry_after: text_header(&headers, "retry-after").map(str::to_owned),
                    headers,
                })));
            }
            Ok(BinaryResponse {
                status: response.status(),
                headers: response.headers().clone(),
                response: Some(response),
                shutdown: self.shutdown.clone(),
            })
        })
        .await
    }
    pub(crate) async fn json<T: DeserializeOwned + Default>(
        &self,
        request: reqwest::RequestBuilder,
        statuses: &[u16],
    ) -> Result<Response<T>, Error> {
        self.observe(async {
            let mut response = self.send(request, statuses).await?;
            let data = if response.status.as_u16() == 204 || response.status.as_u16() == 303 {
                T::default()
            } else {
                let raw = read_bounded(
                    response.response.as_mut().ok_or(Error::Closed)?,
                    self.response_limit,
                )
                .await?;
                serde_json::from_slice(&raw).map_err(|_| {
                    ProtocolError::response(
                        ProtocolKind::InvalidJson,
                        response.status.as_u16(),
                        &response.headers,
                    )
                })?
            };
            Ok(Response {
                data,
                status: response.status,
                headers: response.headers,
            })
        })
        .await
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) fn text_header<'a>(headers: &'a header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}
impl<T> Response<T> {
    pub fn etag(&self) -> Option<&str> {
        text_header(&self.headers, "etag")
    }
    pub fn request_id(&self) -> Option<&str> {
        text_header(&self.headers, "x-request-id")
    }
}
async fn read_bounded(response: &mut reqwest::Response, limit: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| TransportError::classify(error, TransportStage::Body))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(ProtocolError::response(
                ProtocolKind::ResponseTooLarge,
                response.status().as_u16(),
                response.headers(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Streaming body. Drop or close releases it; chunk reads share parent shutdown.
pub struct BinaryResponse {
    pub status: reqwest::StatusCode,
    pub headers: header::HeaderMap,
    response: Option<reqwest::Response>,
    shutdown: CancellationToken,
}
impl BinaryResponse {
    pub async fn chunk(&mut self) -> Result<Option<bytes::Bytes>, Error> {
        if self.shutdown.is_cancelled() {
            self.close();
            return Err(Error::Closed);
        }
        let Some(response) = self.response.as_mut() else {
            return Ok(None);
        };
        let result = tokio::select! {biased; _=self.shutdown.cancelled()=>Err(Error::Closed),chunk=response.chunk()=>chunk.map_err(|error|TransportError::classify(error, TransportStage::Body))};
        if !matches!(&result, Ok(Some(_))) {
            self.close()
        }
        result
    }
    pub fn close(&mut self) {
        self.response.take();
    }
}

/// An owned streaming request body; dropping its request releases this body.
pub struct UploadFile {
    pub name: String,
    pub content_type: String,
    pub body: reqwest::Body,
}
impl UploadFile {
    pub(crate) fn form(self) -> Result<reqwest::multipart::Form, Error> {
        if self.name.is_empty() || self.content_type.is_empty() {
            return Err(Error::InvalidInput);
        }
        let part = reqwest::multipart::Part::stream(self.body)
            .file_name(self.name)
            .mime_str(&self.content_type)
            .map_err(|_| Error::InvalidInput)?;
        Ok(reqwest::multipart::Form::new().part("file", part))
    }
}
