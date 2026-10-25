fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let version = std::env::var("CARGO_PKG_VERSION").expect("package version");
    let numeric = version
        .split(['-', '+'])
        .next()
        .expect("version")
        .replace('.', ",")
        + ",0";
    let resource = format!(
        r#"#pragma code_page(65001)
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0
FILEOS 0x40004L
FILETYPE 1
FILESUBTYPE 0
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904B0"
  BEGIN
   VALUE "CompanyName", "TalayLabs\0"
   VALUE "ProductName", "TalayLabs Update Service\0"
   VALUE "FileDescription", "Product-scoped installation and update service\0"
   VALUE "ProductVersion", "{version}\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "OriginalFilename", "update-service.exe\0"
   VALUE "LegalCopyright", "© 2026 TalayLabs\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x409, 1200
 END
END
"#
    );
    let path = out.join("service-version.rc");
    std::fs::write(&path, resource).expect("service VERSIONINFO");
    embed_resource::compile_for(&path, ["update-service"], embed_resource::NONE)
        .manifest_required()
        .expect("service version resource compilation failed");
    println!("cargo:rerun-if-changed=build.rs");
}
