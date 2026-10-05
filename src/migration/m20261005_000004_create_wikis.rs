use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Wikis::Table)
                    .if_not_exists()
                    .col(string_len(Wikis::Id, 100).primary_key())
                    .col(string_len(Wikis::Domain, 255).unique_key())
                    .col(string_len(Wikis::ApiUrl, 512).unique_key())
                    .col(string_len(Wikis::Name, 255))
                    .col(string_len(Wikis::CheckPage, 512))
                    .col(json(Wikis::NamespacesJson))
                    .col(timestamp(Wikis::CreatedAt))
                    .col(timestamp(Wikis::UpdatedAt))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Wikis::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Wikis {
    Table,
    Id,
    Domain,
    ApiUrl,
    Name,
    CheckPage,
    NamespacesJson,
    CreatedAt,
    UpdatedAt,
}
