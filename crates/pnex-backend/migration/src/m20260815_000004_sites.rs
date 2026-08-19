//! Sites — PK UUID (parité Django sites), FK uuid→uuid entre tables,
//! scoping org_id (D2). Annotation.linked_devices reste un JSON dénormalisé
//! sans FK (lien faible assumé côté Django, conservé).
//!
//! Portabilité sqlite (tier hobbyist) : `gen_random_uuid()` n'existe que
//! côté PG — le défaut DB n'est posé que sur ce backend ; sur sqlite l'UUID
//! doit être fourni par l'app (`Uuid::new_v4()` à l'insert). Note : déjà
//! appliquée sur les dev PG, cette édition ne joue que sur des bases neuves
//! (schéma PG identique de toute façon).

#![allow(dead_code)]
use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

/// PK UUID avec défaut `gen_random_uuid()` sur PG uniquement (fonction
/// serveur absente de sqlite).
fn uuid_pk(m: &SchemaManager, iden: impl IntoIden) -> ColumnDef {
    let mut binding = uuid(iden);
    let col = binding.primary_key();
    if matches!(m.get_database_backend(), sea_orm::DatabaseBackend::Postgres) {
        col.default(Expr::cust("gen_random_uuid()")).take()
    } else {
        col.take()
    }
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Iden)]
enum Site {
    Table,
    Id,
    OrgId,
    Name,
    Description,
    Latitude,
    Longitude,
    Address,
    Tags,
    Metadata,
    DefaultZoom,
    DefaultPanX,
    DefaultPanY,
}
#[derive(Iden)]
enum SvgFile {
    Table,
    Id,
    OrgId,
    Filename,
    Name,
    Content,
    Tags,
    Metadata,
}
#[derive(Iden)]
enum SiteDiagram {
    Table,
    Id,
    SiteId,
    SvgFileId,
    DisplayName,
    DisplayOrder,
    Metadata,
}
#[derive(Iden)]
enum Annotation {
    Table,
    Id,
    SiteDiagramId,
    X,
    Y,
    Title,
    Fields,
    LinkedDevices,
    Zoom,
    PanX,
    PanY,
}
#[derive(Iden)]
enum SavedView {
    Table,
    Id,
    SiteDiagramId,
    Name,
    Zoom,
    PanX,
    PanY,
    Tags,
}
#[derive(Iden)]
enum Organization {
    Table,
    Id,
}

fn timestamps(mut t: TableCreateStatement) -> TableCreateStatement {
    t.col(timestamp_with_time_zone(Alias::new("created_at")).default(Expr::current_timestamp()))
        .col(timestamp_with_time_zone(Alias::new("updated_at")).default(Expr::current_timestamp()));
    t.take()
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let mut sites = Table::create();
        sites
            .table(Alias::new("sites"))
            .if_not_exists()
            .col(uuid_pk(m, Site::Id))
            .col(big_integer(Site::OrgId).not_null())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-sites-org_id-to-organizations")
                    .from(Alias::new("sites"), Site::OrgId)
                    .to(Alias::new("organizations"), Organization::Id)
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .col(string_len(Site::Name, 255).not_null())
            .col(text_null(Site::Description))
            .col(decimal_len_null(Site::Latitude, 9, 6))
            .col(decimal_len_null(Site::Longitude, 9, 6))
            .col(text_null(Site::Address))
            .col(json_binary_null(Site::Tags))
            .col(json_binary_null(Site::Metadata))
            .col(decimal_len(Site::DefaultZoom, 5, 2).not_null().default(1.0))
            .col(
                decimal_len(Site::DefaultPanX, 10, 2)
                    .not_null()
                    .default(0.0),
            )
            .col(
                decimal_len(Site::DefaultPanY, 10, 2)
                    .not_null()
                    .default(0.0),
            );
        m.create_table(timestamps(sites.take())).await?;

        let mut svg = Table::create();
        svg.table(Alias::new("svg_files"))
            .if_not_exists()
            .col(uuid_pk(m, SvgFile::Id))
            .col(big_integer(SvgFile::OrgId).not_null())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-svg_files-org_id-to-organizations")
                    .from(Alias::new("svg_files"), SvgFile::OrgId)
                    .to(Alias::new("organizations"), Organization::Id)
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .col(string_len(SvgFile::Filename, 255).not_null())
            .col(string_len(SvgFile::Name, 255).not_null())
            .col(text(SvgFile::Content).not_null())
            .col(json_binary_null(SvgFile::Tags))
            .col(json_binary_null(SvgFile::Metadata));
        m.create_table(timestamps(svg.take())).await?;

        let mut diagrams = Table::create();
        diagrams
            .table(Alias::new("site_diagrams"))
            .if_not_exists()
            .col(uuid_pk(m, SiteDiagram::Id))
            .col(uuid(SiteDiagram::SiteId).not_null())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-site_diagrams-site_id-to-sites")
                    .from(Alias::new("site_diagrams"), SiteDiagram::SiteId)
                    .to(Alias::new("sites"), Site::Id)
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .col(uuid(SiteDiagram::SvgFileId).not_null())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-site_diagrams-svg_file_id-to-svg_files")
                    .from(Alias::new("site_diagrams"), SiteDiagram::SvgFileId)
                    .to(Alias::new("svg_files"), SvgFile::Id)
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .col(string_len(SiteDiagram::DisplayName, 255).not_null())
            .col(integer(SiteDiagram::DisplayOrder).not_null().default(0))
            .col(json_binary_null(SiteDiagram::Metadata));
        m.create_table(timestamps(diagrams.take())).await?;

        // linked_devices JSON SANS FK (lien dénormalisé voulu)
        let mut ann = Table::create();
        ann.table(Alias::new("annotations"))
            .if_not_exists()
            .col(uuid_pk(m, Annotation::Id))
            .col(uuid(Annotation::SiteDiagramId).not_null())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-annotations-site_diagram_id-to-site_diagrams")
                    .from(Alias::new("annotations"), Annotation::SiteDiagramId)
                    .to(Alias::new("site_diagrams"), SiteDiagram::Id)
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .col(decimal_len(Annotation::X, 10, 2).not_null())
            .col(decimal_len(Annotation::Y, 10, 2).not_null())
            .col(string_len(Annotation::Title, 255).not_null())
            .col(json_binary_null(Annotation::Fields))
            .col(json_binary_null(Annotation::LinkedDevices))
            .col(decimal_len_null(Annotation::Zoom, 5, 2))
            .col(decimal_len_null(Annotation::PanX, 10, 2))
            .col(decimal_len_null(Annotation::PanY, 10, 2));
        m.create_table(timestamps(ann.take())).await?;

        let mut views = Table::create();
        views
            .table(Alias::new("saved_views"))
            .if_not_exists()
            .col(uuid_pk(m, SavedView::Id))
            .col(uuid(SavedView::SiteDiagramId).not_null())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-saved_views-site_diagram_id-to-site_diagrams")
                    .from(Alias::new("saved_views"), SavedView::SiteDiagramId)
                    .to(Alias::new("site_diagrams"), SiteDiagram::Id)
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .col(string_len(SavedView::Name, 255).not_null())
            .col(decimal_len(SavedView::Zoom, 5, 2).not_null().default(1.0))
            .col(decimal_len(SavedView::PanX, 10, 2).not_null().default(0.0))
            .col(decimal_len(SavedView::PanY, 10, 2).not_null().default(0.0))
            .col(json_binary_null(SavedView::Tags));
        m.create_table(timestamps(views.take())).await?;

        // Un statement par appel : sqlite n'accepte pas les batches implicites
        // via execute_unprepared.
        for stmt in [
            "CREATE UNIQUE INDEX uniq_site_diagrams_site_svg ON site_diagrams (site_id, svg_file_id)",
            "CREATE INDEX idx_sites_org_name ON sites (org_id, name)",
            "CREATE INDEX idx_site_diagrams_site_order ON site_diagrams (site_id, display_order)",
            "CREATE INDEX idx_saved_views_diagram_created ON saved_views (site_diagram_id, created_at)",
        ] {
            m.get_connection().execute_unprepared(stmt).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.drop_table(Table::drop().table(Alias::new("saved_views")).to_owned())
            .await?;
        m.drop_table(Table::drop().table(Alias::new("annotations")).to_owned())
            .await?;
        m.drop_table(Table::drop().table(Alias::new("site_diagrams")).to_owned())
            .await?;
        m.drop_table(Table::drop().table(Alias::new("svg_files")).to_owned())
            .await?;
        m.drop_table(Table::drop().table(Alias::new("sites")).to_owned())
            .await?;
        Ok(())
    }
}
