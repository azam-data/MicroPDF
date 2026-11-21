//! Embeds `assets/micropdf.ico` into the Windows executable resource table so File Explorer,
//! shortcuts, and the Windows taskbar display the MicroPDF icon (zero extra crates).

fn main() {
    println!("cargo:rerun-if-changed=assets/micropdf.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico_path = manifest_dir.join("assets").join("micropdf.ico");
    if !ico_path.exists() {
        return;
    }

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join("micropdf_icon.rc");
    let res_path = out_dir.join("micropdf_icon.res");

    let ico_escaped = ico_path.to_string_lossy().replace('\\', "\\\\");
    let rc_content = format!("1 ICON \"{}\"\n", ico_escaped);
    if std::fs::write(&rc_path, rc_content).is_err() {
        return;
    }

    // Try MSVC rc.exe (default on windows-latest), then locate Windows Kits rc.exe, then windres.
    let mut compiled = try_run_rc("rc.exe", &res_path, &rc_path);
    if !compiled {
        if let Some(sdk_rc) = find_windows_sdk_rc() {
            compiled = try_run_rc(&sdk_rc, &res_path, &rc_path);
        }
    }
    if !compiled {
        compiled = std::process::Command::new("windres")
            .arg(&rc_path)
            .arg("-O")
            .arg("coff")
            .arg("-o")
            .arg(&res_path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }

    if compiled && res_path.exists() {
        println!("cargo:rustc-link-arg={}", res_path.display());
    }
}

fn try_run_rc(program: &str, res_path: &std::path::Path, rc_path: &std::path::Path) -> bool {
    std::process::Command::new(program)
        .arg("/nologo")
        .arg("/fo")
        .arg(res_path)
        .arg(rc_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn find_windows_sdk_rc() -> Option<String> {
    let base = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let entries = std::fs::read_dir(base).ok()?;
    let mut versions: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    for ver in versions.into_iter().rev() {
        for arch in ["x64", "x86", "arm64"] {
            let candidate = ver.join(arch).join("rc.exe");
            if candidate.exists() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}
