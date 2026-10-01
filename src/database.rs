use std::{env, path::PathBuf, time::Duration};

use configparser::ini::Ini;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::{AppResult, config::DatabaseConfig, migration::Migrator};

pub async fn connect(config: DatabaseConfig) -> AppResult<DatabaseConnection> {
    let (user, password) = credentials(&config)?;
    validate_database_name(&config.name, &user)?;
    let url = format!(
        "mysql://{}:{}@{}:{}/{}",
        encode(&user),
        encode(&password),
        config.host,
        config.port,
        config.name
    );
    let mut options = ConnectOptions::new(url);
    options
        .max_connections(5)
        .min_connections(0)
        .connect_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(60))
        .sqlx_logging(false);
    let database = Database::connect(options).await?;
    Migrator::up(&database, None).await?;
    Ok(database)
}

fn validate_database_name(name: &str, user: &str) -> AppResult<()> {
    let required_prefix = format!("{user}__");
    if !name.starts_with(&required_prefix) || name.len() == required_prefix.len() {
        return Err(
            "DATABASE_NAME must start with the ToolsDB credential user followed by two underscores and a database suffix"
                .into(),
        );
    }
    Ok(())
}

fn credentials(config: &DatabaseConfig) -> AppResult<(String, String)> {
    match (&config.user, &config.password) {
        (Some(user), Some(password)) => return Ok((user.clone(), password.clone())),
        (Some(_), None) | (None, Some(_)) => {
            return Err("TOOL_TOOLSDB_USER and TOOL_TOOLSDB_PASSWORD must be set together".into());
        }
        (None, None) => {}
    }

    let path = config
        .credentials_file
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(default_credentials_path);
    let mut ini = Ini::new();
    ini.load(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let user = ini
        .get("client", "user")
        .ok_or("replica.my.cnf is missing client.user")?;
    let password = ini
        .get("client", "password")
        .ok_or("replica.my.cnf is missing client.password")?;
    Ok((user, password))
}

fn default_credentials_path() -> PathBuf {
    if let Ok(data_dir) = env::var("TOOL_DATA_DIR") {
        return PathBuf::from(data_dir).join("replica.my.cnf");
    }
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_owned())).join("replica.my.cnf")
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_toolsdb_name_with_credential_prefix() {
        assert!(validate_database_name("s12345__findsmith", "s12345").is_ok());
    }

    #[test]
    fn rejects_missing_or_empty_database_suffix() {
        assert!(validate_database_name("findsmith", "s12345").is_err());
        assert!(validate_database_name("s12345__", "s12345").is_err());
    }
}
