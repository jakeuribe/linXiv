//! Runs the frozen CLI goldens in `goldens/cli/` against the built binary.
//!
//! `.json` goldens are a byte-for-byte contract: each is one command's stdout on an
//! EMPTY database, keys in Python-parity insertion order (serde_json `preserve_order`).
//! Parsing them before comparing would silently accept a key-order regression, so we
//! compare bytes. Every capture gets its own fresh `LINXIV_DATA_DIR`.
//!
//! `.txt` goldens are still frozen *argparse* text while the CLI is clap, so byte
//! comparison is impossible. We assert structure instead: the command set matches
//! both ways, and every long flag the golden names still exists.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Goldens live at the repo root; locate them from the crate, not the process CWD.
fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../goldens/cli")
}

fn goldens_with_extension(ext: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(goldens_dir())
        .expect("goldens/cli not found — is the runner looking in the right place?")
        .map(|e| e.expect("read_dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    paths.sort();
    paths
}

/// `tag_list-all.json` -> `["tag", "list-all"]`.
fn argv_from_slug(slug: &str) -> Vec<&str> {
    if slug.is_empty() {
        vec![]
    } else {
        slug.split('_').collect()
    }
}

/// A throwaway empty data dir, so captures see an empty DB and the user's real
/// data dir is never touched.
struct TempDataDir(PathBuf);

impl TempDataDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "linxiv-goldens-{}-{}",
            std::process::id(),
            tag.replace(['/', ' '], "_")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        Self(dir)
    }
}

impl Drop for TempDataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_cli(args: &[&str], tag: &str) -> String {
    let data_dir = TempDataDir::new(tag);
    let out = Command::new(env!("CARGO_BIN_EXE_linxiv-cli"))
        .args(args)
        .env("LINXIV_DATA_DIR", &data_dir.0)
        .output()
        .unwrap_or_else(|e| panic!("failed to run `linxiv-cli {}`: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "`linxiv-cli {}` exited {}\nstderr:\n{}",
        args.join(" "),
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout).expect("CLI stdout is not UTF-8")
}

/// `--help` writes to stdout and exits 0 for a valid command, so `run_cli` covers it.
fn help_text(argv: &[&str], tag: &str) -> String {
    let mut args = argv.to_vec();
    args.push("--help");
    run_cli(&args, tag)
}

// ---------------------------------------------------------------- json contract

#[test]
fn json_goldens_match_byte_for_byte() {
    let goldens = goldens_with_extension("json");
    assert_eq!(
        goldens.len(),
        9,
        "expected 9 .json goldens, found {} — corpus changed, update this test",
        goldens.len()
    );

    for path in goldens {
        let slug = path.file_stem().unwrap().to_str().unwrap().to_string();
        let argv = argv_from_slug(&slug);
        let expected = std::fs::read_to_string(&path).expect("read golden");
        let actual = run_cli(&argv, &slug);

        assert!(
            expected == actual,
            "golden drift: `linxiv-cli {}`\n  golden: {}\n{}",
            argv.join(" "),
            path.display(),
            describe_diff(&expected, &actual),
        );
    }
}

/// A bare `assert_eq!` on two multi-KB strings is unreadable; point at the line.
fn describe_diff(expected: &str, actual: &str) -> String {
    let exp: Vec<&str> = expected.lines().collect();
    let act: Vec<&str> = actual.lines().collect();
    let first = (0..exp.len().max(act.len())).find(|&i| exp.get(i) != act.get(i));

    let mut msg = match first {
        Some(i) => format!(
            "  first difference at line {}:\n    expected: {:?}\n    actual:   {:?}\n",
            i + 1,
            exp.get(i).unwrap_or(&"<end of output>"),
            act.get(i).unwrap_or(&"<end of output>"),
        ),
        // Same lines but unequal strings: trailing-newline difference.
        None => "  lines are identical; trailing whitespace differs\n".to_string(),
    };
    msg.push_str(&format!(
        "  --- expected ({} bytes) ---\n{expected}\n  --- actual ({} bytes) ---\n{actual}",
        expected.len(),
        actual.len(),
    ));
    msg
}

// ----------------------------------------------------------- txt structure

#[test]
fn txt_goldens_match_the_command_tree() {
    let goldens = goldens_with_extension("txt");
    assert_eq!(
        goldens.len(),
        17,
        "expected 17 .txt goldens, found {} — corpus changed, update this test",
        goldens.len()
    );

    let mut failures = Vec::new();
    for path in goldens {
        // `help.txt` is the root; `project_help.txt` is `project --help`.
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let slug = stem.strip_suffix("help").unwrap_or(stem).trim_end_matches('_');
        let argv = argv_from_slug(slug);

        let golden = std::fs::read_to_string(&path).expect("read golden");
        let actual = help_text(&argv, stem);
        let actual_tokens = tokens(&actual);

        let mut problems = Vec::new();

        let golden_cmds = argparse_commands(&golden);
        let actual_cmds = clap_commands(&actual);
        for missing in golden_cmds.difference(&actual_cmds) {
            problems.push(format!("command `{missing}` is in the golden but not in the CLI"));
        }
        for extra in actual_cmds.difference(&golden_cmds) {
            problems.push(format!("command `{extra}` is in the CLI but not in the golden"));
        }
        let golden_flags = argparse_long_flags(&golden);
        for flag in &golden_flags {
            if !actual_tokens.contains(flag) {
                problems.push(format!("flag `{flag}` is in the golden but not in the CLI"));
            }
        }
        // Both directions, same as commands above: a flag the CLI grew that no
        // golden names is drift too, and it is the half that silently accumulates.
        for flag in clap_long_flags(&actual) {
            if !golden_flags.contains(&flag) {
                problems.push(format!("flag `{flag}` is in the CLI but not in the golden"));
            }
        }

        if !problems.is_empty() {
            failures.push(format!(
                "`linxiv-cli {} --help` vs {}\n    {}\n  --- actual help ---\n{}",
                argv.join(" "),
                path.display(),
                problems.join("\n    "),
                actual,
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "golden drift in {} help golden(s):\n\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}

/// argparse lists subcommands as a brace group, both in the wrapped usage line and
/// on its own line under `positional arguments:`. A flag's choices (`--source
/// {arxiv,...}`) never start a line, so anchoring on `{` keeps them out.
fn argparse_commands(golden: &str) -> BTreeSet<String> {
    golden
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix('{'))
        .filter_map(|rest| rest.split('}').next())
        .flat_map(|inner| inner.split(','))
        .map(str::to_string)
        .collect()
}

fn argparse_long_flags(golden: &str) -> BTreeSet<String> {
    tokens(golden)
        .into_iter()
        .filter(|t| t.starts_with("--") && t.len() > 2)
        .collect()
}

/// The CLI's own long flags, for the reverse direction. Same tokeniser, then
/// trailing `=` stripped — clap writes `--flag=<VAL>` where argparse wrote
/// `--flag <VAL>`, and that punctuation difference is formatting, not drift.
fn clap_long_flags(help: &str) -> BTreeSet<String> {
    tokens(help)
        .into_iter()
        .filter(|t| t.starts_with("--") && t.len() > 2)
        .map(|t| t.trim_end_matches('=').to_string())
        .collect()
}

/// Command names in clap's `Commands:` block sit at exactly two spaces of indent;
/// wrapped description lines are indented further, which is how they're excluded.
/// `help` is clap's builtin and was never in the argparse goldens.
fn clap_commands(help: &str) -> BTreeSet<String> {
    help.lines()
        .skip_while(|l| l.trim() != "Commands:")
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.strip_prefix("  ").filter(|rest| !rest.starts_with(' ')))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_string)
        .collect()
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '[' | ']' | '<' | '>'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}
