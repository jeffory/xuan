//! The parts of starting a plugin in a Windows AppContainer that do not
//! call Windows, so they are tested on every platform.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use anyhow::{Result, bail, ensure};

/// The program `CreateProcessW` should run. A path is used as it is, a bare
/// name is looked up in `path_variable`. A name without an extension gets
/// `.exe`, as with `std::process::Command`.
pub fn find_program(program: &Path, path_variable: Option<&OsStr>) -> Option<PathBuf> {
    let candidates = |path: &Path| {
        let mut exe = path.as_os_str().to_owned();
        exe.push(".exe");
        let exe = PathBuf::from(exe);
        if path.extension().is_none() {
            vec![exe]
        } else {
            vec![path.to_path_buf(), exe]
        }
    };
    if program.is_absolute() || program.components().count() > 1 {
        return candidates(program).into_iter().find(|p| p.is_file());
    }
    std::env::split_paths(path_variable?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| {
            candidates(&dir.join(program))
                .into_iter()
                .find(|p| p.is_file())
        })
}

/// The folders outside `plugin_dir` the container must read to run
/// `program`: the program's own folder, and for a Python virtual
/// environment the base interpreter's folder, `home` in `pyvenv.cfg`.
pub fn interpreter_dirs(program: &Path, plugin_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let Some(bin) = program.parent() else {
        return dirs;
    };
    dirs.push(bin.to_path_buf());
    // `pyvenv.cfg` is in the venv root, the parent of `Scripts` (or `bin`).
    if let Some(text) = [Some(bin), bin.parent()]
        .into_iter()
        .flatten()
        .find_map(|dir| std::fs::read_to_string(dir.join("pyvenv.cfg")).ok())
    {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=')
                && key.trim().eq_ignore_ascii_case("home")
                && !value.trim().is_empty()
            {
                dirs.push(PathBuf::from(value.trim()));
            }
        }
    }
    dirs.retain(|dir| !dir.starts_with(plugin_dir));
    dirs.dedup();
    dirs
}

/// The command line for `CreateProcessW`, quoted so that the C runtime (and
/// Rust's `std::env::args`) splits it back into `program` and `args`. The
/// rules are those `std::process::Command` follows.
pub fn command_line(program: &str, args: &[String]) -> Result<String> {
    ensure!(
        !program.contains(['"', '\0']),
        "the program path `{program}` cannot be put on a command line"
    );
    // The program name is parsed without escapes: quotes suffice.
    let mut line = format!("\"{program}\"");
    for arg in args {
        if arg.contains('\0') {
            bail!("a plugin argument contains a NUL character");
        }
        line.push(' ');
        let quote = arg.is_empty() || arg.contains([' ', '\t']);
        if quote {
            line.push('"');
        }
        let mut backslashes = 0;
        for c in arg.chars() {
            if c == '\\' {
                backslashes += 1;
            } else {
                if c == '"' {
                    // Escape the backslashes before the quote, and the quote.
                    line.extend(std::iter::repeat_n('\\', backslashes + 1));
                }
                backslashes = 0;
            }
            line.push(c);
        }
        if quote {
            // Backslashes before the closing quote are doubled.
            line.extend(std::iter::repeat_n('\\', backslashes));
            line.push('"');
        }
    }
    Ok(line)
}

/// Xuan's environment with `overrides` applied, sorted by name ignoring
/// case as Windows expects. Names that differ only in case are one variable.
pub fn environment(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    overrides: &[(String, String)],
) -> Vec<(OsString, OsString)> {
    let key = |name: &OsStr| name.to_string_lossy().to_uppercase();
    let mut merged: BTreeMap<String, (OsString, OsString)> = (base.into_iter())
        .map(|(name, value)| (key(&name), (name, value)))
        .collect();
    for (name, value) in overrides {
        merged.insert(key(OsStr::new(name)), (name.into(), value.into()));
    }
    merged.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_follow_the_windows_quoting_rules() {
        let line = command_line(
            r"C:\Program Files\Python\python.exe",
            &[
                "main.py".into(),
                String::new(),
                "two words".into(),
                r#"say "hi""#.into(),
                r"C:\dir\".into(),
                r"C:\my dir\".into(),
                r#"a\"b"#.into(),
            ],
        )
        .unwrap();
        assert_eq!(
            line,
            r#""C:\Program Files\Python\python.exe" main.py "" "two words" "say \"hi\"" C:\dir\ "C:\my dir\\" a\\\"b"#
        );
        assert!(command_line("bad\"name", &[]).is_err());
        assert!(command_line("ok", &["nul\0".into()]).is_err());
    }

    #[test]
    fn programs_are_found_as_std_finds_them() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for file in ["python3.exe", "python3", "tool.cmd", "python3.12.exe"] {
            std::fs::write(bin.join(file), "").unwrap();
        }
        let path = std::env::join_paths([dir.path().join("missing"), bin.clone()]).unwrap();
        let find = |name: &str| find_program(Path::new(name), Some(&path));
        // A name without an extension gets .exe; the bare file is skipped.
        assert_eq!(find("python3"), Some(bin.join("python3.exe")));
        assert_eq!(find("tool.cmd"), Some(bin.join("tool.cmd")));
        assert_eq!(find("python3.12"), Some(bin.join("python3.12.exe")));
        assert_eq!(find("nothing"), None);
        assert_eq!(find_program(Path::new("python3"), None), None);
        // A path is not looked up.
        assert_eq!(
            find_program(&bin.join("python3"), None),
            Some(bin.join("python3.exe"))
        );
        assert_eq!(find_program(&dir.path().join("python3"), Some(&path)), None);
    }

    #[test]
    fn the_interpreter_and_a_venv_base_are_read_outside_the_plugin_folder() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        let python = dir.path().join("Python312");
        let scripts = plugin.join(".venv").join("Scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            plugin.join(".venv").join("pyvenv.cfg"),
            format!(
                "home = {}\ninclude-system-site-packages = false\n",
                python.display()
            ),
        )
        .unwrap();
        assert_eq!(
            interpreter_dirs(&python.join("python.exe"), &plugin),
            vec![python.clone()]
        );
        assert_eq!(
            interpreter_dirs(&scripts.join("python.exe"), &plugin),
            vec![python.clone()]
        );
        assert_eq!(
            interpreter_dirs(&plugin.join("target").join("tool.exe"), &plugin),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn overrides_replace_variables_ignoring_case_and_the_result_is_sorted() {
        let base = [("Path", "a"), ("windir", "w"), ("TEMP", "t")]
            .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let merged = environment(
            base,
            &[
                ("PATH".into(), "b".into()),
                ("Tmp".into(), "s".into()),
                ("XUAN_PLUGIN_ID".into(), "p".into()),
            ],
        );
        let merged: Vec<(String, String)> = (merged.into_iter())
            .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
            .collect();
        let expected = [
            ("PATH", "b"),
            ("TEMP", "t"),
            ("Tmp", "s"),
            ("windir", "w"),
            ("XUAN_PLUGIN_ID", "p"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        assert_eq!(merged, expected);
    }
}
