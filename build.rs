//! Compiles `data/resources.gresource.xml` (icons) into `$OUT_DIR`, so the
//! binary carries them and runs from anywhere without being installed.

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
}
