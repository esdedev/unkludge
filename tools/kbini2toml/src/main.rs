//! Generates `data/models/<PID>.toml` from an installed copy of the RK software.
//!
//! Usage: `kbini2toml <RK dir> <out dir>`. Reads `Cfg.ini` and `Dev/<PID>/KB.ini`, and only writes
//! when every model passes validation. The output is sorted, so regenerating gives no diff.

mod ini;
mod model;

use ini::Ini;
use model::Model;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [rk_dir, out_dir] = args.as_slice() else {
        eprintln!("usage: kbini2toml <RK dir> <out dir>");
        return ExitCode::from(2);
    };
    match generate(Path::new(rk_dir), Path::new(out_dir)) {
        Ok(report) => {
            for warning in &report.warnings {
                println!("warning: {warning}");
            }
            println!("{}", report.summary);
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in &errors {
                eprintln!("error: {error}");
            }
            ExitCode::FAILURE
        }
    }
}

struct Report {
    warnings: Vec<String>,
    summary: String,
}

fn read_ini(path: &Path, warnings: &mut Vec<String>) -> Result<Ini, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (text, replaced) = ini::decode(&bytes);
    if replaced {
        warnings.push(format!(
            "{}: not valid UTF-16 or UTF-8, bytes replaced",
            path.display()
        ));
    }
    Ok(Ini::parse(&text))
}

fn generate(rk_dir: &Path, out_dir: &Path) -> Result<Report, Vec<String>> {
    let mut warnings = Vec::new();
    let cfg = read_ini(&rk_dir.join("Cfg.ini"), &mut warnings).map_err(|e| vec![e])?;
    let names = model::parse_names(&cfg);

    let mut models = Vec::new();
    let mut unknown: BTreeMap<String, Vec<u16>> = BTreeMap::new();
    let mut errors = Vec::new();
    for (folder, path) in model_files(rk_dir).map_err(|e| vec![e])? {
        // Folder names aren't consistent in case: both 01FA and 006b exist.
        let Ok(pid) = u16::from_str_radix(&folder, 16) else {
            errors.push(format!("{folder}: folder name isn't a PID"));
            continue;
        };
        let parsed = read_ini(&path, &mut warnings)
            .and_then(|ini| model::parse_model(pid, model::model_name(pid, &names), &ini));
        match parsed {
            Ok(parsed) => {
                for key in parsed.unknown {
                    unknown.entry(key).or_default().push(pid);
                }
                warnings.extend(parsed.warnings.iter().map(|w| format!("{pid:04X}: {w}")));
                models.push(parsed.model);
            }
            Err(error) => errors.push(format!("{pid:04X}: {error}")),
        }
    }
    models.sort_by_key(|model| model.pid);
    errors.extend(model::validate(&models));
    if !errors.is_empty() {
        return Err(errors);
    }

    std::fs::create_dir_all(out_dir).map_err(|e| vec![format!("{}: {e}", out_dir.display())])?;
    for model in &models {
        let path = out_dir.join(format!("{:04X}.toml", model.pid));
        std::fs::write(&path, model::to_toml(model))
            .map_err(|e| vec![format!("{}: {e}", path.display())])?;
    }

    for (key, pids) in &unknown {
        warnings.push(format!(
            "unknown key {key} in {} models (first {:04X})",
            pids.len(),
            pids[0]
        ));
    }
    Ok(Report {
        summary: summary(&models, &unknown),
        warnings,
    })
}

/// Folders of `Dev` that hold a `KB.ini`, as (folder name, file), sorted by name. Other folders
/// are language resources.
fn model_files(rk_dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let dev = rk_dir.join("Dev");
    let read_error = |e: std::io::Error| format!("{}: {e}", dev.display());
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dev).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        let path = entry.path().join("KB.ini");
        if path.is_file() {
            files.push((entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    files.sort();
    Ok(files)
}

fn summary(models: &[Model], unknown: &BTreeMap<String, Vec<u16>>) -> String {
    let count = |fw: u32| models.iter().filter(|m| m.fw == fw).count();
    let screens = models
        .iter()
        .filter(|m| m.fw == 1 && m.screen.is_some())
        .count();
    let unknown: Vec<String> = unknown
        .iter()
        .map(|(key, pids)| format!("{key} ({})", pids.len()))
        .collect();
    let unknown = if unknown.is_empty() {
        "none".to_string()
    } else {
        unknown.join(", ")
    };
    format!(
        "models: {}, fw0 (v1): {}, fw1 (v2): {}, fw1 with screen: {screens}, fw2 (v2alt): {}, unknown keys: {unknown}",
        models.len(),
        count(0),
        count(1),
        count(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A throwaway RK folder, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("kbini2toml-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("Dev")).unwrap();
            TempDir(dir)
        }

        fn add_model(&self, folder: &str, text: &str) {
            let dir = self.0.join("Dev").join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("KB.ini"), text).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn utf16(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn writes_one_uppercase_file_per_kb_ini_and_is_repeatable() {
        let rk = TempDir::new("writes");
        fs::write(
            rk.0.join("Cfg.ini"),
            utf16("[OPT]\r\nDevName1=006b,RK Lower Keyboard\r\n"),
        )
        .unwrap();
        rk.add_model(
            "006b",
            "[OPT]\r\nFw=1\r\n[KEY]\r\n;Esc\r\nK1=1,2,3,4,2,0x1B,0,0\r\n",
        );
        rk.add_model("00C0", "[OPT]\r\nKbImgUse=0x006B\r\n[KEY]\r\n");
        // A language folder, without a KB.ini.
        fs::create_dir_all(rk.0.join("Dev").join("en")).unwrap();

        let out = rk.0.join("out");
        let report = generate(&rk.0, &out).unwrap();
        assert!(
            report
                .summary
                .starts_with("models: 2, fw0 (v1): 1, fw1 (v2): 1,"),
            "{}",
            report.summary
        );

        let first = fs::read(out.join("006B.toml")).unwrap();
        let text = String::from_utf8(first.clone()).unwrap();
        assert!(
            text.starts_with("pid = 0x006B\nname = \"RK Lower\"\n"),
            "{text}"
        );
        assert!(!text.contains('\r'));
        let unnamed = fs::read_to_string(out.join("00C0.toml")).unwrap();
        assert!(unnamed.contains("name = \"RK 00C0\"\n"), "{unnamed}");
        assert!(unnamed.contains("layout = \"006B\"\n"), "{unnamed}");

        generate(&rk.0, &out).unwrap();
        assert_eq!(fs::read(out.join("006B.toml")).unwrap(), first);
        assert_eq!(fs::read_dir(&out).unwrap().count(), 2);
    }

    #[test]
    fn writes_nothing_when_validation_fails() {
        let rk = TempDir::new("invalid");
        fs::write(rk.0.join("Cfg.ini"), "[OPT]\n").unwrap();
        rk.add_model("0001", "[OPT]\nKbImgUse=0x0002\n[KEY]\n");
        rk.add_model("0003", "[OPT]\n[KEY]\n");

        let out = rk.0.join("out");
        let errors = generate(&rk.0, &out).err().unwrap();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("0002"), "{errors:?}");
        assert!(!out.exists());
    }

    #[test]
    fn a_kb_ini_in_a_folder_that_is_not_a_pid_is_an_error() {
        let rk = TempDir::new("badfolder");
        fs::write(rk.0.join("Cfg.ini"), "[OPT]\n").unwrap();
        rk.add_model("en", "[OPT]\n[KEY]\n");
        assert!(generate(&rk.0, &rk.0.join("out")).is_err());
    }
}
