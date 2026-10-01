use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Jobs::Table)
                    .if_not_exists()
                    .col(string_len(Jobs::Id, 36).primary_key())
                    .col(string_len(Jobs::ProjectId, 36))
                    .col(string_len(Jobs::Owner, 255))
                    .col(string_len(Jobs::WikiUrl, 2048))
                    .col(json(Jobs::ConfigJson))
                    .col(json(Jobs::StateJson))
                    .col(big_integer(Jobs::StateVersion).default(1))
                    .col(timestamp(Jobs::CreatedAt))
                    .col(timestamp(Jobs::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_jobs_project")
                            .from(Jobs::Table, Jobs::ProjectId)
                            .to(Projects::Table, Projects::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_jobs_owner_project_updated")
                    .table(Jobs::Table)
                    .col(Jobs::Owner)
                    .col(Jobs::ProjectId)
                    .col(Jobs::UpdatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Jobs::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Jobs {
    Table,
    Id,
    ProjectId,
    Owner,
    WikiUrl,
    ConfigJson,
    StateJson,
    StateVersion,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Projects {
    Table,
    Id,
}
