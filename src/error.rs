/// User-safe categories for MediaWiki API failures. Server-provided error text can
/// echo page content, so responses and job history store only these categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiFailureKind {
    Authentication,
    Permission,
    Block,
    Protection,
    Captcha,
    Spam,
    Conflict,
    Maxlag,
    AbuseFilter,
    Network,
    Unknown,
}

impl ApiFailureKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Permission => "permission",
            Self::Block => "block",
            Self::Protection => "protection",
            Self::Captcha => "captcha",
            Self::Spam => "spam",
            Self::Conflict => "conflict",
            Self::Maxlag => "maxlag",
            Self::AbuseFilter => "abuse filter",
            Self::Network => "network",
            Self::Unknown => "unknown",
        }
    }

    /// Failures that need a person to resolve something on the wiki. A job that
    /// hits one stays locked instead of letting the editor retry.
    pub fn is_policy_sensitive(self) -> bool {
        matches!(
            self,
            Self::AbuseFilter
                | Self::Captcha
                | Self::Spam
                | Self::Block
                | Self::Protection
                | Self::Permission
                | Self::Unknown
        )
    }

    /// Failures after which MediaWiki may already have committed an edit.
    pub fn is_ambiguous(error: &mwapi::Error) -> bool {
        matches!(
            error,
            mwapi::Error::HttpError(_) | mwapi::Error::InvalidJson(_) | mwapi::Error::Unknown(_)
        )
    }
}

pub fn classify(error: &mwapi::Error) -> ApiFailureKind {
    use ApiFailureKind as Kind;
    let code = match error {
        mwapi::Error::NotLoggedIn | mwapi::Error::NotLoggedInAsBot | mwapi::Error::BadToken => {
            return Kind::Authentication;
        }
        mwapi::Error::Maxlag { .. }
        | mwapi::Error::TooManyRequests { .. }
        | mwapi::Error::Readonly { .. } => return Kind::Maxlag,
        mwapi::Error::HttpError(_) => return Kind::Network,
        mwapi::Error::ApiError(error) => error.code.to_ascii_lowercase(),
        _ => return Kind::Unknown,
    };
    if code.contains("abusefilter") {
        Kind::AbuseFilter
    } else if code.contains("captcha") {
        Kind::Captcha
    } else if code.contains("spam") {
        Kind::Spam
    } else if code.contains("editconflict") || code == "articleexists" {
        Kind::Conflict
    } else if code.contains("protected") || code.contains("cascade") {
        Kind::Protection
    } else if code.contains("blocked") || code.contains("block") {
        Kind::Block
    } else if code.contains("assert") || code.contains("token") || code.contains("login") {
        Kind::Authentication
    } else if code.contains("permission")
        || code.contains("denied")
        || code.contains("create")
        || code.contains("right")
    {
        Kind::Permission
    } else {
        Kind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_classified_without_exposing_server_text() {
        let error = |code: &str| {
            mwapi::Error::ApiError(mwapi::ApiError {
                code: code.to_owned(),
                text: "private page text".to_owned(),
                data: None,
            })
        };
        assert_eq!(
            classify(&error("abusefilter-warning")),
            ApiFailureKind::AbuseFilter
        );
        assert_eq!(
            classify(&error("protectedpage")),
            ApiFailureKind::Protection
        );
        assert_eq!(classify(&error("editconflict")), ApiFailureKind::Conflict);
        assert_eq!(classify(&error("spamdetected")), ApiFailureKind::Spam);
        assert_eq!(classify(&error("captcha")), ApiFailureKind::Captcha);
        assert_eq!(
            classify(&mwapi::Error::Maxlag {
                info: String::new(),
                retry_after: None
            }),
            ApiFailureKind::Maxlag
        );
    }

    #[test]
    fn only_wiki_side_problems_lock_a_job() {
        assert!(ApiFailureKind::AbuseFilter.is_policy_sensitive());
        assert!(ApiFailureKind::Block.is_policy_sensitive());
        assert!(!ApiFailureKind::Conflict.is_policy_sensitive());
        assert!(!ApiFailureKind::Maxlag.is_policy_sensitive());
        assert!(!ApiFailureKind::Network.is_policy_sensitive());
        assert!(!ApiFailureKind::Authentication.is_policy_sensitive());
    }
}
