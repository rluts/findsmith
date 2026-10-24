use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserPreferences::Table)
                    .add_column_if_not_exists(string_len_null(UserPreferences::LastWikiId, 100))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserPreferences::Table)
                    .drop_column(UserPreferences::LastWikiId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum UserPreferences {
    Table,
    LastWikiId,
}
