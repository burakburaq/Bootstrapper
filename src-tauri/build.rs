fn main() {
    use std::{env, fs, path::PathBuf};
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    for name in ["product.rs", "trust.json"] {
        let source = PathBuf::from("../build").join(name);
        println!("cargo:rerun-if-changed={}", source.display());
        fs::copy(&source, output.join(name))
            .expect("Run npm run build before Cargo; missing generated installer configuration");
    }
    println!("cargo:rerun-if-env-changed=BAYKUS_INSTALLER_CA");
    let ca = env::var_os("BAYKUS_INSTALLER_CA").map(PathBuf::from);
    if let Some(ca) = ca {
        println!("cargo:rerun-if-changed={}", ca.display());
        fs::copy(ca, output.join("local-ca.pem")).expect("embedded installer CA");
    } else {
        fs::write(output.join("local-ca.pem"), []).expect("empty local CA");
    }
    println!("cargo:rerun-if-changed=../build/ui");
    println!("cargo:rerun-if-changed=../build/application.ico");
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    println!("cargo:rerun-if-changed=installer.manifest");
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("installer.manifest")),
    ))
    .expect("installer build configuration failed");
}
