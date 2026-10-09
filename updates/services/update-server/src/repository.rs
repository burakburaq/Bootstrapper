use anyhow::{Result, bail, ensure};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::sync::RwLock;
use update_protocol::{
    AdminAction, Lifecycle, ReleaseBundle, ReleaseRecord, RolloutPolicy, canonical, sha256,
    transition,
};

#[async_trait]
pub trait Repository: Send + Sync {
    async fn list(&self, app: &str, channel: &str, target: &str) -> Result<Vec<ReleaseRecord>>;
    async fn draft(&self, bundle: ReleaseBundle) -> Result<ReleaseRecord>;
    async fn act(
        &self,
        app: &str,
        channel: &str,
        release: &str,
        action: &AdminAction,
    ) -> Result<ReleaseRecord>;
    async fn ready(&self) -> Result<()>;
}

#[derive(Clone)]
pub struct PostgresRepository {
    pub pool: PgPool,
}
fn record(row: &sqlx::postgres::PgRow) -> Result<ReleaseRecord> {
    Ok(ReleaseRecord {
        bundle: serde_json::from_value(row.try_get("bundle")?)?,
        lifecycle: serde_json::from_value(serde_json::Value::String(row.try_get("lifecycle")?))?,
        policy: serde_json::from_value(row.try_get("policy")?)?,
    })
}
fn lifecycle(value: &Lifecycle) -> &'static str {
    match value {
        Lifecycle::Draft => "draft",
        Lifecycle::Published => "published",
        Lifecycle::Disabled => "disabled",
        Lifecycle::Revoked => "revoked",
    }
}
fn apply(record: &mut ReleaseRecord, action: &AdminAction) -> Result<()> {
    if action.action == "rollout" {
        ensure!(
            record.lifecycle != Lifecycle::Revoked,
            "revoked release is terminal"
        );
        let policy = action
            .policy
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("rollout requires policy"))?;
        policy.validate()?;
        ensure!(
            policy.revision == record.policy.revision + 1,
            "stale rollout revision"
        );
        ensure!(
            record.policy.revision == 0 || record.policy.cohort_key == policy.cohort_key,
            "cohort key is immutable after first rollout"
        );
        record.policy = policy.clone();
    } else {
        record.lifecycle = transition(&record.lifecycle, &action.action)?;
    }
    Ok(())
}

#[async_trait]
impl Repository for PostgresRepository {
    async fn list(&self, app: &str, channel: &str, target: &str) -> Result<Vec<ReleaseRecord>> {
        let rows = sqlx::query("SELECT bundle,lifecycle,policy FROM update_releases WHERE app_id=$1 AND channel=$2 AND target=$3")
            .bind(app).bind(channel).bind(target).fetch_all(&self.pool).await?;
        rows.iter().map(record).collect()
    }
    async fn draft(&self, bundle: ReleaseBundle) -> Result<ReleaseRecord> {
        let m = &bundle.manifest.body;
        let value = serde_json::to_value(&bundle)?;
        let fingerprint = sha256(&canonical(&bundle)?);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("catalog:{}/{}", m.app_id, m.channel))
            .execute(&mut *tx)
            .await?;
        let existing=sqlx::query("SELECT bundle,lifecycle,policy FROM update_releases WHERE app_id=$1 AND channel=$2 AND target=$3")
            .bind(&m.app_id).bind(&m.channel).bind(&m.target).fetch_all(&mut *tx).await?;
        for old in existing.iter().map(record) {
            let old = old?;
            let previous = &old.bundle.manifest.body;
            if previous.release_id != m.release_id {
                ensure!(
                    m.version.cmp(&previous.version) == m.sequence.cmp(&previous.sequence),
                    "version and sequence order differ"
                );
            }
        }
        sqlx::query("INSERT INTO update_releases(app_id,channel,target,release_id,version,sequence,bundle,bundle_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING")
            .bind(&m.app_id).bind(&m.channel).bind(&m.target).bind(&m.release_id).bind(m.version.to_string())
            .bind(i64::try_from(m.sequence)?).bind(value).bind(&fingerprint).execute(&mut *tx).await?;
        let row = sqlx::query("SELECT bundle,lifecycle,policy,bundle_hash FROM update_releases WHERE app_id=$1 AND channel=$2 AND target=$3 AND release_id=$4 FOR UPDATE")
            .bind(&m.app_id).bind(&m.channel).bind(&m.target).bind(&m.release_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| anyhow::anyhow!("version/sequence already reserved by another release"))?;
        ensure!(
            row.try_get::<String, _>("bundle_hash")? == fingerprint,
            "immutable release metadata differs"
        );
        let result = record(&row)?;
        sqlx::query("INSERT INTO update_audit(app_id,channel,release_id,operation_id,action) VALUES($1,$2,$3,$4,'draft') ON CONFLICT DO NOTHING")
            .bind(&m.app_id).bind(&m.channel).bind(&m.release_id).bind(format!("draft:{}:{}", m.target, m.release_id)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(result)
    }
    async fn act(
        &self,
        app: &str,
        channel: &str,
        release: &str,
        action: &AdminAction,
    ) -> Result<ReleaseRecord> {
        let mut tx = self.pool.begin().await?;
        // Scoped transaction advisory lock serializes retries and same-operation concurrency.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("catalog:{app}/{channel}"))
            .execute(&mut *tx)
            .await?;
        let lock_key = format!("{app}/{channel}/{}", action.operation_id);
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(lock_key)
            .execute(&mut *tx)
            .await?;
        let request_hash = sha256(&canonical(action)?);
        if let Some(row) = sqlx::query("SELECT request_hash,response FROM update_operations WHERE app_id=$1 AND channel=$2 AND operation_id=$3")
            .bind(app).bind(channel).bind(&action.operation_id).fetch_optional(&mut *tx).await? {
            ensure!(row.try_get::<String,_>("request_hash")? == sha256(format!("{release}:{request_hash}").as_bytes()), "operation ID reused with different request");
            return Ok(serde_json::from_value(row.try_get("response")?)?);
        }
        let row = sqlx::query("SELECT bundle,lifecycle,policy FROM update_releases WHERE app_id=$1 AND channel=$2 AND release_id=$3 FOR UPDATE")
            .bind(app).bind(channel).bind(release).fetch_optional(&mut *tx).await?.ok_or_else(|| anyhow::anyhow!("release not found"))?;
        let mut result = record(&row)?;
        if action.action == "publish" || action.action == "enable" {
            for checkpoint in &result.bundle.manifest.body.required_checkpoints {
                let row = sqlx::query("SELECT bundle,lifecycle,policy FROM update_releases WHERE app_id=$1 AND channel=$2 AND target=$3 AND release_id=$4")
                    .bind(app).bind(channel).bind(&result.bundle.manifest.body.target).bind(&checkpoint.release_id)
                    .fetch_optional(&mut *tx).await?.ok_or_else(||anyhow::anyhow!("checkpoint missing"))?;
                let existing = record(&row)?;
                ensure!(
                    existing.lifecycle == Lifecycle::Published
                        && !existing.policy.paused
                        && existing.bundle.manifest.body.version == checkpoint.version,
                    "checkpoint unavailable"
                );
            }
        }
        apply(&mut result, action)?;
        sqlx::query("UPDATE update_releases SET lifecycle=$4,policy=$5,updated_at=now() WHERE app_id=$1 AND channel=$2 AND release_id=$3")
            .bind(app).bind(channel).bind(release).bind(lifecycle(&result.lifecycle)).bind(serde_json::to_value(&result.policy)?).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO update_operations(app_id,channel,operation_id,request_hash,response) VALUES($1,$2,$3,$4,$5)")
            .bind(app).bind(channel).bind(&action.operation_id).bind(sha256(format!("{release}:{request_hash}").as_bytes()))
            .bind(serde_json::to_value(&result)?).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO update_audit(app_id,channel,release_id,operation_id,action) VALUES($1,$2,$3,$4,$5)")
            .bind(app).bind(channel).bind(release).bind(&action.operation_id).bind(&action.action).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(result)
    }
    async fn ready(&self) -> Result<()> {
        sqlx::query("SELECT app_id FROM update_releases LIMIT 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// Same domain API for isolated HTTP tests; production always uses PostgresRepository.
#[derive(Default)]
pub struct MemoryRepository {
    records: RwLock<Vec<ReleaseRecord>>,
    operations: RwLock<std::collections::BTreeMap<String, (String, ReleaseRecord)>>,
}
#[async_trait]
impl Repository for MemoryRepository {
    async fn list(&self, app: &str, channel: &str, target: &str) -> Result<Vec<ReleaseRecord>> {
        Ok(self
            .records
            .read()
            .await
            .iter()
            .filter(|r| {
                let m = &r.bundle.manifest.body;
                m.app_id == app && m.channel == channel && m.target == target
            })
            .cloned()
            .collect())
    }
    async fn draft(&self, bundle: ReleaseBundle) -> Result<ReleaseRecord> {
        let mut records = self.records.write().await;
        let m = &bundle.manifest.body;
        for existing in records.iter() {
            let e = &existing.bundle.manifest.body;
            if e.app_id == m.app_id && e.channel == m.channel {
                if e.release_id == m.release_id {
                    ensure!(existing.bundle == bundle, "immutable release differs");
                    return Ok(existing.clone());
                }
                ensure!(
                    e.target != m.target || (e.version != m.version && e.sequence != m.sequence),
                    "version/sequence collision"
                );
                if e.target == m.target {
                    ensure!(
                        m.version.cmp(&e.version) == m.sequence.cmp(&e.sequence),
                        "version and sequence order differ"
                    );
                }
            }
        }
        let result = ReleaseRecord {
            bundle,
            lifecycle: Lifecycle::Draft,
            policy: RolloutPolicy::default(),
        };
        records.push(result.clone());
        Ok(result)
    }
    async fn act(
        &self,
        app: &str,
        channel: &str,
        release: &str,
        action: &AdminAction,
    ) -> Result<ReleaseRecord> {
        let mut operations = self.operations.write().await;
        let key = format!("{app}/{channel}/{}", action.operation_id);
        let hash = sha256(format!("{release}:{}", sha256(&canonical(action)?)).as_bytes());
        if let Some((old, result)) = operations.get(&key) {
            ensure!(old == &hash, "operation ID reused");
            return Ok(result.clone());
        }
        let mut records = self.records.write().await;
        let Some(record) = records.iter_mut().find(|r| {
            let m = &r.bundle.manifest.body;
            m.app_id == app && m.channel == channel && m.release_id == release
        }) else {
            bail!("release not found");
        };
        apply(record, action)?;
        let result = record.clone();
        operations.insert(key, (hash, result.clone()));
        Ok(result)
    }
    async fn ready(&self) -> Result<()> {
        Ok(())
    }
}

pub type SharedRepository = Arc<dyn Repository>;
