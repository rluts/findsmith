use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SavedRules::Table)
                    .if_not_exists()
                    .col(string_len(SavedRules::Id, 36).primary_key())
                    .col(string_len(SavedRules::Owner, 255))
                    .col(string_len(SavedRules::Name, 255))
                    .col(json(SavedRules::RuleJson))
                    .col(timestamp(SavedRules::CreatedAt))
                    .col(timestamp(SavedRules::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_saved_rules_owner_name")
                    .table(SavedRules::Table)
                    .col(SavedRules::Owner)
                    .col(SavedRules::Name)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SavedRules::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SavedRules {
    Table,
    Id,
    Owner,
    Name,
    RuleJson,
    CreatedAt,
    UpdatedAt,
}
