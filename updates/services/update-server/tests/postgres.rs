use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use update_protocol::{fixtures::*, *};
use update_server::repository::{PostgresRepository, Repository};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set UPDATE_TEST_DATABASE_URL"]
async fn postgres_atomicity_concurrency_and_immutable_metadata() {
    let url = std::env::var("UPDATE_TEST_DATABASE_URL").expect("isolated test database URL");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::query("DO $$ BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='update_runtime') THEN CREATE ROLE update_runtime LOGIN PASSWORD 'local-runtime-test'; END IF; END $$")
        .execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("../../../deploy/grants.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let mut runtime_url = url::Url::parse(&url).unwrap();
    runtime_url.set_username("update_runtime").unwrap();
    runtime_url
        .set_password(Some("local-runtime-test"))
        .unwrap();
    let runtime = PgPoolOptions::new()
        .max_connections(10)
        .connect(runtime_url.as_str())
        .await
        .unwrap();
    let repo = Arc::new(PostgresRepository {
        pool: runtime.clone(),
    });
    let app = format!("test-{}", uuid_like());
    let b = bundle(&app, "production", "2.0.0", 2, b"package", 1);
    repo.draft(b.clone()).await.unwrap();
    repo.draft(b).await.unwrap();
    assert!(
        repo.draft(bundle(&app, "production", "3.0.0", 1, b"wrong order", 1))
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM update_audit")
            .execute(&runtime)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE update_audit SET action='rewritten'")
            .execute(&runtime)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("CREATE TABLE runtime_should_not_create(id int)")
            .execute(&runtime)
            .await
            .is_err()
    );
    let action = AdminAction {
        operation_id: "publish-retry".into(),
        action: "publish".into(),
        policy: None,
    };
    let (a, b) = tokio::join!(
        repo.act(&app, "production", "release-2", &action),
        repo.act(&app, "production", "release-2", &action)
    );
    a.unwrap();
    b.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM update_audit WHERE app_id=$1 AND action='publish'",
    )
    .bind(&app)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert!(
        sqlx::query("UPDATE update_releases SET version='9.0.0' WHERE app_id=$1")
            .bind(&app)
            .execute(&pool)
            .await
            .is_err()
    );
    repo.act(
        &app,
        "production",
        "release-2",
        &AdminAction {
            operation_id: "revoke".into(),
            action: "revoke".into(),
            policy: None,
        },
    )
    .await
    .unwrap();
    assert!(
        repo.act(
            &app,
            "production",
            "release-2",
            &AdminAction {
                operation_id: "enable".into(),
                action: "enable".into(),
                policy: None
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        repo.list(&app, "production", "windows-x86_64")
            .await
            .unwrap()[0]
            .lifecycle,
        Lifecycle::Revoked
    );
    assert!(
        repo.act(
            &app,
            "production",
            "release-2",
            &AdminAction {
                operation_id: "publish-retry".into(),
                action: "disable".into(),
                policy: None
            }
        )
        .await
        .is_err()
    );
    sqlx::query("DELETE FROM update_audit WHERE app_id=$1")
        .bind(&app)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM update_operations WHERE app_id=$1")
        .bind(&app)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM update_releases WHERE app_id=$1")
        .bind(&app)
        .execute(&pool)
        .await
        .unwrap();
}
fn uuid_like() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}
