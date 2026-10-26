//! Recognises OAuth authorization redirects captured from an embedded browser.

use url::Url;

/// Outcome carried by an OAuth redirect URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirect {
    Code(String),
    Error(String),
}

/// Parses `uri` if it is a redirect to one of `redirect_uris` (or a known native-client
/// redirect) carrying either an authorization `code` or an `error`.
#[must_use]
pub fn parse(uri: &str, redirect_uris: &[&str]) -> Option<Redirect> {
    let is_redirect = redirect_uris.iter().any(|r| uri.starts_with(r))
        || uri.starts_with("ms-appx-web://")
        || uri.contains("/oauth2/nativeclient");
    if !is_redirect {
        return None;
    }
    let parsed = Url::parse(uri).ok()?;
    let mut code = None;
    let mut error = None;
    let mut error_description = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" if !value.is_empty() => code = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "error_description" => error_description = Some(value.into_owned()),
            _ => {}
        }
    }
    if error.is_some() || error_description.is_some() {
        let message = match (error, error_description) {
            (Some(e), Some(d)) => format!("{e}: {d}"),
            (e, d) => e.or(d).unwrap_or_default(),
        };
        return Some(Redirect::Error(message));
    }
    code.map(Redirect::Code)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAM: &str = "ms-appx-web://microsoft.aad.brokerplugin/4fb5cc57";

    #[test]
    fn extracts_code() {
        assert_eq!(
            parse(&format!("{WAM}?code=0.AbC-d_e&session_state=x"), &[]),
            Some(Redirect::Code("0.AbC-d_e".into()))
        );
        assert_eq!(
            parse(
                "https://www.wvd.microsoft.com/webclient?code=abc",
                &["https://www.wvd.microsoft.com/webclient"]
            ),
            Some(Redirect::Code("abc".into()))
        );
    }

    #[test]
    fn reports_errors_and_ignores_others() {
        assert_eq!(
            parse(
                &format!("{WAM}?error=access_denied&error_description=no"),
                &[]
            ),
            Some(Redirect::Error("access_denied: no".into()))
        );
        assert_eq!(
            parse(&format!("{WAM}?error_code=1&code=real"), &[]),
            Some(Redirect::Code("real".into()))
        );
        assert_eq!(parse(&format!("{WAM}?code="), &[]), None);
        assert_eq!(
            parse("https://login.microsoftonline.com/x?code=1", &[]),
            None
        );
        assert_eq!(parse("not a url code%3Dencoded&x", &[]), None);
    }
}
