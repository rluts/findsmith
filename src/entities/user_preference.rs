use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "user_preferences")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub owner: String,
    pub language: String,
    pub last_wiki_id: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
