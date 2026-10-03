//! Minimal line-based unified diff (LCS), no external dependencies.
//! Used to show the user what write_file / edit_file actually changed.

pub struct FileDiff {
    /// Unified diff body with `@@` hunk headers; no file-name header.
    pub text: String,
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Del,
    Ins,
}

/// LCS backtrace over two line slices. Quadratic memory — callers cap input.
fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let n = a.len();
    let m = b.len();
    let w = m + 1;
    let mut dp = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * w + j] = if a[i] == b[j] {
                dp[(i + 1) * w + j + 1] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Eq);
            i += 1;
            j += 1;
        } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
            ops.push(Op::Del);
            i += 1;
        } else {
            ops.push(Op::Ins);
            j += 1;
        }
    }
    ops.extend(std::iter::repeat_n(Op::Del, n - i));
    ops.extend(std::iter::repeat_n(Op::Ins, m - j));
    ops
}

const MAX_LCS_CELLS: usize = 2_000_000;

/// Unified diff with `ctx` context lines. None when the content is identical.
pub fn unified_diff(old: &str, new: &str, ctx: usize) -> Option<FileDiff> {
    if old == new {
        return None;
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();

    // Trim the common prefix/suffix so the quadratic LCS only sees the core.
    let mut p = 0usize;
    while p < a.len() && p < b.len() && a[p] == b[p] {
        p += 1;
    }
    let mut s = 0usize;
    while s < a.len() - p && s < b.len() - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
        s += 1;
    }
    let (mid_a, mid_b) = (&a[p..a.len() - s], &b[p..b.len() - s]);

    if mid_a.len().saturating_mul(mid_b.len()) > MAX_LCS_CELLS {
        return Some(FileDiff {
            text: format!("(diff too large to display: ~{} old / ~{} new lines in the changed region)", mid_a.len(), mid_b.len()),
            added: mid_b.len(),
            removed: mid_a.len(),
        });
    }

    // Flatten to (op, line) with running old/new line counters per item.
    #[derive(Clone)]
    struct It {
        op: Op,
        t: String,
        a_before: usize,
        b_before: usize,
    }
    let mut items: Vec<It> = Vec::with_capacity(a.len() + b.len());
    // Re-attach the trimmed common prefix/suffix as equal lines so hunks
    // have real context to show around the changes.
    for (k, line) in a.iter().take(p).enumerate() {
        items.push(It { op: Op::Eq, t: line.to_string(), a_before: k, b_before: k });
    }
    let (mut ia, mut ib) = (p, p);
    for op in lcs_ops(mid_a, mid_b) {
        match op {
            Op::Eq => {
                items.push(It { op, t: mid_a[ia - p].to_string(), a_before: ia, b_before: ib });
                ia += 1;
                ib += 1;
            }
            Op::Del => {
                items.push(It { op, t: mid_a[ia - p].to_string(), a_before: ia, b_before: ib });
                ia += 1;
            }
            Op::Ins => {
                items.push(It { op, t: mid_b[ib - p].to_string(), a_before: ia, b_before: ib });
                ib += 1;
            }
        }
    }
    for k in 0..s {
        let ai = a.len() - s + k;
        items.push(It { op: Op::Eq, t: a[ai].to_string(), a_before: ai, b_before: b.len() - s + k });
    }
    let added = items.iter().filter(|i| i.op == Op::Ins).count();
    let removed = items.iter().filter(|i| i.op == Op::Del).count();
    if added + removed == 0 {
        return None;
    }

    // Group changed items into hunks, merging across gaps of ≤ 2·ctx equal lines.
    let changed: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i.op != Op::Eq).map(|(k, _)| k).collect();
    let mut text = String::new();
    let mut g = 0usize;
    while g < changed.len() {
        let start0 = changed[g];
        let mut end0 = changed[g];
        let mut h = g + 1;
        while h < changed.len() && changed[h] - end0 <= ctx * 2 {
            end0 = changed[h];
            h += 1;
        }
        let start = start0.saturating_sub(ctx);
        let end = (end0 + ctx).min(items.len() - 1);

        // Hunk bounds in each file, 1-based; an empty side starts at the line before.
        let a_len = items[start..=end].iter().filter(|i| i.op != Op::Ins).count();
        let b_len = items[start..=end].iter().filter(|i| i.op != Op::Del).count();
        let a_start = if a_len == 0 { items[start].a_before } else { items[start].a_before + 1 };
        let b_start = if b_len == 0 { items[start].b_before } else { items[start].b_before + 1 };
        text.push_str(&format!("@@ -{a_start},{a_len} +{b_start},{b_len} @@\n"));
        for it in &items[start..=end] {
            let sign = match it.op {
                Op::Eq => ' ',
                Op::Del => '-',
                Op::Ins => '+',
            };
            text.push(sign);
            text.push_str(&it.t);
            text.push('\n');
        }
        g = h;
    }
    Some(FileDiff { text, added, removed })
}

#[cfg(test)]
mod tests {
    use super::unified_diff;

    #[test]
    fn identical_is_no_diff() {
        assert!(unified_diff("a\nb\nc", "a\nb\nc", 3).is_none());
        assert!(unified_diff("", "", 3).is_none());
    }

    #[test]
    fn single_line_change_with_context() {
        let d = unified_diff("one\ntwo\nthree\nfour\nfive\nsix\neight", "one\ntwo\nTHREE\nfour\nfive\nsix\neight", 1).unwrap();
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
        let lines: Vec<&str> = d.text.lines().collect();
        assert_eq!(lines[0], "@@ -2,3 +2,3 @@");
        assert_eq!(lines[1], " two");
        assert_eq!(lines[2], "-three");
        assert_eq!(lines[3], "+THREE");
        assert_eq!(lines[4], " four");
    }

    #[test]
    fn new_file_is_all_additions() {
        let d = unified_diff("", "hello\nworld", 3).unwrap();
        assert_eq!((d.added, d.removed), (2, 0));
        assert!(d.text.starts_with("@@ -0,0 +1,2 @@\n"));
        assert!(d.text.contains("+hello"));
    }

    #[test]
    fn deletion_of_last_lines() {
        let d = unified_diff("a\nb\nc\nd", "a\nb", 3).unwrap();
        assert_eq!((d.added, d.removed), (0, 2));
        assert!(d.text.contains("-c"));
        assert!(d.text.contains("-d"));
    }

    #[test]
    fn separate_hunks_when_far_apart() {
        let old: Vec<String> = (1..=30).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        new[0] = "CHANGED".into();
        new[29] = "ALSO".into();
        let d = unified_diff(&old.join("\n"), &new.join("\n"), 3).unwrap();
        assert_eq!(d.text.lines().filter(|l| l.starts_with("@@ ")).count(), 2);
    }

    #[test]
    fn huge_change_falls_back_without_panicking() {
        let old: String = (0..5000).map(|i| format!("o{i}\n")).collect();
        let new: String = (0..5000).map(|i| format!("n{i}\n")).collect();
        let d = unified_diff(&old, &new, 3).unwrap();
        assert!(d.text.contains("diff too large"));
    }
}
