use sea_orm_migration::prelude::*;

mod m20260908_000001_create_projects;
mod m20261001_000002_create_jobs;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260908_000001_create_projects::Migration),
            Box::new(m20261001_000002_create_jobs::Migration),
        ]
    }
}
