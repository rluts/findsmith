use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Projects::Table)
                    .if_not_exists()
                    .col(string_len(Projects::Id, 36).primary_key())
                    .col(string_len(Projects::Owner, 255))
                    .col(string_len(Projects::Name, 255))
                    .col(string_len(Projects::WikiUrl, 2048))
                    .col(json(Projects::StateJson))
                    .col(big_integer(Projects::StateVersion).default(1))
                    .col(timestamp(Projects::CreatedAt))
                    .col(timestamp(Projects::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_projects_owner_updated")
                    .table(Projects::Table)
                    .col(Projects::Owner)
                    .col(Projects::UpdatedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq_projects_owner_name")
                    .table(Projects::Table)
                    .col(Projects::Owner)
                    .col(Projects::Name)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Projects::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Projects {
    Table,
    Id,
    Owner,
    Name,
    WikiUrl,
    StateJson,
    StateVersion,
    CreatedAt,
    UpdatedAt,
}
