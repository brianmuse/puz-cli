//! Calls to a Swords site over HTTP. Blocking, one call at a time.

use crate::json::Json;
use crate::util::origin;
use anyhow::Result;
use std::sync::OnceLock;
use std::time::Duration;
use ureq::Agent;

/// What proves a request is yours: the puzzle's key, and the sign-in on that site when there is one.
#[derive(Clone, Copy, Debug, Default)]
pub struct Proof<'a> {
    pub key: Option<&'a str>,
    pub session: Option<&'a str>,
    /// Sent with a push: the name to use as the author when the file names none.
    pub author: Option<&'a str>,
}

impl<'a> Proof<'a> {
    /// The headers to send. An empty key or session sends nothing. Every request names the version of
    /// the program that sends it (`x-swords-cli`, see [`set_client`]), so the site can tell a push from
    /// the command line from one from a browser.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("x-swords-cli", client().1.to_string())];
        if let Some(key) = self.key.filter(|k| !k.is_empty()) {
            out.push(("authorization", format!("Bearer {key}")));
        }
        if let Some(session) = self.session.filter(|s| !s.is_empty()) {
            out.push(("x-swords-session", session.to_string()));
        }
        // A header holds plain ASCII only, and a name may not be: it goes encoded.
        if let Some(author) = self.author.filter(|a| !a.is_empty()) {
            out.push(("x-swords-author", crate::util::encode_component(author)));
        }
        out
    }
}

/// What a request carries.
#[derive(Clone, Copy, Debug)]
pub enum Body<'a> {
    None,
    /// A puzzle file, as bytes: a .puz file is not text.
    Bytes(&'a [u8]),
    Json(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

/// A reply from the site, whatever its status.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn json(&self) -> Json {
        Json::parse(&self.body)
    }
}

/// The program that makes the calls, and its version: `("puz", "0.1.0")`. Set it once with
/// [`set_client`] before the first call; without it, this library's own name and version are used.
static CLIENT: OnceLock<(&'static str, &'static str)> = OnceLock::new();

/// Name the program that makes the calls (its name and its version). Only the first call counts.
pub fn set_client(name: &'static str, version: &'static str) {
    let _ = CLIENT.set((name, version));
}

fn client() -> (&'static str, &'static str) {
    *CLIENT.get_or_init(|| ("swords-cli", env!("CARGO_PKG_VERSION")))
}

/// The largest reply this command reads (a build of the command itself is a few MB).
const MAX_REPLY: u64 = 200_000_000;

fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        Agent::config_builder()
            // A status like 404 is a reply to read, not a failure to connect.
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(format!("{}/{}", client().0, client().1))
            .build()
            .into()
    })
}

/// Get a small text from the site, giving up after `seconds`: for a check that must never hold the
/// person up (such as a look for a newer version). Nothing but the request itself is sent: no key, no
/// sign-in, no name. None when the site did not answer in time or said no.
pub fn get_quick(url: &str, seconds: u64) -> Option<String> {
    let agent: Agent = Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(seconds)))
        .user_agent(format!("{}/{}", client().0, client().1))
        .build()
        .into();
    let mut reply = agent.get(url).call().ok()?;
    if !reply.status().is_success() {
        return None;
    }
    reply.body_mut().with_config().limit(1000).read_to_string().ok()
}

/// Send one request. An error here means the site did not answer at all.
pub fn send(method: Method, url: &str, proof: Proof, body: Body) -> Result<Reply, ureq::Error> {
    let agent = agent();
    let mut reply = match method {
        Method::Get | Method::Delete => {
            let mut req = if method == Method::Get { agent.get(url) } else { agent.delete(url) };
            for (name, value) in proof.headers() {
                req = req.header(name, value);
            }
            req.call()?
        }
        Method::Post | Method::Put => {
            let mut req = if method == Method::Post { agent.post(url) } else { agent.put(url) };
            for (name, value) in proof.headers() {
                req = req.header(name, value);
            }
            match body {
                Body::None => req.send_empty()?,
                Body::Bytes(bytes) => req.header("content-type", "application/octet-stream").send(bytes)?,
                Body::Json(text) => req.header("content-type", "application/json").send(text)?,
            }
        }
    };
    let status = reply.status().as_u16();
    let body = reply.body_mut().with_config().limit(MAX_REPLY).read_to_vec()?;
    Ok(Reply { status, body })
}

/// A call that failed: the site did not answer, or it said no.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The address that was called.
    pub url: String,
    /// The HTTP status the site answered with. None: the site did not answer at all.
    pub status: Option<u16>,
    /// The site's own `error` text, when it sent one.
    pub message: Option<String>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match (self.status, &self.message) {
            (None, _) => write!(f, "Can't reach {}. Is the site running? Set another one with: puz swords site URL", origin(&self.url)),
            (Some(_), Some(message)) => f.write_str(message),
            (Some(status), None) => write!(f, "{}: {status}", self.url),
        }
    }
}
impl std::error::Error for Error {}

/// Send one request and give the reply's JSON, or a typed error: no answer, or the site's status and its
/// own `error` text.
pub fn request(method: Method, url: &str, proof: Proof, body: Body) -> Result<Json, Error> {
    let Ok(reply) = send(method, url, proof, body) else {
        return Err(Error { url: url.to_string(), status: None, message: None });
    };
    let data = reply.json();
    if !reply.ok() {
        let message = data.get("error").and_then(Json::as_str).filter(|e| !e.is_empty()).map(str::to_string);
        return Err(Error { url: url.to_string(), status: Some(reply.status), message });
    }
    Ok(data)
}

/// Send one request and give the reply's JSON. A site that does not answer, or says no, is an error
/// in plain words: the site's own `error` text when it sent one.
pub fn call(method: Method, url: &str, proof: Proof, body: Body) -> Result<Json> {
    Ok(request(method, url, proof, body)?)
}

/// Send a request whose failure does not matter: no answer and a "no" both give nothing.
pub fn try_call(method: Method, url: &str, proof: Proof) -> Option<Json> {
    send(method, url, proof, Body::None).ok().filter(Reply::ok).map(|r| r.json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_headers() {
        let version = ("x-swords-cli", client().1.to_string());
        assert_eq!(Proof::default().headers(), vec![version.clone()]);
        assert_eq!(Proof { key: Some(""), session: Some(""), author: Some("") }.headers(), vec![version.clone()]);
        assert_eq!(
            Proof { key: Some("K"), session: Some("S"), author: Some("Zoë B") }.headers(),
            vec![version, ("authorization", "Bearer K".to_string()), ("x-swords-session", "S".to_string()), ("x-swords-author", "Zo%C3%AB%20B".to_string())]
        );
    }

    #[test]
    fn error_words() {
        let e = |status, message: Option<&str>| Error { url: "http://s:1/api/puzzles/x".into(), status, message: message.map(str::to_string) }.to_string();
        assert_eq!(e(None, None), "Can't reach http://s:1. Is the site running? Set another one with: puz swords site URL");
        assert_eq!(e(Some(403), Some("This puzzle is not yours.")), "This puzzle is not yours.");
        assert_eq!(e(Some(500), None), "http://s:1/api/puzzles/x: 500");
    }
}
