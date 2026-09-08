use serde::Deserialize;

use crate::AppResult;

#[derive(Deserialize)]
struct UserInfoResponse {
    query: UserInfoQuery,
}

#[derive(Deserialize)]
struct UserInfoQuery {
    userinfo: UserInfo,
}

#[derive(Deserialize)]
struct UserInfo {
    name: String,
}

pub async fn owner_username(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
) -> AppResult<String> {
    let client = mwapi::Client::builder(wiki_url)
        .set_oauth2_token(access_token)
        .set_user_agent(user_agent)
        .set_assert(mwapi::Assert::User)
        .build()
        .await?;
    let response: UserInfoResponse = client
        .get([("action", "query"), ("meta", "userinfo")])
        .await?;
    if response.query.userinfo.name.trim().is_empty() {
        return Err("OWNER_ACCESS_TOKEN returned an empty username".into());
    }

    Ok(response.query.userinfo.name)
}
