use crate::{
    ArtifactKind, CheckRequest, Lifecycle, ProtocolError, ReleaseRecord, Result, RolloutPolicy,
};
use sha2::{Digest, Sha256};

pub fn eligible(app_id: &str, policy: &RolloutPolicy, installation: Option<&str>) -> bool {
    if policy.paused || policy.percentage == 0 {
        return false;
    }
    if policy.percentage == 100 {
        return true;
    }
    let Some(id) = installation else {
        return false;
    };
    let bytes = Sha256::digest(format!("{app_id}\0{}\0{id}", policy.cohort_key));
    let cohort = u64::from_be_bytes(bytes[..8].try_into().expect("hash length")) % 10_000;
    cohort < u64::from(policy.percentage) * 100
}

pub fn transition(current: &Lifecycle, action: &str) -> Result<Lifecycle> {
    use Lifecycle::*;
    match (current, action) {
        (Draft | Published, "publish") => Ok(Published),
        (Published | Disabled, "disable") => Ok(Disabled),
        (Disabled | Published, "enable") => Ok(Published),
        (_, "revoke") => Ok(Revoked),
        _ => Err(ProtocolError::Invalid(
            "invalid lifecycle transition".into(),
        )),
    }
}

/// Finds the first required checkpoint before the newest eligible destination.
pub fn resolve<'a>(
    records: &'a [ReleaseRecord],
    request: &CheckRequest,
) -> Result<Option<(&'a ReleaseRecord, String)>> {
    request.validate()?;
    let mut candidates: Vec<_> = records
        .iter()
        .filter(|r| {
            let m = &r.bundle.manifest.body;
            r.lifecycle == Lifecycle::Published
                && m.app_id == request.app_id
                && m.channel == request.channel
                && m.target == request.target
                && m.version > request.current_version
                && eligible(
                    &request.app_id,
                    &r.policy,
                    request.installation_id.as_deref(),
                )
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.bundle
            .manifest
            .body
            .version
            .cmp(&a.bundle.manifest.body.version)
    });
    let Some(mut chosen) = candidates.first().copied() else {
        return Ok(None);
    };
    let mut visited = std::collections::BTreeSet::new();
    loop {
        let m = &chosen.bundle.manifest.body;
        if !visited.insert(&m.release_id) {
            return Err(ProtocolError::Invalid("checkpoint cycle".into()));
        }
        if let Some(checkpoint) = m
            .required_checkpoints
            .iter()
            .find(|c| c.version > request.current_version)
        {
            chosen = records
                .iter()
                .find(|r| {
                    let cm = &r.bundle.manifest.body;
                    cm.app_id == m.app_id
                        && cm.channel == m.channel
                        && cm.target == m.target
                        && cm.release_id == checkpoint.release_id
                        && cm.version == checkpoint.version
                        && r.lifecycle == Lifecycle::Published
                        && !r.policy.paused
                })
                .ok_or_else(|| ProtocolError::Invalid("required checkpoint unavailable".into()))?;
            // Required checkpoints inherit the final destination's cohort; they cannot be skipped.
            continue;
        }
        if m.minimum_supported_version
            .as_ref()
            .is_some_and(|v| &request.current_version < v)
        {
            return Err(ProtocolError::Invalid(
                "current version cannot upgrade directly".into(),
            ));
        }
        break;
    }
    let m = &chosen.bundle.manifest.body;
    let full = chosen
        .bundle
        .artifact(&m.full_artifact_id)
        .ok_or_else(|| ProtocolError::Invalid("missing full package".into()))?;
    let selected = if request.supports_delta {
        chosen
            .bundle
            .artifacts
            .iter()
            .filter(|a| match &a.body.artifact {
                ArtifactKind::Delta {
                    base_sha256,
                    base_size_bytes,
                    ..
                } => {
                    request.base_sha256.as_ref() == Some(base_sha256)
                        && request.base_size_bytes == Some(*base_size_bytes)
                        && a.body.size_bytes < full.size_bytes
                }
                _ => false,
            })
            .min_by_key(|a| a.body.size_bytes)
            .map(|a| a.body.artifact_id.clone())
    } else {
        None
    };
    Ok(Some((
        chosen,
        selected.unwrap_or_else(|| m.full_artifact_id.clone()),
    )))
}
