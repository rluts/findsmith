use axum::http::StatusCode;
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use uuid::Uuid;

const COOKIE_NAME: &str = "findsmith_csrf";

pub fn token(jar: PrivateCookieJar, secure: bool) -> (PrivateCookieJar, String) {
    if let Some(cookie) = jar.get(COOKIE_NAME) {
        return (jar, cookie.value().to_owned());
    }

    let value = Uuid::new_v4().to_string();
    let cookie = Cookie::build((COOKIE_NAME, value.clone()))
        .path("/")
        .secure(secure)
        .http_only(true)
        .same_site(SameSite::Strict)
        .build();
    (jar.add(cookie), value)
}

pub fn verify(jar: &PrivateCookieJar, submitted: &str) -> Result<(), StatusCode> {
    let Some(expected) = jar.get(COOKIE_NAME).map(|cookie| cookie.value().to_owned()) else {
        return Err(StatusCode::FORBIDDEN);
    };
    if !constant_time_eq(expected.as_bytes(), submitted.as_bytes()) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
