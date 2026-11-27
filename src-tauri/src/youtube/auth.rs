//! Google OAuth for a desktop app: the system browser, a loopback redirect and PKCE.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::error::AppError;

pub const SCOPE: &str = "https://www.googleapis.com/auth/youtube.force-ssl";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);
const MAX_REQUEST_LINE: u64 = 8 * 1024;
/// Google's token address now and then answers 500 for a moment.
const TOKEN_ATTEMPTS: u32 = 3;
const TOKEN_RETRY_PAUSE: Duration = Duration::from_millis(500);

/// The "Desktop app" OAuth client the user downloaded from Google Cloud. Deliberately not `Debug`.
#[derive(Deserialize)]
pub struct Client {
    pub client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
}

impl Client {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        #[derive(Deserialize)]
        struct File {
            installed: Client,
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            AppError::new(format!(
                "cannot read the OAuth client file {}: {e}",
                path.display()
            ))
        })?;
        let client = serde_json::from_str::<File>(&text)
            .map_err(|_| {
                AppError::new(format!(
                    "{} is not a \"Desktop app\" OAuth client file",
                    path.display()
                ))
            })?
            .installed;
        // The client secret and the sign-in code are sent to these.
        if ![&client.auth_uri, &client.token_uri]
            .iter()
            .all(|uri| uri.starts_with("https://"))
        {
            return Err(AppError::new(
                "the OAuth client file must use https addresses",
            ));
        }
        Ok(client)
    }
}

fn random_token() -> Result<String, AppError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| AppError::new(format!("no random numbers: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn consent_url(
    client: &Client,
    redirect: &str,
    state: &str,
    verifier: &str,
) -> Result<String, AppError> {
    let challenge = challenge(verifier);
    Url::parse_with_params(
        &client.auth_uri,
        [
            ("client_id", client.client_id.as_str()),
            ("redirect_uri", redirect),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("state", state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ],
    )
    .map(String::from)
    .map_err(|e| AppError::new(format!("bad sign-in address: {e}")))
}

/// Reads one request to the loopback address. `Some` only for the redirect that carries our `state`.
fn answer(stream: TcpStream, state: &str) -> Option<Result<String, AppError>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    (&mut reader)
        .take(MAX_REQUEST_LINE)
        .read_line(&mut line)
        .ok()?;
    // The rest of the headers is read so that closing the socket does not reset the browser's connection.
    let mut header = String::new();
    while (&mut reader)
        .take(MAX_REQUEST_LINE)
        .read_line(&mut header)
        .ok()?
        > 2
    {
        header.clear();
    }
    let target = line.split_whitespace().nth(1)?;
    let url = Url::parse("http://127.0.0.1").ok()?.join(target).ok()?;
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let outcome = if param("state").as_deref() != Some(state) {
        None
    } else if let Some(code) = param("code") {
        Some(Ok(code))
    } else {
        let reason = param("error").unwrap_or_else(|| "no code".into());
        Some(Err(AppError::new(format!(
            "YouTube access was not granted: {reason}"
        ))))
    };
    let (status, message) = match &outcome {
        Some(Ok(_)) => ("200 OK", "Готово. Вернитесь в приложение."),
        Some(Err(_)) => ("200 OK", "Доступ не выдан. Вернитесь в приложение."),
        None => ("400 Bad Request", "Bad request"),
    };
    let _ = write!(
        reader.get_mut(),
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    outcome
}

fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String, AppError> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(AppError::new("YouTube sign-in cancelled"));
        }
        if Instant::now() >= deadline {
            return Err(AppError::new("YouTube sign-in: no answer from the browser"));
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(outcome) = answer(stream, state) {
                    return outcome;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(e) => return Err(AppError::new(format!("YouTube sign-in failed: {e}"))),
        }
    }
}

#[derive(Deserialize, Default)]
struct TokenReply {
    access_token: Option<String>,
    refresh_token: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

/// A form post to the token address, repeated while Google answers with a server error. `Err` carries Google's
/// error code, if it sent one.
fn token_request(
    client: &Client,
    form: &[(&str, &str)],
) -> Result<TokenReply, (Option<String>, AppError)> {
    let failed = |e: ureq::Error| (None, AppError::new(format!("cannot reach Google: {e}")));
    let credentials = [
        ("client_id", client.client_id.as_str()),
        ("client_secret", client.client_secret.as_str()),
    ];
    let mut attempt = 1;
    let (status, body) = loop {
        let mut response = http()
            .post(&client.token_uri)
            .send_form(credentials.iter().chain(form).copied())
            .map_err(failed)?;
        let status = response.status();
        if status.is_server_error() && attempt < TOKEN_ATTEMPTS {
            std::thread::sleep(TOKEN_RETRY_PAUSE * attempt);
            attempt += 1;
            continue;
        }
        break (
            status,
            response.body_mut().read_to_string().map_err(failed)?,
        );
    };
    let reply: TokenReply = serde_json::from_str(&body).unwrap_or_default();
    if status.is_success() {
        return Ok(reply);
    }
    let error = AppError::new(format!(
        "Google refused the sign-in ({status}): {} {}",
        reply.error.as_deref().unwrap_or_default(),
        reply.error_description.unwrap_or_default()
    ));
    Err((reply.error, error))
}

fn exchange(
    client: &Client,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<String, AppError> {
    let form = [
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect),
    ];
    let reply = token_request(client, &form).map_err(|(_, error)| error)?;
    // Google lets the user untick a permission on the consent screen.
    if !reply
        .scope
        .is_some_and(|granted| granted.split(' ').any(|scope| scope == SCOPE))
    {
        return Err(AppError::new(
            "the YouTube permission was not granted: connect again and allow it",
        ));
    }
    reply
        .refresh_token
        .ok_or_else(|| AppError::new("Google sent no refresh token"))
}

pub enum Refreshed {
    Access(String),
    /// The saved sign-in expired or was revoked: it is of no further use.
    Expired,
}

/// A fresh access token (good for about an hour).
pub fn refresh(client: &Client, refresh_token: &str) -> Result<Refreshed, AppError> {
    let form = [
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    match token_request(client, &form) {
        Ok(reply) => reply
            .access_token
            .map(Refreshed::Access)
            .ok_or_else(|| AppError::new("Google sent no access token")),
        Err((Some(code), _)) if code == "invalid_grant" => Ok(Refreshed::Expired),
        Err((_, error)) => Err(error),
    }
}

/// Signs in through the browser (`open` is given the consent address) and returns the refresh token. Blocks until
/// the browser answers, `cancel` is set or `timeout` passes.
pub fn sign_in(
    client: &Client,
    open: impl FnOnce(&str) -> Result<(), AppError>,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String, AppError> {
    let local = |e: std::io::Error| AppError::new(format!("cannot listen for the sign-in: {e}"));
    let listener = TcpListener::bind("127.0.0.1:0").map_err(local)?;
    listener.set_nonblocking(true).map_err(local)?;
    let redirect = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().map_err(local)?.port()
    );
    let (state, verifier) = (random_token()?, random_token()?);
    open(&consent_url(client, &redirect, &state, &verifier)?)?;
    let code = wait_for_code(&listener, &state, cancel, timeout)?;
    exchange(client, &code, &verifier, &redirect)
}

#[cfg(test)]
mod tests {
    use super::super::fake::Server;
    use super::*;

    fn client(token_uri: &str) -> Client {
        Client {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_uri: "https://accounts.example/auth".into(),
            token_uri: token_uri.into(),
        }
    }

    /// Answers one token request with `status` and `body`; returns its address and the form it was sent.
    fn token_server(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}/token", listener.local_addr().unwrap());
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut form = vec![0; length];
            reader.read_exact(&mut form).unwrap();
            reader.get_mut().write_all(reply.as_bytes()).unwrap();
            String::from_utf8(form).unwrap()
        });
        (address, handle)
    }

    fn param(url: &str, name: &str) -> String {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    }

    fn get(url: &str) -> String {
        let url = Url::parse(url).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        let query = url.query().unwrap_or_default();
        write!(stream, "GET /?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }

    /// A browser that sends a forged request first and then the real redirect with `answer`.
    fn browser(answer: &'static str) -> impl FnOnce(&str) -> Result<(), AppError> {
        move |url| {
            let redirect = param(url, "redirect_uri");
            let state = param(url, "state");
            std::thread::spawn(move || {
                assert!(get(&format!("{redirect}?code=forged&state=wrong")).contains("400"));
                get(&format!("{redirect}?{answer}&state={state}"));
            });
            Ok(())
        }
    }

    const GRANTED: &str = r#"{"access_token":"a","refresh_token":"r","scope":"https://www.googleapis.com/auth/youtube.force-ssl"}"#;

    fn run(token: &Client, answer: &'static str) -> Result<String, AppError> {
        sign_in(
            token,
            browser(answer),
            &AtomicBool::new(false),
            Duration::from_secs(10),
        )
    }

    #[test]
    fn sign_in_returns_the_refresh_token_and_proves_the_pkce_verifier() {
        let (address, server) = token_server("200 OK", GRANTED);
        let mut asked = String::new();
        let open = browser("code=abc%2F1");
        let token = sign_in(
            &client(&address),
            |url| {
                asked = url.to_owned();
                open(url)
            },
            &AtomicBool::new(false),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(token, "r");
        assert_eq!(param(&asked, "scope"), SCOPE);
        assert_eq!(param(&asked, "code_challenge_method"), "S256");
        assert!(param(&asked, "redirect_uri").starts_with("http://127.0.0.1:"));
        let form = format!("http://x/?{}", server.join().unwrap());
        assert_eq!(param(&form, "code"), "abc/1");
        assert_eq!(param(&form, "grant_type"), "authorization_code");
        assert_eq!(param(&form, "redirect_uri"), param(&asked, "redirect_uri"));
        assert_eq!(
            challenge(&param(&form, "code_verifier")),
            param(&asked, "code_challenge")
        );
    }

    #[test]
    fn challenge_is_the_unpadded_base64url_sha256_of_the_verifier() {
        // SHA-256("abc") = ba7816bf...15ad, the FIPS 180 example.
        assert_eq!(
            challenge("abc"),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
        assert_ne!(random_token().unwrap(), random_token().unwrap());
    }

    #[test]
    fn a_refused_consent_is_reported() {
        let error = run(&client("http://127.0.0.1:9/token"), "error=access_denied").unwrap_err();
        assert!(error.to_string().contains("access_denied"), "{error}");
    }

    #[test]
    fn an_answer_without_the_youtube_permission_or_refresh_token_is_refused() {
        for (body, expected) in [
            (
                r#"{"access_token":"a","refresh_token":"r","scope":"openid"}"#,
                "permission",
            ),
            (
                r#"{"access_token":"a","scope":"https://www.googleapis.com/auth/youtube.force-ssl"}"#,
                "refresh token",
            ),
        ] {
            let (address, _server) = token_server("200 OK", body);
            let error = run(&client(&address), "code=c").unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn a_token_error_names_googles_reason_without_the_secret() {
        let (address, _server) = token_server(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"Bad code"}"#,
        );
        let error = run(&client(&address), "code=c").unwrap_err().to_string();
        assert!(
            error.contains("invalid_grant") && error.contains("Bad code"),
            "{error}"
        );
        assert!(!error.contains("secret"), "{error}");
    }

    #[test]
    fn refresh_returns_an_access_token_or_says_the_sign_in_expired() {
        let (address, server) = token_server("200 OK", r#"{"access_token":"fresh"}"#);
        let outcome = refresh(&client(&address), "r1").unwrap();
        assert!(matches!(outcome, Refreshed::Access(token) if token == "fresh"));
        let form = format!("http://x/?{}", server.join().unwrap());
        assert_eq!(param(&form, "refresh_token"), "r1");
        assert_eq!(param(&form, "grant_type"), "refresh_token");

        let (address, _server) = token_server("400 Bad Request", r#"{"error":"invalid_grant"}"#);
        let outcome = refresh(&client(&address), "r").unwrap();
        assert!(matches!(outcome, Refreshed::Expired));
    }

    #[test]
    fn a_server_failure_of_google_is_retried_a_few_times() {
        let failure = (
            "500 Internal Server Error",
            "",
            r#"{"error":"internal_failure"}"#,
        );
        let server = Server::start(&[failure, ("200 OK", "", r#"{"access_token":"fresh"}"#)]);
        let outcome = refresh(&client(&server.url), "r").unwrap();
        assert!(matches!(outcome, Refreshed::Access(token) if token == "fresh"));
        assert_eq!(server.requests()[0].body, server.requests()[1].body);

        let server = Server::start(&[failure; TOKEN_ATTEMPTS as usize]);
        let error = refresh(&client(&server.url), "r").err().unwrap();
        assert!(error.to_string().contains("internal_failure"), "{error}");
        assert_eq!(server.requests().len(), TOKEN_ATTEMPTS as usize);
    }

    #[test]
    fn waiting_ends_on_cancel_and_on_timeout() {
        let client = client("http://127.0.0.1:9/token");
        let cancel = AtomicBool::new(false);
        let error = sign_in(
            &client,
            |_| {
                cancel.store(true, Ordering::SeqCst);
                Ok(())
            },
            &cancel,
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error}");
        let error =
            sign_in(&client, |_| Ok(()), &AtomicBool::new(false), Duration::ZERO).unwrap_err();
        assert!(error.to_string().contains("no answer"), "{error}");
    }

    #[test]
    fn the_client_file_must_exist_and_use_https() {
        let dir = std::env::temp_dir().join(format!("audiobooks-oauth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.json");
        let error = Client::load(&path).err().unwrap().to_string();
        assert!(error.contains("client.json"), "{error}");
        let write = |token_uri: &str| {
            let json = format!(
                r#"{{"installed":{{"client_id":"i","client_secret":"s","auth_uri":"https://a","token_uri":"{token_uri}"}}}}"#
            );
            std::fs::write(&path, json).unwrap();
        };
        write("https://t");
        assert_eq!(Client::load(&path).unwrap().client_id, "i");
        write("http://t");
        assert!(Client::load(&path).is_err());
        std::fs::write(&path, r#"{"web":{}}"#).unwrap();
        assert!(Client::load(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
