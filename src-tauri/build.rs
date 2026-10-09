fn main() {
    use std::{env, fs, path::PathBuf};
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    for (variable, destination, fallback) in [
        (
            "BAYKUS_INSTALLER_TRUST",
            "trust.json",
            Some("../trust.json"),
        ),
        ("BAYKUS_INSTALLER_HELPER", "update-service.exe", None),
        ("BAYKUS_INSTALLER_CA", "local-ca.pem", None),
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
        let source = env::var_os(variable)
            .map(PathBuf::from)
            .or_else(|| fallback.map(PathBuf::from));
        if let Some(source) = source {
            println!("cargo:rerun-if-changed={}", source.display());
            fs::copy(source, output.join(destination)).expect("embedded installer input");
        } else {
            fs::write(output.join(destination), []).expect("empty installer input");
        }
    }
    println!("cargo:rerun-if-changed=../ui");
    println!("cargo:rerun-if-changed=../build/ui");
    println!("cargo:rerun-if-changed=installer.manifest");
    println!("cargo:rerun-if-changed=../trust.json");
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("installer.manifest")),
    ))
    .expect("installer build configuration failed");
}
