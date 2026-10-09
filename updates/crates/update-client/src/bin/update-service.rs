fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let mut args = std::env::args().skip(1);
        match args.next().as_deref() {
            Some("run") if args.next().is_none() => {
                update_client::windows::service::run_registered_service()
            }
            Some("uninstall") if args.next().is_none() => {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?
                    .block_on(update_client::windows::uninstall_product())
            }
            Some("register") => {
                let path = args.next().ok_or_else(|| {
                    anyhow::anyhow!("register requires a developer-provided policy JSON")
                })?;
                anyhow::ensure!(args.next().is_none(), "unexpected arguments");
                let policy = serde_json::from_slice(&std::fs::read(path)?)?;
                update_client::windows::service::register_service(policy)
            }
            _ => anyhow::bail!("usage: update-service run | register <policy.json> | uninstall"),
        }
    }
    #[cfg(not(windows))]
    anyhow::bail!("update-service is supported on Windows only")
}
