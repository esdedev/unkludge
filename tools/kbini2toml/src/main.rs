//! Generates `data/models/<PID>.toml` from an installed copy of the RK software.
//!
//! Usage: `kbini2toml <RK dir> <out dir>`. Reads `Cfg.ini` and `Dev/<PID>/KB.ini`, and only writes
//! when every model passes validation. The output is sorted and a file's `tested` status stays, so
//! regenerating gives no diff.

mod ini;
mod model;

use ini::Ini;
use model::Model;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [rk_dir, out_dir] = args.as_slice() else {
        eprintln!("usage: kbini2toml <RK dir> <out dir>");
        return ExitCode::from(2);
    };
    let report = generate(Path::new(rk_dir), Path::new(out_dir));
    eprint!("{}", diagnostics(&report));
    if !report.summary.is_empty() {
        println!("{}", report.summary);
    }
    if report.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[derive(Default)]
struct Report {
    warnings: Vec<String>,
    errors: Vec<String>,
    summary: String,
}

/// Warnings first, then errors, so a failed run still shows both. Built as one string so it reaches
/// the terminal in one write, with no line split by the summary on the other stream.
fn diagnostics(report: &Report) -> String {
    let mut out = String::new();
    for warning in &report.warnings {
        let _ = writeln!(out, "warning: {warning}");
    }
    for error in &report.errors {
        let _ = writeln!(out, "error: {error}");
    }
    out
}

fn read_ini(path: &Path) -> Result<Ini, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = ini::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Ini::parse(&text))
}

fn generate(rk_dir: &Path, out_dir: &Path) -> Report {
    let mut report = Report::default();
    if let Err(error) = run(rk_dir, out_dir, &mut report) {
        report.errors.push(error);
    }
    report
}

fn run(rk_dir: &Path, out_dir: &Path, report: &mut Report) -> Result<(), String> {
    let names = model::parse_names(&read_ini(&rk_dir.join("Cfg.ini"))?);

    let mut models = Vec::new();
    let mut sources = BTreeSet::new();
    let mut unknown: BTreeMap<String, Vec<u16>> = BTreeMap::new();
    for (folder, path) in model_files(rk_dir)? {
        // Folder names aren't consistent in case: both 01FA and 006b exist.
        let Ok(pid) = u16::from_str_radix(&folder, 16) else {
            report
                .errors
                .push(format!("{folder}: folder name isn't a PID"));
            continue;
        };
        sources.insert(pid);
        let parsed = read_ini(&path)
            .and_then(|ini| model::parse_model(pid, model::model_name(pid, &names), &ini));
        match parsed {
            Ok(parsed) => {
                for key in parsed.unknown {
                    unknown.entry(key).or_default().push(pid);
                }
                let warnings = parsed.warnings.iter().map(|w| format!("{pid:04X}: {w}"));
                report.warnings.extend(warnings);
                models.push(parsed.model);
            }
            Err(error) => report.errors.push(format!("{pid:04X}: {error}")),
        }
    }
    models.sort_by_key(|model| model.pid);
    let own_keys = model::resolve_layouts(&mut models);
    report.errors.extend(model::validate(&models));

    for (key, pids) in &unknown {
        report.warnings.push(format!(
            "unknown key {key} in {} models (first {:04X})",
            pids.len(),
            pids[0]
        ));
    }
    report.warnings.extend(stale_files(out_dir, &sources));
    report.summary = summary(&models, own_keys, &unknown);
    if !report.errors.is_empty() {
        return Ok(());
    }

    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for model in &models {
        let path = out_dir.join(file_name(model.pid));
        let mut text = model::to_toml(model);
        // Nothing in the source files says which models have been tried.
        let tested = std::fs::read_to_string(&path)
            .is_ok_and(|old| old.lines().any(|line| line.trim() == "status = \"tested\""));
        if tested {
            text = text.replacen("status = \"experimental\"", "status = \"tested\"", 1);
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn file_name(pid: u16) -> String {
    format!("{pid:04X}.toml")
}

/// Warnings for `.toml` files in the out dir with no `KB.ini` behind them, for example a model RK
/// removed. They stay in place: deleting is for a person to decide.
fn stale_files(out_dir: &Path, sources: &BTreeSet<u16>) -> Vec<String> {
    let expected: BTreeSet<String> = sources.iter().map(|pid| file_name(*pid)).collect();
    // No out dir yet means nothing is stale.
    let Ok(entries) = std::fs::read_dir(out_dir) else {
        return Vec::new();
    };
    let mut stale: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".toml") && !expected.contains(name))
        .collect();
    stale.sort();
    stale
        .iter()
        .map(|name| format!("{name} in the out dir has no KB.ini"))
        .collect()
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

fn summary(models: &[Model], own_keys: usize, unknown: &BTreeMap<String, Vec<u16>>) -> String {
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
        "models: {}, fw0 (v1): {}, fw1 (v2): {}, fw1 with screen: {screens}, fw2 (v2alt): {}, own keys despite KbImgUse: {own_keys}, unknown keys: {unknown}",
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
            fs::write(dir.join("Cfg.ini"), "[OPT]\n").unwrap();
            TempDir(dir)
        }

        fn add_bytes(&self, folder: &str, bytes: &[u8]) {
            let dir = self.0.join("Dev").join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("KB.ini"), bytes).unwrap();
        }

        fn add_model(&self, folder: &str, text: &str) {
            self.add_bytes(folder, text.as_bytes());
        }

        fn out(&self) -> PathBuf {
            self.0.join("out")
        }

        fn generate(&self) -> Report {
            generate(&self.0, &self.out())
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

    /// Smallest valid file. `opt` comes first so its keys win over the default effect.
    fn kb(opt: &str) -> String {
        format!(
            "[OPT]\r\n{opt}\r\nLedOpt1=0,0,0,0,0,0\r\n[KEY]\r\n;Esc\r\nK1=1,2,3,4,2,0x1B,0,0\r\n"
        )
    }

    /// File names as the file system reports them. On NTFS `exists()` and `read()` ignore case, so
    /// only this can tell `006B.toml` from `006b.toml`.
    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_one_uppercase_file_per_kb_ini_and_is_repeatable() {
        let rk = TempDir::new("writes");
        fs::write(
            rk.0.join("Cfg.ini"),
            utf16("[OPT]\r\nDevName1=006b,RK Lower Keyboard\r\n"),
        )
        .unwrap();
        rk.add_model("006b", &kb("Fw=1"));
        rk.add_model("00C0", &kb("KbImgUse=0x006B"));
        // A language folder, without a KB.ini.
        fs::create_dir_all(rk.0.join("Dev").join("en")).unwrap();

        let out = rk.out();
        let report = rk.generate();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(file_names(&out), ["006B.toml", "00C0.toml"]);

        let first = fs::read(out.join("006B.toml")).unwrap();
        let text = String::from_utf8(first.clone()).unwrap();
        assert!(
            text.starts_with("pid = 0x006B\nname = \"RK Lower\"\n"),
            "{text}"
        );
        assert!(!text.contains('\r'));
        let unnamed = fs::read_to_string(out.join("00C0.toml")).unwrap();
        assert!(unnamed.contains("name = \"RK 00C0\"\n"), "{unnamed}");

        assert!(rk.generate().errors.is_empty());
        assert_eq!(fs::read(out.join("006B.toml")).unwrap(), first);
        assert_eq!(file_names(&out), ["006B.toml", "00C0.toml"]);
    }

    #[test]
    fn regenerating_keeps_a_tested_status_and_nothing_else_of_the_old_file() {
        let rk = TempDir::new("tested");
        rk.add_model("0001", &kb("Fw=1"));
        rk.add_model("0002", &kb("Fw=1"));
        assert!(rk.generate().errors.is_empty());

        let tested = rk.out().join("0001.toml");
        let original = fs::read_to_string(&tested).unwrap();
        let marked = original.replacen(
            "status = \"experimental\"",
            "status = \"tested\"\n# by hand",
            1,
        );
        assert_ne!(marked, original);
        fs::write(&tested, marked).unwrap();
        let other = fs::read(rk.out().join("0002.toml")).unwrap();

        assert!(rk.generate().errors.is_empty());
        assert_eq!(
            fs::read_to_string(&tested).unwrap(),
            original.replacen("status = \"experimental\"", "status = \"tested\"", 1),
        );
        assert_eq!(fs::read(rk.out().join("0002.toml")).unwrap(), other);
    }

    #[test]
    fn writes_nothing_when_validation_fails() {
        let rk = TempDir::new("invalid");
        rk.add_model("0001", &kb("KbImgUse=0x0002"));
        rk.add_model("0003", &kb(""));

        let errors = rk.generate().errors;
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("0002"), "{errors:?}");
        assert!(!rk.out().exists());
    }

    #[test]
    fn a_kb_ini_in_a_folder_that_is_not_a_pid_is_an_error() {
        let rk = TempDir::new("badfolder");
        rk.add_model("en", &kb(""));
        let errors = rk.generate().errors;
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("en"), "{errors:?}");
    }

    #[test]
    fn a_file_that_reads_as_nothing_is_an_error_and_writes_nothing() {
        let no_bom: Vec<u8> = utf16(&kb(""))[2..].to_vec();
        let mut big_endian = vec![0xFE, 0xFF];
        for unit in kb("").encode_utf16() {
            big_endian.extend_from_slice(&unit.to_be_bytes());
        }
        let cases: [(&str, Vec<u8>); 5] = [
            ("empty", Vec::new()),
            ("utf-16 without a BOM", no_bom),
            ("utf-16 big endian", big_endian),
            ("lowercase sections", kb("").to_lowercase().into_bytes()),
            (
                "not utf-8",
                b"[OPT]\n;\xC9\xCF\nLedOpt1=0,0,0,0,0,0\n[KEY]\n".to_vec(),
            ),
        ];
        for (name, bytes) in cases {
            let rk = TempDir::new(&format!("nothing-{}", name.replace(' ', "-")));
            rk.add_bytes("0001", &bytes);
            let errors = rk.generate().errors;
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert!(errors[0].starts_with("0001"), "{name}: {errors:?}");
            assert!(!rk.out().exists(), "{name}");
        }
    }

    #[test]
    fn an_undecodable_cfg_ini_is_an_error() {
        let rk = TempDir::new("badcfg");
        fs::write(rk.0.join("Cfg.ini"), b"\xFE\xFF\x00[").unwrap();
        rk.add_model("0001", &kb(""));
        let errors = rk.generate().errors;
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("Cfg.ini"), "{errors:?}");
        assert!(!rk.out().exists());
    }

    #[test]
    fn unknown_keys_are_reported_with_count_and_first_pid() {
        let rk = TempDir::new("unknown");
        rk.add_model("0002", &kb("BrandNew=1"));
        rk.add_model("0001", &kb("BrandNew=1\r\nOtherNew=1"));
        let report = rk.generate();
        assert_eq!(
            report.warnings,
            [
                "unknown key BrandNew in 2 models (first 0001)",
                "unknown key OtherNew in 1 models (first 0001)",
            ]
        );
        assert!(
            report
                .summary
                .ends_with("unknown keys: BrandNew (2), OtherNew (1)"),
            "{}",
            report.summary
        );
    }

    #[test]
    fn warnings_from_a_model_are_reported_with_its_pid() {
        let rk = TempDir::new("modelwarn");
        rk.add_model(
            "00AB",
            &format!("{}Foo=1\r\n[FN1]\r\nK9=2,1,0\r\n[FN3]\r\n", kb("")),
        );
        assert_eq!(
            rk.generate().warnings,
            [
                "00AB: unknown section [FN3]",
                "00AB: Foo in KEY isn't a key",
                "00AB: K9 in FN1 has no key",
            ]
        );
    }

    #[test]
    fn summary_counts_each_kind_of_model() {
        let rk = TempDir::new("summary");
        rk.add_model("0001", &kb(""));
        rk.add_model("0002", &kb("Fw=1"));
        rk.add_model(
            "0003",
            &kb("Fw=1\r\nShowScreen=1\r\nsiScreen=8,8\r\nMaxScreenFrame=9"),
        );
        rk.add_model("0004", &kb("Fw=2"));
        assert_eq!(
            rk.generate().summary,
            "models: 4, fw0 (v1): 1, fw1 (v2): 2, fw1 with screen: 1, fw2 (v2alt): 1, \
             own keys despite KbImgUse: 0, unknown keys: none"
        );
    }

    #[test]
    fn layouts_are_decided_by_comparing_keys() {
        let rk = TempDir::new("layouts");
        rk.add_model("0001", &kb(""));
        rk.add_model("0002", &kb("KbImgUse=0x0001"));
        let different = kb("KbImgUse=0x0001").replace("0x1B", "0x1C");
        rk.add_model("0003", &different);

        let report = rk.generate();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(
            report.summary.contains("own keys despite KbImgUse: 1,"),
            "{}",
            report.summary
        );

        let same = fs::read_to_string(rk.out().join("0002.toml")).unwrap();
        assert!(same.contains("layout = \"0001\"\n"), "{same}");
        assert!(!same.contains("[[keys]]"), "{same}");

        let own = fs::read_to_string(rk.out().join("0003.toml")).unwrap();
        assert!(!own.contains("layout"), "{own}");
        assert!(own.contains("code = 0x1C\n"), "{own}");
    }

    #[test]
    fn toml_files_with_no_source_are_warned_about_and_kept() {
        let rk = TempDir::new("stale");
        rk.add_model("006b", &kb(""));
        let out = rk.out();
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("FFFF.toml"), "old").unwrap();
        fs::write(out.join("AAAA.toml"), "old").unwrap();
        fs::write(out.join("notes.txt"), "mine").unwrap();
        // Written by an earlier run: has a source, so no warning.
        assert!(rk.generate().warnings.iter().all(|w| !w.contains("006B")));

        let report = rk.generate();
        assert_eq!(
            report.warnings,
            [
                "AAAA.toml in the out dir has no KB.ini",
                "FFFF.toml in the out dir has no KB.ini",
            ]
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(fs::read_to_string(out.join("FFFF.toml")).unwrap(), "old");
    }

    #[test]
    fn a_model_that_fails_to_parse_still_counts_as_a_source() {
        let rk = TempDir::new("stalefail");
        rk.add_model("0001", "");
        let out = rk.out();
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("0001.toml"), "old").unwrap();
        let report = rk.generate();
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn warnings_survive_a_failed_run_and_print_before_errors() {
        let rk = TempDir::new("order");
        rk.add_model("0001", &kb("BrandNew=1\r\nKbImgUse=0x0002"));
        let report = rk.generate();
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);

        let text = diagnostics(&report);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "warning: unknown key BrandNew in 1 models (first 0001)",
                "error: 0001: layout 0002 doesn't exist",
            ]
        );
    }
}
