//! Collecting and printing differences, grouped by document kind.

use std::collections::BTreeMap;

#[derive(Default)]
pub struct Section {
    /// Documents compared.
    pub compared: usize,
    /// One line per same-everywhere fact.
    pub notes: Vec<String>,
    /// Distinct differences, each with the documents that showed it.
    pub differences: BTreeMap<String, Vec<String>>,
}

#[derive(Default)]
pub struct Report {
    pub sections: BTreeMap<String, Section>,
    order: Vec<String>,
    /// Every per-document emitted-bytes diff, raw, printed after the grouped report.
    pub appendix: Vec<String>,
}

impl Report {
    pub fn section(&mut self, kind: &str) -> &mut Section {
        if !self.sections.contains_key(kind) {
            self.order.push(kind.to_owned());
        }
        self.sections.entry(kind.to_owned()).or_default()
    }

    pub fn print(&self) {
        println!("\n# DIFFERENCES BY DOCUMENT KIND");
        for kind in &self.order {
            let s = &self.sections[kind];
            println!("\n## {kind}  (compared: {}, distinct differences: {})", s.compared, s.differences.len());
            for note in &s.notes {
                println!("  note: {note}");
            }
            if s.differences.is_empty() {
                println!("  NO DIFFERENCES");
            }
            for (i, (what, docs)) in s.differences.iter().enumerate() {
                println!("\n  [{kind} #{}] seen in {} document(s): {}", i + 1, docs.len(), summarize(docs));
                for line in what.lines() {
                    println!("    {line}");
                }
            }
        }
        println!("\n# APPENDIX: every per-document emitted-bytes difference, raw ({} entries)", self.appendix.len());
        for entry in &self.appendix {
            println!("\n{entry}");
        }
    }
}

impl Section {
    /// Record one difference seen in `doc`; a document is listed once per difference.
    pub fn differ(&mut self, what: String, doc: &str) {
        let docs = self.differences.entry(what).or_default();
        if !docs.iter().any(|d| d == doc) {
            docs.push(doc.to_owned());
        }
    }
}

fn summarize(docs: &[String]) -> String {
    if docs.len() <= 4 {
        docs.join(", ")
    } else {
        format!("{}, ... (+{} more)", docs[..3].join(", "), docs.len() - 3)
    }
}

/// Line hunks (removed, added) of an LCS diff of `before` and `after`.
pub fn hunks(before: &str, after: &str) -> Vec<(Vec<String>, Vec<String>)> {
    let a: Vec<&str> = before.split_inclusive('\n').collect();
    let b: Vec<&str> = after.split_inclusive('\n').collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    let mut cur: (Vec<String>, Vec<String>) = Default::default();
    let line = |s: &str| s.strip_suffix('\n').unwrap_or(s).to_owned();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
            if !cur.0.is_empty() || !cur.1.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            cur.0.push(line(a[i]));
            i += 1;
        } else {
            cur.1.push(line(b[j]));
            j += 1;
        }
    }
    if !cur.0.is_empty() || !cur.1.is_empty() {
        out.push(cur);
    }
    out
}

/// The hunks as text: `- today`, `+ after`, hunks separated by `~`.
pub fn line_diff(before: &str, after: &str) -> String {
    hunks(before, after)
        .iter()
        .map(|(r, a)| {
            r.iter()
                .map(|l| format!("- {l:?}"))
                .chain(a.iter().map(|l| format!("+ {l:?}")))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n~\n")
}

fn style(scalar: &str) -> &'static str {
    match scalar.chars().next() {
        Some('\'') => "single-quoted",
        Some('"') => "double-quoted",
        Some('|') => "literal block",
        Some('>') => "folded block",
        None => "empty",
        _ => "plain",
    }
}

fn split_scalar(line: &str) -> (String, String) {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let (dash, body) = match body.strip_prefix("- ") {
        Some(rest) => ("- ", rest),
        None => ("", body),
    };
    if !body.starts_with('\'') && !body.starts_with('"') {
        if let Some((key, value)) = body.split_once(": ") {
            return (format!("{indent}{dash}{key}: "), value.to_owned());
        }
    }
    (format!("{indent}{dash}"), body.to_owned())
}

fn shape(value: &str) -> &'static str {
    let v = value.trim_matches(|c| c == '"' || c == '\'');
    let b = v.as_bytes();
    if b.len() >= 10 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit) {
        " (a timestamp-like string)"
    } else if v.is_empty() {
        " (the empty string)"
    } else {
        ""
    }
}

/// What kind of change each hunk is, with the hunk as its example.
pub fn categorize(before: &str, after: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (removed, added) in hunks(before, after) {
        let example = removed
            .iter()
            .map(|l| format!("- {l:?}"))
            .chain(added.iter().map(|l| format!("+ {l:?}")))
            .collect::<Vec<_>>()
            .join("\n");
        if removed.len() == added.len() {
            for (r, a) in removed.iter().zip(&added) {
                let ex = format!("- {r:?}\n+ {a:?}");
                if r.trim().is_empty() && a.trim().is_empty() {
                    out.push(("an empty line inside a literal block scalar: today has no indentation on it, serde-saphyr writes the block's indentation".into(), ex));
                    continue;
                }
                let (rp, rs) = split_scalar(r);
                let (ap, as_) = split_scalar(a);
                if rp == ap {
                    out.push((format!("scalar style: today {} -> serde-saphyr {}{}", style(&rs), style(&as_), shape(&rs)), ex));
                } else {
                    out.push(("other line change".into(), ex));
                }
            }
        } else if added.iter().any(|l| l.ends_with(": >-") || l.ends_with("- >-") || l.ends_with(": >") ) {
            out.push(("a long single-line string (over 80 chars) is folded into a `>-` block scalar".into(), example));
        } else if added.iter().any(|l| l.ends_with(": |") || l.ends_with(": |-") || l.ends_with(": |+")) || removed.iter().any(|l| l.ends_with(": |") || l.ends_with(": |-")) {
            out.push(("a multi-line string's style changes".into(), example));
        } else {
            out.push(("other multi-line change".into(), example));
        }
    }
    out
}
