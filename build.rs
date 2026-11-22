//! Embeds Windows PE version metadata (`ProductName`, `FileDescription`, `CompanyName`,
//! `LegalCopyright`, `FileVersion`, `OriginalFilename`) and `assets/micropdf.ico` into
//! `micro_pdf.exe` via `winresource` on Windows builds (reduces AV false positives and shows
//! crisp identity in Windows Explorer & Task Manager). Zero impact on Linux/macOS builds.

fn main() {
    println!("cargo:rerun-if-changed=assets/micropdf.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        let pkg_version = env!("CARGO_PKG_VERSION");
        let pkg_desc = env!("CARGO_PKG_DESCRIPTION");

        res.set("ProductName", "MicroPDF");
        res.set("FileDescription", pkg_desc);
        res.set("CompanyName", "azam-data");
        res.set("LegalCopyright", "Copyright (c) azam-data");
        res.set("FileVersion", pkg_version);
        res.set("ProductVersion", pkg_version);
        res.set("OriginalFilename", "micro_pdf.exe");
        res.set("InternalName", "micro_pdf");

        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let ico_path = manifest_dir.join("assets").join("micropdf.ico");
        if ico_path.exists() {
            res.set_icon(&ico_path.to_string_lossy());
        }

        res.compile()
            .expect("Failed to compile Windows version metadata and icon resource");
    }
}
