use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Wikis::Table)
                    .add_column_if_not_exists(json(Wikis::InUseTemplatesJson).default("[]"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Wikis::Table)
                    .drop_column(Wikis::InUseTemplatesJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Wikis {
    Table,
    InUseTemplatesJson,
}
