//! Test-runner output parsers.
//!
//! Every parser takes untrusted text and never panics (see the property tests in
//! `tests/props.rs`). Output from several runners in one log (for example a
//! `make test` that runs cargo and pytest) is summed.

use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Upper bound on test names kept per list, so a receipt stays small.
pub const MAX_NAMES: usize = 5000;

/// Which harness produced a summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    Cargo,
    Nextest,
    Pytest,
    Jest,
    Vitest,
    Go,
}

impl Runner {
    pub fn as_str(&self) -> &'static str {
        match self {
            Runner::Cargo => "cargo",
            Runner::Nextest => "nextest",
            Runner::Pytest => "pytest",
            Runner::Jest => "jest",
            Runner::Vitest => "vitest",
            Runner::Go => "go",
        }
    }
}

/// Counts and names recovered from a test log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Runners whose summary lines were found, in detection order.
    pub runners: Vec<Runner>,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    /// Passed with a warning (pytest XPASS).
    pub warned: u64,
    #[serde(default)]
    pub passed_names: Vec<String>,
    #[serde(default)]
    pub failed_names: Vec<String>,
    #[serde(default)]
    pub warned_names: Vec<String>,
    /// Human-readable notes about how the counts were derived.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Summary {
    /// True when at least one runner's summary was recognised.
    pub fn recognized(&self) -> bool {
        !self.runners.is_empty()
    }

    fn add_runner(&mut self, r: Runner) {
        if !self.runners.contains(&r) {
            self.runners.push(r);
        }
    }

    fn push_name(list: &mut Vec<String>, name: &str) {
        if list.len() < MAX_NAMES {
            let n = name.trim();
            if !n.is_empty() && !list.iter().any(|x| x == n) {
                list.push(n.chars().take(512).collect());
            }
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("static regex"))
}

fn num(s: Option<regex_lite::Match<'_>>) -> u64 {
    s.and_then(|m| m.as_str().parse::<u64>().ok()).unwrap_or(0)
}

/// Parse a complete test log. `argv` is used only as a hint for Go's
/// package-level output; parsing is driven by the text itself.
pub fn parse(output: &str, argv: &[String]) -> Summary {
    let text = crate::util::strip_ansi(output);
    let mut s = Summary::default();
    parse_libtest(&text, &mut s);
    parse_nextest(&text, &mut s);
    parse_pytest(&text, &mut s);
    parse_jest(&text, &mut s);
    parse_vitest(&text, &mut s);
    parse_go(&text, argv, &mut s);
    s
}

/// `cargo test` (libtest) output, including doctests.
fn parse_libtest(text: &str, s: &mut Summary) {
    static RESULT: OnceLock<Regex> = OnceLock::new();
    static LINE: OnceLock<Regex> = OnceLock::new();
    let result =
        re(&RESULT, r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out");
    let line = re(&LINE, r"^test (.+?) \.\.\. (ok|FAILED|ignored(?:, .*)?|bench:.*)$");
    let mut found = false;
    let (mut p, mut f, mut i, mut filtered) = (0u64, 0u64, 0u64, 0u64);
    let (mut pn, mut fnames) = (Vec::new(), Vec::new());
    for l in text.lines() {
        let l = l.trim_end();
        if let Some(c) = result.captures(l) {
            found = true;
            p += num(c.get(1));
            f += num(c.get(2));
            i += num(c.get(3));
            filtered += num(c.get(5));
        } else if let Some(c) = line.captures(l) {
            let name = c.get(1).map(|m| m.as_str()).unwrap_or("");
            match c.get(2).map(|m| m.as_str()).unwrap_or("") {
                "ok" => Summary::push_name(&mut pn, name),
                "FAILED" => Summary::push_name(&mut fnames, name),
                _ => {}
            }
        }
    }
    if found {
        s.add_runner(Runner::Cargo);
        s.passed += p;
        s.failed += f;
        s.skipped += i;
        s.passed_names.extend(pn);
        s.failed_names.extend(fnames);
        if filtered > 0 {
            s.notes.push(format!("cargo: {filtered} tests filtered out"));
        }
    }
}

/// `cargo nextest run` output.
fn parse_nextest(text: &str, s: &mut Summary) {
    static SUM: OnceLock<Regex> = OnceLock::new();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static LINE: OnceLock<Regex> = OnceLock::new();
    let sum = re(&SUM, r"^\s*Summary \[\s*[\d.]+s\]\s+(\d+) tests? run: (.*)$");
    let item = re(&ITEM, r"(\d+) (passed|failed|skipped|timed out|flaky|leaky)");
    let line = re(&LINE, r"^\s+(PASS|FAIL|SIGSEGV|SIGABRT|TIMEOUT|ABORT)(?: \[[^\]]*\])?\s+\[\s*[\d.]+s\]\s+(.+)$");
    let mut last: Option<(u64, u64, u64)> = None;
    let (mut pn, mut fnames) = (Vec::new(), Vec::new());
    for l in text.lines() {
        if let Some(c) = sum.captures(l) {
            let (mut p, mut f, mut sk) = (0, 0, 0);
            let rest = c.get(2).map(|m| m.as_str()).unwrap_or("");
            for ic in item.captures_iter(rest) {
                let n = num(ic.get(1));
                match ic.get(2).map(|m| m.as_str()).unwrap_or("") {
                    "passed" => p += n,
                    "failed" | "timed out" => f += n,
                    "skipped" => sk += n,
                    _ => {}
                }
            }
            last = Some((p, f, sk));
        } else if let Some(c) = line.captures(l) {
            let name = c.get(2).map(|m| m.as_str()).unwrap_or("");
            if c.get(1).map(|m| m.as_str()) == Some("PASS") {
                Summary::push_name(&mut pn, name);
            } else {
                Summary::push_name(&mut fnames, name);
            }
        }
    }
    if let Some((p, f, sk)) = last {
        s.add_runner(Runner::Nextest);
        s.passed += p;
        s.failed += f;
        s.skipped += sk;
        s.passed_names.extend(pn);
        s.failed_names.extend(fnames);
    } else if text.lines().any(|l| l.trim_start().starts_with("error: no tests to run")) {
        s.add_runner(Runner::Nextest);
        s.notes.push("nextest: no tests to run".into());
    }
}

/// pytest output, including pytest-xdist.
fn parse_pytest(text: &str, s: &mut Summary) {
    static FINAL: OnceLock<Regex> = OnceLock::new();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static VLINE: OnceLock<Regex> = OnceLock::new();
    static XLINE: OnceLock<Regex> = OnceLock::new();
    static SHORT: OnceLock<Regex> = OnceLock::new();
    // "==== 3 passed, 1 skipped in 0.12s ====", "3 passed in 0.01s" (-q), "no tests ran in 0.01s".
    let fin = re(
        &FINAL,
        r"^=*\s*((?:\d+ (?:passed|failed|skipped|deselected|xfailed|xpassed|errors?|warnings?|rerun)(?:, )?)+|no tests ran)(?: in [\d.]+s(?: \([^)]*\))?)?\s*=*$",
    );
    let item = re(&ITEM, r"(\d+) (passed|failed|skipped|deselected|xfailed|xpassed|errors?|warnings?|rerun)");
    let vline = re(&VLINE, r"^(\S+::\S.*?) (PASSED|FAILED|SKIPPED|XFAIL|XPASS|ERROR)(?:\s|$)");
    let xline = re(&XLINE, r"^\[gw\d+\] \[\s*\d+%\] (PASSED|FAILED|SKIPPED|XFAIL|XPASS|ERROR) (\S+::\S.*)$");
    let short = re(&SHORT, r"^(FAILED|ERROR) (\S+::\S+?)(?: - .*)?$");
    let mut last: Option<(u64, u64, u64, u64, bool, u64)> = None;
    let (mut pn, mut fnames, mut wn) = (Vec::new(), Vec::new(), Vec::new());
    for l in text.lines() {
        let t = l.trim();
        if let Some(c) = fin.captures(t) {
            let body = c.get(1).map(|m| m.as_str()).unwrap_or("");
            let (mut p, mut f, mut sk, mut w, mut d) = (0, 0, 0, 0, 0);
            for ic in item.captures_iter(body) {
                let n = num(ic.get(1));
                match ic.get(2).map(|m| m.as_str()).unwrap_or("") {
                    "passed" => p += n,
                    "failed" | "error" | "errors" => f += n,
                    "skipped" | "xfailed" => sk += n,
                    "xpassed" => w += n,
                    "deselected" => d += n,
                    _ => {}
                }
            }
            last = Some((p, f, sk, w, body == "no tests ran", d));
            continue;
        }
        let (status, name) = if let Some(c) = xline.captures(t) {
            (c.get(1).map(|m| m.as_str()).unwrap_or(""), c.get(2).map(|m| m.as_str()).unwrap_or(""))
        } else if let Some(c) = vline.captures(t) {
            (c.get(2).map(|m| m.as_str()).unwrap_or(""), c.get(1).map(|m| m.as_str()).unwrap_or(""))
        } else if let Some(c) = short.captures(t) {
            (c.get(1).map(|m| m.as_str()).unwrap_or(""), c.get(2).map(|m| m.as_str()).unwrap_or(""))
        } else {
            continue;
        };
        match status {
            "PASSED" => Summary::push_name(&mut pn, name),
            "FAILED" | "ERROR" => Summary::push_name(&mut fnames, name),
            "XPASS" => Summary::push_name(&mut wn, name),
            _ => {}
        }
    }
    if let Some((p, f, sk, w, none, desel)) = last {
        s.add_runner(Runner::Pytest);
        s.passed += p;
        s.failed += f;
        s.skipped += sk;
        s.warned += w;
        s.passed_names.extend(pn);
        s.failed_names.extend(fnames);
        s.warned_names.extend(wn);
        if none {
            s.notes.push("pytest: no tests ran".into());
        }
        if desel > 0 {
            s.notes.push(format!("pytest: {desel} deselected"));
        }
    }
}

/// Jest output.
fn parse_jest(text: &str, s: &mut Summary) {
    static TESTS: OnceLock<Regex> = OnceLock::new();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static FAILNAME: OnceLock<Regex> = OnceLock::new();
    let tests = re(&TESTS, r"^Tests:\s+(.*\d+ total)\s*$");
    let item = re(&ITEM, r"(\d+) (passed|failed|skipped|todo|total)");
    let failname = re(&FAILNAME, r"^\s*● (.+ › .+)$");
    let mut last: Option<(u64, u64, u64)> = None;
    let mut fnames = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if let Some(c) = tests.captures(t) {
            let (mut p, mut f, mut sk) = (0, 0, 0);
            for ic in item.captures_iter(c.get(1).map(|m| m.as_str()).unwrap_or("")) {
                let n = num(ic.get(1));
                match ic.get(2).map(|m| m.as_str()).unwrap_or("") {
                    "passed" => p += n,
                    "failed" => f += n,
                    "skipped" | "todo" => sk += n,
                    _ => {}
                }
            }
            last = Some((p, f, sk));
        } else if let Some(c) = failname.captures(l) {
            Summary::push_name(&mut fnames, c.get(1).map(|m| m.as_str()).unwrap_or(""));
        }
    }
    if let Some((p, f, sk)) = last {
        s.add_runner(Runner::Jest);
        s.passed += p;
        s.failed += f;
        s.skipped += sk;
        s.failed_names.extend(fnames);
    } else if text.lines().any(|l| l.trim_start().starts_with("No tests found, exiting with code")) {
        s.add_runner(Runner::Jest);
        s.notes.push("jest: no tests found".into());
    }
    // "Ran all test suites." is a full run; "Ran all test suites matching /x/i." or
    // "... related to changed files" or "... with tests matching" is a selection.
    if last.is_some() {
        if let Some(l) = text.lines().map(str::trim).find(|l| l.starts_with("Ran all test suites") && *l != "Ran all test suites.") {
            s.notes.push(format!("jest: selected a subset ({})", l.chars().take(160).collect::<String>()));
        }
    }
}

/// Vitest output.
fn parse_vitest(text: &str, s: &mut Summary) {
    static TESTS: OnceLock<Regex> = OnceLock::new();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static FAILNAME: OnceLock<Regex> = OnceLock::new();
    let tests = re(&TESTS, r"^Tests\s+((?:\d+ (?:passed|failed|skipped|todo)(?: \| )?)+)\s*\((\d+)\)\s*$");
    let item = re(&ITEM, r"(\d+) (passed|failed|skipped|todo)");
    let failname = re(&FAILNAME, r"^\s*(?:FAIL|×|✗)\s+(\S+ > .+?)(?: \d+ms)?$");
    let mut last: Option<(u64, u64, u64)> = None;
    let mut fnames = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if let Some(c) = tests.captures(t) {
            let (mut p, mut f, mut sk) = (0, 0, 0);
            for ic in item.captures_iter(c.get(1).map(|m| m.as_str()).unwrap_or("")) {
                let n = num(ic.get(1));
                match ic.get(2).map(|m| m.as_str()).unwrap_or("") {
                    "passed" => p += n,
                    "failed" => f += n,
                    _ => sk += n,
                }
            }
            last = Some((p, f, sk));
        } else if let Some(c) = failname.captures(t) {
            Summary::push_name(&mut fnames, c.get(1).map(|m| m.as_str()).unwrap_or(""));
        }
    }
    if let Some((p, f, sk)) = last {
        s.add_runner(Runner::Vitest);
        s.passed += p;
        s.failed += f;
        s.skipped += sk;
        s.failed_names.extend(fnames);
    } else if text.lines().any(|l| l.trim_start().starts_with("No test files found, exiting with code")) {
        s.add_runner(Runner::Vitest);
        s.notes.push("vitest: no test files found".into());
    }
}

/// `go test` output: `-json`, `-v`, or the default package lines.
fn parse_go(text: &str, argv: &[String], s: &mut Summary) {
    static RES: OnceLock<Regex> = OnceLock::new();
    static PKG: OnceLock<Regex> = OnceLock::new();
    let res = re(&RES, r"^\s*--- (PASS|FAIL|SKIP): (\S+)");
    let pkg = re(&PKG, r"^(ok|FAIL|\?)\s+(\S+)(?:\s+(.*))?$");

    // go test -json
    let (mut jp, mut jf, mut js) = (0u64, 0u64, 0u64);
    let (mut jpn, mut jfn) = (Vec::new(), Vec::new());
    let mut json_seen = false;
    for l in text.lines() {
        let t = l.trim();
        if !t.starts_with('{') || !t.contains("\"Action\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(t) else { continue };
        let (Some(action), Some(test)) = (v.get("Action").and_then(|a| a.as_str()), v.get("Test").and_then(|a| a.as_str())) else {
            if v.get("Action").is_some() {
                json_seen = true;
            }
            continue;
        };
        json_seen = true;
        let pkgname = v.get("Package").and_then(|a| a.as_str()).unwrap_or("");
        let name = if pkgname.is_empty() { test.to_string() } else { format!("{pkgname}.{test}") };
        match action {
            "pass" => {
                jp += 1;
                Summary::push_name(&mut jpn, &name);
            }
            "fail" => {
                jf += 1;
                Summary::push_name(&mut jfn, &name);
            }
            "skip" => js += 1,
            _ => {}
        }
    }
    if json_seen {
        s.add_runner(Runner::Go);
        s.passed += jp;
        s.failed += jf;
        s.skipped += js;
        s.passed_names.extend(jpn);
        s.failed_names.extend(jfn);
        return;
    }

    let (mut vp, mut vf, mut vs) = (0u64, 0u64, 0u64);
    let (mut vpn, mut vfn) = (Vec::new(), Vec::new());
    let (mut ok_pkgs, mut fail_pkgs, mut empty_pkgs) = (0u64, 0u64, 0u64);
    let mut fail_pkg_names = Vec::new();
    for l in text.lines() {
        let t = l.trim_end();
        if let Some(c) = res.captures(t) {
            let name = c.get(2).map(|m| m.as_str()).unwrap_or("");
            match c.get(1).map(|m| m.as_str()).unwrap_or("") {
                "PASS" => {
                    vp += 1;
                    Summary::push_name(&mut vpn, name);
                }
                "FAIL" => {
                    vf += 1;
                    Summary::push_name(&mut vfn, name);
                }
                _ => vs += 1,
            }
        } else if let Some(c) = pkg.captures(t) {
            let tailtxt = c.get(3).map(|m| m.as_str()).unwrap_or("");
            let pkgname = c.get(2).map(|m| m.as_str()).unwrap_or("");
            // Exclude lines that are clearly not go package results.
            if pkgname.contains("::") {
                continue;
            }
            match c.get(1).map(|m| m.as_str()).unwrap_or("") {
                "ok" if tailtxt.contains("[no tests to run]") => empty_pkgs += 1,
                "ok" if looks_like_go_duration(tailtxt) => ok_pkgs += 1,
                "FAIL" if looks_like_go_duration(tailtxt) || tailtxt.contains("[build failed]") || tailtxt.contains("[setup failed]") => {
                    fail_pkgs += 1;
                    Summary::push_name(&mut fail_pkg_names, pkgname);
                }
                "?" if tailtxt.contains("[no test files]") => empty_pkgs += 1,
                _ => {}
            }
        }
    }
    let is_go_cmd = argv.iter().any(|a| a == "go" || a.ends_with("/go") || a.ends_with("\\go.exe") || a == "go.exe");
    if vp + vf + vs > 0 {
        s.add_runner(Runner::Go);
        s.passed += vp;
        s.failed += vf.max(fail_pkgs);
        s.skipped += vs;
        s.passed_names.extend(vpn);
        s.failed_names.extend(vfn);
    } else if ok_pkgs + fail_pkgs > 0 || (empty_pkgs > 0 && is_go_cmd) {
        s.add_runner(Runner::Go);
        s.passed += ok_pkgs;
        s.failed += fail_pkgs;
        s.failed_names.extend(fail_pkg_names);
        s.notes.push("go: counts are packages, not tests (run with -v or -json for per-test counts)".into());
    }
}

fn looks_like_go_duration(t: &str) -> bool {
    let first = t.split_whitespace().next().unwrap_or("");
    first == "(cached)" || (first.ends_with('s') && first.trim_end_matches('s').parse::<f64>().is_ok())
}

/// Words of a command for argument inspection: argv as given, or a shell line split on
/// whitespace with surrounding quotes removed.
pub fn command_words(argv: &[String], shell: bool) -> Vec<String> {
    if shell {
        argv.join(" ").split_whitespace().map(|w| w.trim_matches(|c| c == '"' || c == '\'').to_string()).collect()
    } else {
        argv.to_vec()
    }
}

/// Lower-case program name without directory or Windows executable extension.
pub fn prog_name(w: &str) -> String {
    let base = w.rsplit(['/', '\\']).next().unwrap_or(w).to_ascii_lowercase();
    base.trim_end_matches(".exe").trim_end_matches(".cmd").trim_end_matches(".bat").to_string()
}

/// Does this run select a subset of the tests by name or file? Returns what gave it away.
///
/// Detected from the output (`cargo test` "N filtered out", pytest "N deselected", jest
/// "Ran all test suites matching ...") and from the arguments (pytest `-k`, `-m`, `--lf`,
/// `--deselect`, node ids and single files; `go test -run`/`-skip`; jest/vitest `-t`,
/// `--testNamePattern`, `--onlyChanged`, `--changed`, `--shard`, `related`, and file
/// filters; nextest filter expressions, `--partition` and name filters). Package or target
/// selection (`cargo test -p x`, `go test ./pkg/...`) is not treated as a subset.
pub fn subset_reason(words: &[String], notes: &[String]) -> Option<String> {
    for n in notes {
        let filtered = n.starts_with("cargo: ") && n.ends_with(" tests filtered out");
        let desel = n.starts_with("pytest: ") && n.ends_with(" deselected");
        if filtered || desel || n.starts_with("jest: selected a subset") {
            return Some(n.clone());
        }
    }
    let names: Vec<String> = words.iter().map(|w| prog_name(w)).collect();
    let found = |prog: &str| names.iter().position(|n| n == prog);
    let flag_in = |args: &[String], flags: &[&str]| -> Option<String> {
        args.iter().find(|a| flags.iter().any(|f| a.as_str() == *f || (f.starts_with("--") && a.starts_with(&format!("{f}="))))).cloned()
    };
    // A bare word following the runner (or a subcommand) rather than a flag's value.
    let positional = |args: &[String], skip: &[&str], pred: &dyn Fn(&str) -> bool| -> Option<String> {
        let mut prev_flag = false;
        for a in args {
            if a == "--" {
                break;
            }
            if a.starts_with('-') {
                prev_flag = !a.contains('=');
                continue;
            }
            if !prev_flag && !skip.contains(&a.as_str()) && pred(a) {
                return Some(a.clone());
            }
            prev_flag = false;
        }
        None
    };
    // pytest (also `python -m pytest`)
    if let Some(i) = found("pytest").or_else(|| found("py.test")) {
        let args = &words[i + 1..];
        if let Some(a) = args.iter().find(|a| a.contains("::")) {
            return Some(format!("pytest node id {a}"));
        }
        if let Some(f) = flag_in(args, &["-k", "-m", "--lf", "--last-failed", "--deselect", "--sw", "--stepwise"]) {
            return Some(format!("pytest {f}"));
        }
        if let Some(a) = args.iter().find(|a| (a.starts_with("-k") || a.starts_with("-m")) && a.len() > 2 && !a.starts_with("--")) {
            return Some(format!("pytest {a}"));
        }
        if let Some(p) = positional(args, &[], &|a: &str| a.ends_with(".py")) {
            return Some(format!("pytest file {p}"));
        }
    }
    // go test
    if let Some(i) = found("go") {
        if words.get(i + 1).map(|w| w.as_str()) == Some("test") {
            if let Some(f) = words[i + 2..].iter().find(|a| {
                let a = a.trim_start_matches('-');
                a == "run" || a == "skip" || a.starts_with("run=") || a.starts_with("skip=")
            }) {
                return Some(format!("go test {f}"));
            }
        }
    }
    // jest / vitest
    for prog in ["jest", "vitest"] {
        if let Some(i) = found(prog) {
            let args = &words[i + 1..];
            let flags = [
                "-t",
                "--testNamePattern",
                "--testPathPattern",
                "--testPathPatterns",
                "-o",
                "--onlyChanged",
                "--changed",
                "--changedSince",
                "--findRelatedTests",
                "--onlyFailures",
                "-f",
                "--shard",
                "--project",
            ];
            if let Some(f) = flag_in(args, &flags) {
                return Some(format!("{prog} {f}"));
            }
            if prog == "vitest" && args.first().map(|a| a.as_str()) == Some("related") {
                return Some("vitest related".to_string());
            }
            if let Some(p) = positional(args, &["run", "watch", "dev"], &|_| true) {
                return Some(format!("{prog} file filter {p}"));
            }
        }
    }
    // cargo nextest run
    if let Some(i) = found("nextest").or_else(|| found("cargo-nextest")) {
        let args = &words[i + 1..];
        if let Some(f) = flag_in(args, &["-E", "--filterset", "--filter-expr", "--partition"]) {
            return Some(format!("nextest {f}"));
        }
        if let Some(p) = positional(args, &["run", "r"], &|_| true) {
            return Some(format!("nextest name filter {p}"));
        }
        if let Some(j) = args.iter().position(|a| a == "--") {
            if let Some(p) = args[j + 1..].iter().find(|a| !a.starts_with('-')) {
                return Some(format!("nextest name filter {p}"));
            }
        }
    }
    // cargo test: name filters show up as "filtered out" in the output; --ignored runs only ignored tests.
    if found("cargo").is_some() && words.iter().any(|w| w == "--ignored") {
        return Some("libtest --ignored".to_string());
    }
    None
}

/// Detect invocations that list or collect tests without running them.
pub fn is_collect_only(argv: &[String]) -> Option<&'static str> {
    let has = |f: &str| argv.iter().any(|a| a == f);
    let joined: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    if has("--collect-only") || has("--co") {
        return Some("--collect-only lists tests without running them");
    }
    if has("--listTests") || has("--list-tests") {
        return Some("listing tests does not run them");
    }
    if has("--no-run") {
        return Some("--no-run builds tests without running them");
    }
    if joined.windows(2).any(|w| w[0] == "nextest" && w[1] == "list") {
        return Some("nextest list does not run tests");
    }
    // `cargo test -- --list`
    if let Some(i) = joined.iter().position(|a| *a == "--") {
        if joined[i + 1..].contains(&"--list") {
            return Some("libtest --list does not run tests");
        }
    }
    if has("--passWithNoTests") {
        return Some("--passWithNoTests lets an empty run pass");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsets() {
        let w = |s: &str| command_words(&[s.to_string()], true);
        let none: Vec<String> = Vec::new();
        for full in [
            "cargo test",
            "cargo test --workspace -p foo",
            "python -m pytest -q",
            "pytest tests/",
            "pytest -x -q --maxfail 3",
            "go test ./...",
            "go test -v -count=1 ./pkg/...",
            "npx jest",
            "npx jest --ci --coverage",
            "npx vitest run",
            "cargo nextest run --workspace",
            "cargo nextest run -p foo --profile ci",
            "npm test",
        ] {
            assert_eq!(subset_reason(&w(full), &none), None, "{full}");
        }
        for sub in [
            "python -m pytest -q test_a.py::test_a",
            "pytest -k slow",
            "pytest -kslow",
            "pytest -m 'not slow'",
            "pytest --lf",
            "pytest test_a.py",
            "go test -run TestX ./...",
            "go test -run=TestX ./...",
            "npx jest -t adds",
            "npx jest src/math",
            "npx vitest run src/math",
            "npx vitest related src/a.ts",
            "cargo nextest run adds",
            "cargo nextest run -E test(adds)",
            "cargo test -- --ignored",
        ] {
            assert!(subset_reason(&w(sub), &none).is_some(), "{sub}");
        }
        assert!(subset_reason(&w("cargo test"), &["cargo: 12 tests filtered out".into()]).is_some());
        assert!(subset_reason(&w("pytest"), &["pytest: 2 deselected".into()]).is_some());
        let s = parse("Tests:       3 passed, 3 total\nRan all test suites matching /math/i.\n", &[]);
        assert!(subset_reason(&[], &s.notes).is_some());
        let s = parse("Tests:       3 passed, 3 total\nRan all test suites.\n", &[]);
        assert!(subset_reason(&[], &s.notes).is_none());
        let s = parse("===== 3 passed, 2 deselected in 0.02s =====\n", &[]);
        assert!(subset_reason(&[], &s.notes).is_some());
    }

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn cargo_pass_and_doctest() {
        let out = "running 2 tests\ntest a::one ... ok\ntest a::two ... ignored, slow\n\ntest result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n   Doc-tests x\n\nrunning 1 test\ntest src/lib.rs - f (line 3) ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s\n";
        let s = parse(out, &argv("cargo test"));
        assert_eq!(s.runners, vec![Runner::Cargo]);
        assert_eq!((s.passed, s.failed, s.skipped), (2, 0, 1));
        assert_eq!(s.passed_names, vec!["a::one", "src/lib.rs - f (line 3)"]);
    }

    #[test]
    fn cargo_failure() {
        let out = "test t::bad ... FAILED\n\nfailures:\n\ntest result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let s = parse(out, &[]);
        assert_eq!((s.passed, s.failed), (3, 1));
        assert_eq!(s.failed_names, vec!["t::bad"]);
    }

    #[test]
    fn cargo_filtered_to_nothing() {
        let out = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.00s\n";
        let s = parse(out, &[]);
        assert!(s.recognized());
        assert_eq!(s.passed, 0);
        assert!(s.notes.iter().any(|n| n.contains("12 tests filtered out")));
    }

    #[test]
    fn nextest() {
        let out = "    Starting 3 tests across 1 binary (1 test skipped)\n        PASS [   0.004s] mycrate tests::a\n        FAIL [   0.005s] mycrate tests::b\n        PASS [   0.003s] mycrate tests::c\n------------\n     Summary [   0.010s] 3 tests run: 2 passed, 1 failed, 1 skipped\n";
        let s = parse(out, &[]);
        assert_eq!(s.runners, vec![Runner::Nextest]);
        assert_eq!((s.passed, s.failed, s.skipped), (2, 1, 1));
        assert_eq!(s.failed_names, vec!["mycrate tests::b"]);
    }

    #[test]
    fn pytest_variants() {
        let s = parse("============ 3 passed, 1 skipped, 2 warnings in 0.12s ============\n", &[]);
        assert_eq!((s.passed, s.failed, s.skipped), (3, 0, 1));
        let s = parse("1 failed, 2 passed, 1 error in 0.50s\n", &[]);
        assert_eq!((s.passed, s.failed), (2, 2));
        let s = parse("===== no tests ran in 0.01s =====\n", &[]);
        assert!(s.recognized());
        assert_eq!(s.passed, 0);
        let s = parse("========= 4 deselected in 0.02s =========\n", &[]);
        assert!(s.recognized());
        assert_eq!(s.passed, 0);
        let s = parse("== 1 passed, 1 xfailed, 1 xpassed in 0.1s ==\n", &[]);
        assert_eq!((s.passed, s.skipped, s.warned), (1, 1, 1));
    }

    #[test]
    fn pytest_names_verbose_and_xdist() {
        let out = "tests/test_a.py::test_one PASSED                [ 50%]\ntests/test_a.py::test_two FAILED                [100%]\n[gw1] [ 66%] PASSED tests/test_b.py::test_x\n=========================== short test summary info ============================\nFAILED tests/test_a.py::test_two - assert 1 == 2\n========================= 1 failed, 2 passed in 0.31s =========================\n";
        let s = parse(out, &[]);
        assert_eq!(s.passed_names, vec!["tests/test_a.py::test_one", "tests/test_b.py::test_x"]);
        assert_eq!(s.failed_names, vec!["tests/test_a.py::test_two"]);
        assert_eq!((s.passed, s.failed), (2, 1));
    }

    #[test]
    fn jest() {
        let out = "  ● math › adds\n\nTest Suites: 1 failed, 1 total\nTests:       1 failed, 1 skipped, 2 passed, 4 total\nSnapshots:   0 total\n";
        let s = parse(out, &[]);
        assert_eq!(s.runners, vec![Runner::Jest]);
        assert_eq!((s.passed, s.failed, s.skipped), (2, 1, 1));
        assert_eq!(s.failed_names, vec!["math › adds"]);
        let s = parse("No tests found, exiting with code 1\n", &[]);
        assert!(s.recognized());
        assert_eq!(s.passed, 0);
    }

    #[test]
    fn vitest() {
        let out = " FAIL  src/a.test.ts > math > adds\n Test Files  1 failed | 1 passed (2)\n      Tests  1 failed | 5 passed | 2 skipped (8)\n   Start at  12:00:00\n";
        let s = parse(out, &[]);
        assert_eq!(s.runners, vec![Runner::Vitest]);
        assert_eq!((s.passed, s.failed, s.skipped), (5, 1, 2));
        assert_eq!(s.failed_names, vec!["src/a.test.ts > math > adds"]);
    }

    #[test]
    fn go_verbose_and_packages() {
        let out = "=== RUN   TestA\n--- PASS: TestA (0.00s)\n=== RUN   TestB\n--- FAIL: TestB (0.00s)\nFAIL\nFAIL\texample.com/m\t0.002s\n";
        let s = parse(out, &argv("go test -v ./..."));
        assert_eq!((s.passed, s.failed), (1, 1));
        let out = "ok  \texample.com/m/a\t0.010s\nok  \texample.com/m/b\t(cached)\n?   \texample.com/m/c\t[no test files]\n";
        let s = parse(out, &argv("go test ./..."));
        assert_eq!((s.passed, s.failed), (2, 0));
        assert!(s.notes[0].contains("packages"));
        let out = "ok  \texample.com/m/a\t0.010s [no tests to run]\n";
        let s = parse(out, &argv("go test -run Nope ./..."));
        assert!(s.recognized());
        assert_eq!(s.passed, 0);
    }

    #[test]
    fn go_json() {
        let out = "{\"Action\":\"run\",\"Package\":\"m\",\"Test\":\"TestA\"}\n{\"Action\":\"pass\",\"Package\":\"m\",\"Test\":\"TestA\",\"Elapsed\":0}\n{\"Action\":\"skip\",\"Package\":\"m\",\"Test\":\"TestS\"}\n{\"Action\":\"pass\",\"Package\":\"m\",\"Elapsed\":0.1}\n";
        let s = parse(out, &[]);
        assert_eq!((s.passed, s.failed, s.skipped), (1, 0, 1));
        assert_eq!(s.passed_names, vec!["m.TestA"]);
    }

    #[test]
    fn unrecognised() {
        let s = parse("All tests passed!\nEverything is fine.\n", &[]);
        assert!(!s.recognized());
    }

    #[test]
    fn collect_only() {
        assert!(is_collect_only(&argv("pytest --collect-only")).is_some());
        assert!(is_collect_only(&argv("cargo test --no-run")).is_some());
        assert!(is_collect_only(&argv("cargo test -- --list")).is_some());
        assert!(is_collect_only(&argv("cargo nextest list")).is_some());
        assert!(is_collect_only(&argv("jest --passWithNoTests")).is_some());
        assert!(is_collect_only(&argv("cargo test --workspace")).is_none());
    }
}
