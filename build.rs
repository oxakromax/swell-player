//! Compiles `data/resources.gresource.xml` (icons) into `$OUT_DIR`, so the
//! binary carries them and runs from anywhere without being installed.
//!
//! For Windows (the GNU target, as built in MSYS2) it also links a resource
//! with the version information Explorer and Task Manager show, and the
//! window/Explorer icon from `packaging/windows/swell.ico` when that file
//! exists (`packaging/windows/make-ico.sh` makes it; the release workflow runs
//! it before building). The resource is compiled with binutils' `windres`
//! (`$WINDRES`, else `<arch>-w64-mingw32-windres`, else `windres`), so no
//! extra crate is needed and other targets are unaffected.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=data/resources.gresource.xml");
    println!("cargo::rerun-if-changed=data/icons");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo");
    let status = Command::new("glib-compile-resources")
        .arg("--sourcedir=data")
        .arg(format!("--target={out_dir}/swell.gresource"))
        .arg("data/resources.gresource.xml")
        .status()
        .expect(
            "glib-compile-resources (part of GLib's development tools) is needed to build Swell",
        );
    assert!(status.success(), "glib-compile-resources failed");

    // The target's cfg, not the build script's own (that is the host's).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource(Path::new(&out_dir));
    }
}

fn windows_resource(out_dir: &Path) {
    println!("cargo::rerun-if-changed=packaging/windows");
    println!("cargo::rerun-if-env-changed=WINDRES");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        println!(
            "cargo::warning=no Windows resource (icon, version) for this toolchain: only the GNU target (MSYS2) is supported"
        );
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let ico = manifest_dir.join("packaging/windows/swell.ico");

    let version = std::env::var("CARGO_PKG_VERSION").expect("set by Cargo");
    let mut parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    let numeric = parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");

    let mut rc = String::new();
    if ico.is_file() {
        // windres reads C-style strings: forward slashes need no escaping.
        let path = ico.to_string_lossy().replace('\\', "/");
        rc.push_str(&format!("1 ICON \"{path}\"\n"));
    } else {
        println!(
            "cargo::warning=packaging/windows/swell.ico is missing, so swell-player.exe has no icon (make it with packaging/windows/make-ico.sh)"
        );
    }
    rc.push_str(&format!(
        r#"1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Swell contributors"
      VALUE "FileDescription", "Swell"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "swell-player"
      VALUE "LegalCopyright", "GPL-3.0-or-later"
      VALUE "OriginalFilename", "swell-player.exe"
      VALUE "ProductName", "Swell"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    ));
    let rc_file = out_dir.join("swell.rc");
    std::fs::write(&rc_file, rc).expect("writing swell.rc");
    let object = out_dir.join("swell-resource.o");

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let candidates: Vec<String> = std::env::var("WINDRES")
        .ok()
        .into_iter()
        .chain([format!("{arch}-w64-mingw32-windres"), "windres".to_owned()])
        .collect();
    for windres in &candidates {
        let result = Command::new(windres)
            .arg("--input-format=rc")
            .arg("--output-format=coff")
            .arg("-i")
            .arg(&rc_file)
            .arg("-o")
            .arg(&object)
            .status();
        match result {
            Ok(status) if status.success() => {
                // A plain object, not an archive: the linker keeps it whole.
                println!("cargo::rustc-link-arg-bins={}", object.display());
                return;
            }
            Ok(status) => panic!("{windres} failed ({status}) on {}", rc_file.display()),
            Err(_) => continue, // not installed under this name
        }
    }
    println!(
        "cargo::warning=windres not found ({}): swell-player.exe gets no icon or version information",
        candidates.join(", ")
    );
}
