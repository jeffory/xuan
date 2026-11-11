//! Who may talk to the server: the bearer token, kept in the plugin's data
//! folder readable only by the user, and the checks every HTTP request
//! passes before it reaches MCP.
use std::{
    io::Write,
    path::{Path, PathBuf},
};

use axum::http::{HeaderMap, StatusCode, header};

/// The file in the data folder that holds the token.
pub const TOKEN_FILE: &str = "token";

/// A new random token: 32 bytes from the system's generator, as hex.
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system's random generator works");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn token_path(dir: &Path) -> PathBuf {
    dir.join(TOKEN_FILE)
}

/// The stored token, or a new one written to the data folder.
pub fn load_or_create(dir: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(token_path(dir)) {
        Ok(text) if valid(text.trim()) => Ok(text.trim().to_owned()),
        _ => replace(dir),
    }
}

/// Write a new token, replacing the old one, and return it.
pub fn replace(dir: &Path) -> std::io::Result<String> {
    let token = generate();
    std::fs::create_dir_all(dir)?;
    write_private(&token_path(dir), token.as_bytes())?;
    Ok(token)
}

fn valid(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Write a file only the user can read (0600 on Unix), atomically.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

/// Why a request was turned away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// The `Host` header is not this computer's loopback name: a DNS
    /// rebinding attempt or a misdirected request.
    Host,
    /// The request comes from a web page (it has an `Origin`).
    Origin,
    /// No token, or the wrong one.
    Token,
}

impl Rejection {
    pub fn status(self) -> StatusCode {
        match self {
            Self::Host | Self::Origin => StatusCode::FORBIDDEN,
            Self::Token => StatusCode::UNAUTHORIZED,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Host => "Forbidden: the Host must be 127.0.0.1 or localhost",
            Self::Origin => "Forbidden: requests from web pages are not accepted",
            Self::Token => "Unauthorized: send the token from Xuan's MCP server pane",
        }
    }
}

/// Check a request's headers, before anything else reads it: the `Host`
/// names this computer and the server's port, there is no `Origin` (browsers
/// send one, MCP clients do not), and the bearer token matches.
pub fn check(headers: &HeaderMap, token: &str, port: u16) -> Result<(), Rejection> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or(Rejection::Host)?;
    let (name, given_port) = match host.rsplit_once(':') {
        Some((name, given)) => (name, Some(given)),
        None => (host, None),
    };
    let loopback = name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1";
    let port_matches = given_port.is_none_or(|given| given.parse::<u16>() == Ok(port));
    if !loopback || !port_matches {
        return Err(Rejection::Host);
    }
    if headers.contains_key(header::ORIGIN) {
        return Err(Rejection::Origin);
    }
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(Rejection::Token)?;
    if !same(given.trim().as_bytes(), token.as_bytes()) {
        return Err(Rejection::Token);
    }
    Ok(())
}

/// Compare without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn tokens_are_random_hex_and_stored_privately() {
        let a = generate();
        assert!(valid(&a));
        assert_ne!(a, generate());
        let dir = std::env::temp_dir().join(format!("xuan-mcp-auth-{}", generate()));
        let token = load_or_create(&dir).unwrap();
        assert_eq!(load_or_create(&dir).unwrap(), token, "kept between runs");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let new = replace(&dir).unwrap();
        assert_ne!(new, token);
        assert_eq!(load_or_create(&dir).unwrap(), new);
        // A damaged file is replaced rather than trusted.
        std::fs::write(dir.join(TOKEN_FILE), "short").unwrap();
        assert!(valid(&load_or_create(&dir).unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn requests_need_a_loopback_host_no_origin_and_the_token() {
        let token = "a".repeat(64);
        let bearer = format!("Bearer {token}");
        let good = [
            (header::HOST, "127.0.0.1:8765"),
            (header::AUTHORIZATION, bearer.as_str()),
        ];
        assert_eq!(check(&headers(&good), &token, 8765), Ok(()));
        for host in ["localhost:8765", "LOCALHOST:8765", "127.0.0.1"] {
            let request = headers(&[(header::HOST, host), (header::AUTHORIZATION, &bearer)]);
            assert_eq!(check(&request, &token, 8765), Ok(()), "{host}");
        }
        // DNS rebinding: a name that resolves to 127.0.0.1 is still refused.
        for host in [
            "evil.example:8765",
            "127.0.0.1:9999",
            "0.0.0.0:8765",
            "[::1]:8765",
            "127.0.0.1.nip.io:8765",
        ] {
            let request = headers(&[(header::HOST, host), (header::AUTHORIZATION, &bearer)]);
            assert_eq!(
                check(&request, &token, 8765),
                Err(Rejection::Host),
                "{host}"
            );
        }
        assert_eq!(
            check(&headers(&[(header::AUTHORIZATION, &bearer)]), &token, 8765),
            Err(Rejection::Host)
        );
        // Any browser origin, even a local one.
        for origin in ["http://localhost:8765", "https://evil.example", "null"] {
            let mut request = headers(&good);
            request.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            assert_eq!(
                check(&request, &token, 8765),
                Err(Rejection::Origin),
                "{origin}"
            );
        }
        for authorization in [
            None,
            Some("Bearer "),
            Some("Basic abc"),
            Some("Bearer bbbb"),
        ] {
            let mut request = headers(&[(header::HOST, "127.0.0.1:8765")]);
            if let Some(value) = authorization {
                request.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            }
            assert_eq!(check(&request, &token, 8765), Err(Rejection::Token));
        }
        assert_eq!(Rejection::Token.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(Rejection::Origin.status(), StatusCode::FORBIDDEN);
    }
}
