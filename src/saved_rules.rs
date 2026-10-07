use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{entities::saved_rule, projects::ReplacementRule, rules::RuleEngine};

#[derive(Serialize)]
pub struct SavedRuleView {
    pub id: String,
    pub name: String,
    pub rule: ReplacementRule,
}

pub fn validate(rule: &ReplacementRule) -> Result<(), String> {
    let name = rule.name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err("findsmith-api-rule-name-must-contain-between-1-and-255-characters".to_owned());
    }
    RuleEngine::compile(std::slice::from_ref(rule)).map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn list(
    database: &DatabaseConnection,
    owner: &str,
) -> Result<Vec<SavedRuleView>, String> {
    let rows = saved_rule::Entity::find()
        .filter(saved_rule::Column::Owner.eq(owner))
        .order_by_asc(saved_rule::Column::Name)
        .all(database)
        .await
        .map_err(|error| error.to_string())?;
    rows.into_iter()
        .map(|row| {
            let rule = serde_json::from_value(row.rule_json).map_err(|error| error.to_string())?;
            Ok(SavedRuleView {
                id: row.id,
                name: row.name,
                rule,
            })
        })
        .collect()
}

pub async fn save(
    database: &DatabaseConnection,
    owner: &str,
    id: Option<&str>,
    mut rule: ReplacementRule,
) -> Result<SavedRuleView, String> {
    validate(&rule)?;
    rule.name = rule.name.trim().to_owned();
    let now = Utc::now();
    let value = serde_json::to_value(&rule).map_err(|error| error.to_string())?;
    let id = if let Some(id) = id {
        let row = saved_rule::Entity::find_by_id(id)
            .filter(saved_rule::Column::Owner.eq(owner))
            .one(database)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "findsmith-api-saved-rule-unavailable".to_owned())?;
        let mut active: saved_rule::ActiveModel = row.into();
        active.name = Set(rule.name.clone());
        active.rule_json = Set(value);
        active.updated_at = Set(now);
        active
            .update(database)
            .await
            .map_err(|error| error.to_string())?;
        id.to_owned()
    } else {
        let id = Uuid::new_v4().to_string();
        saved_rule::ActiveModel {
            id: Set(id.clone()),
            owner: Set(owner.to_owned()),
            name: Set(rule.name.clone()),
            rule_json: Set(value),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .map_err(|error| error.to_string())?;
        id
    };
    Ok(SavedRuleView {
        id,
        name: rule.name.clone(),
        rule,
    })
}

pub async fn delete(database: &DatabaseConnection, owner: &str, id: &str) -> Result<bool, String> {
    Ok(saved_rule::Entity::delete_many()
        .filter(saved_rule::Column::Id.eq(id))
        .filter(saved_rule::Column::Owner.eq(owner))
        .exec(database)
        .await
        .map_err(|error| error.to_string())?
        .rows_affected
        > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_rule_without_name_loads() {
        let rule: ReplacementRule = serde_json::from_str(
            r#"{"enabled":true,"find":"old","replace":"new","regex":false,"case_sensitive":true,"replace_all":true,"multiline":false,"dot_matches_new_line":false}"#,
        ).unwrap();
        assert!(rule.name.is_empty());
    }
}
