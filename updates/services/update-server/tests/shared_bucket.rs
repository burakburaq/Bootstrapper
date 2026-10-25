use std::collections::{BTreeMap, BTreeSet};
use update_protocol::fixtures::*;
use update_server::{
    config::*,
    storage::{R2Storage, Storage},
};

fn config() -> Config {
    let apps = ["owlligacademy", "owlligcampus", "modnasoft"]
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            let seed = index as u8 + 1;
            AppConfig {
                app_id: id.into(),
                channels: BTreeMap::from([(
                    "production".into(),
                    ChannelConfig {
                        private: false,
                        read_token_sha256: None,
                    },
                )]),
                targets: BTreeSet::from(["windows-x86_64".into()]),
                metadata_keys: keys(seed),
                package_public_key: package_signature(b"package", seed).1,
                bucket: "shared-private".into(),
                health_object_key: format!("{id}/health.txt"),
                admin_tokens: vec![],
            }
        })
        .collect();
    Config {
        public_url: "https://updates.example.com/".parse().unwrap(),
        r2_endpoint: "https://example.r2.cloudflarestorage.com/".parse().unwrap(),
        apps,
        max_artifact_bytes: 1024 * 1024,
        presign_ttl_seconds: 300,
    }
}

#[tokio::test]
async fn shared_bucket_presigns_each_products_own_object_and_rejects_foreign_keys() {
    let config = config();
    config.validate().unwrap();
    let storage = R2Storage::new(&config, "test-access", "test-secret").unwrap();
    for (index, app) in config.apps.iter().enumerate() {
        let b = bundle(
            &app.app_id,
            "production",
            "2.0.0",
            2,
            b"package",
            index as u8 + 1,
        );
        let artifact = &b.artifacts[0].body;
        let url: url::Url = storage
            .presign(app, artifact)
            .await
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            url.path(),
            format!("/shared-private/{}", artifact.object_key)
        );
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "X-Amz-Expires" && value == "300")
        );
        let foreign = &config.apps[(index + 1) % config.apps.len()];
        assert!(storage.presign(foreign, artifact).await.is_err());
        let mut altered = artifact.clone();
        altered.object_key = format!("{}/windows-x86_64/file.package", foreign.app_id);
        assert!(storage.presign(app, &altered).await.is_err());
        altered.object_key = format!("{}/windows-x86_64/../foreign.package", app.app_id);
        assert!(storage.presign(app, &altered).await.is_err());
    }
}

#[test]
fn shared_bucket_still_rejects_duplicate_products_and_foreign_health_objects() {
    let mut config = config();
    config.validate().unwrap();
    config.apps.push(config.apps[0].clone());
    assert!(config.validate().is_err());
    config.apps.pop();
    config.apps[0].health_object_key = "modnasoft/health.txt".into();
    assert!(config.validate().is_err());
}
