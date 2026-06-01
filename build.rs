fn main() {
    println!("cargo:rerun-if-env-changed=CARABINER_VERSION");
    println!("cargo:rerun-if-env-changed=CARABINER_GETTEXT_PACKAGE");
    println!("cargo:rerun-if-env-changed=CARABINER_LOCALEDIR");
    println!("cargo:rerun-if-env-changed=CARABINER_PKGDATADIR");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = std::path::Path::new(&out_dir).join("config.rs");

    let version = std::env::var("CARABINER_VERSION").unwrap_or_else(|_| "unknown".into());
    let gettext_package =
        std::env::var("CARABINER_GETTEXT_PACKAGE").unwrap_or_else(|_| "carabiner".into());
    let localedir =
        std::env::var("CARABINER_LOCALEDIR").unwrap_or_else(|_| "/usr/share/locale".into());
    let pkgdatadir =
        std::env::var("CARABINER_PKGDATADIR").unwrap_or_else(|_| "/usr/share/carabiner".into());

    let config = format!(
        r#"pub static VERSION: &str = "{version}";
pub static GETTEXT_PACKAGE: &str = "{gettext_package}";
pub static LOCALEDIR: &str = "{localedir}";
pub static PKGDATADIR: &str = "{pkgdatadir}";
"#,
    );

    std::fs::write(dest_path, config).unwrap();
}
