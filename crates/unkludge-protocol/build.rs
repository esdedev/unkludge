//! Compiles `data/models/*.toml` into `$OUT_DIR/models.rs`.

use std::{env, fs, path::PathBuf, process};

#[path = "build/models.rs"]
mod models;

fn main() {
    if let Err(message) = generate() {
        eprintln!("error: data/models: {message}");
        process::exit(1);
    }
}

fn generate() -> Result<(), String> {
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("no CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("no OUT_DIR")?);
    let root = package
        .ancestors()
        .nth(2)
        .ok_or("the package isn't inside the workspace")?;
    let data = root.join("data").join("models");

    // Naming the directory makes Cargo look for changed, new and removed files in it. Naming
    // anything also turns off Cargo's default of watching the package, so the scripts are listed.
    println!("cargo:rerun-if-changed={}", data.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/models.rs");

    let entries = fs::read_dir(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let mut parsed = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("{}: {e}", data.display()))?
            .path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
        parsed.push(models::parse_model(&file, &text)?);
    }

    let code = models::render(parsed)?;
    let out = out_dir.join("models.rs");
    fs::write(&out, code).map_err(|e| format!("{}: {e}", out.display()))
}
