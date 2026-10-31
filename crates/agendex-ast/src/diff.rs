//! Unified diff generator for comparing and reviewing proposed code changes.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

/// Computes line-level difference using the Myers diff algorithm.
pub fn myers_diff<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<DiffOp<'a>> {
    let n = a.len();
    let m = b.len();
    let max = n + m;
    if max == 0 {
        return Vec::new();
    }

    let mut v = vec![0isize; 2 * max + 1];
    let offset = max as isize;
    let mut trace: Vec<Vec<isize>> = Vec::new();

    let mut found = false;
    for d in 0..=(max as isize) {
        trace.push(v.clone());
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d
                || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize])
            {
                v[(k + 1 + offset) as usize]
            } else {
                v[(k - 1 + offset) as usize] + 1
            };
            let mut y = x - k;
            while x < n as isize && y < m as isize && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(k + offset) as usize] = x;
            if x >= n as isize && y >= m as isize {
                found = true;
                break;
            }
        }
        if found {
            break;
        }
    }

    // Backtrack to assemble diff operations
    let mut x = n as isize;
    let mut y = m as isize;
    let mut ops = Vec::new();

    for d in (0..trace.len() as isize).rev() {
        let v_prev = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d
            || (k != d && v_prev[(k - 1 + offset) as usize] < v_prev[(k + 1 + offset) as usize])
        {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v_prev[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;

        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops.push(DiffOp::Equal(a[x as usize]));
        }

        if d > 0 {
            if x == prev_x {
                y -= 1;
                ops.push(DiffOp::Insert(b[y as usize]));
            } else if y == prev_y {
                x -= 1;
                ops.push(DiffOp::Delete(a[x as usize]));
            }
        }
    }

    ops.reverse();
    ops
}

/// Generates a standard unified diff string between `old_content` and `new_content`.
/// If both contents are identical, returns an empty string `""`.
pub fn generate_unified_diff(file_path: &str, old_content: &str, new_content: &str) -> String {
    if old_content == new_content {
        return String::new();
    }

    let normalized_path = file_path.replace('\\', "/");
    let old_lines: Vec<&str> = if old_content.is_empty() {
        Vec::new()
    } else {
        old_content.lines().collect()
    };
    let new_lines: Vec<&str> = if new_content.is_empty() {
        Vec::new()
    } else {
        new_content.lines().collect()
    };

    let ops = myers_diff(&old_lines, &new_lines);

    // Group diff ops into hunks with up to 3 lines of context
    let context_size = 3;
    let mut hunks: Vec<Hunk> = Vec::new();

    // Identify indices of change operations
    let change_indices: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(idx, op)| match op {
            DiffOp::Equal(_) => None,
            _ => Some(idx),
        })
        .collect();

    if change_indices.is_empty() {
        return String::new();
    }

    // Cluster change indices within 2 * context_size of each other
    let mut clusters: Vec<(usize, usize)> = Vec::new();
    for &ch_idx in &change_indices {
        let start = ch_idx.saturating_sub(context_size);
        let end = (ch_idx + context_size + 1).min(ops.len());
        if let Some(last) = clusters.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        clusters.push((start, end));
    }

    // Build hunks from clusters
    let mut old_line_num = 1usize;
    let mut new_line_num = 1usize;
    let mut current_op_idx = 0usize;

    for (cluster_start, cluster_end) in clusters {
        // Advance line numbers up to cluster_start
        while current_op_idx < cluster_start {
            match ops[current_op_idx] {
                DiffOp::Equal(_) => {
                    old_line_num += 1;
                    new_line_num += 1;
                }
                DiffOp::Delete(_) => {
                    old_line_num += 1;
                }
                DiffOp::Insert(_) => {
                    new_line_num += 1;
                }
            }
            current_op_idx += 1;
        }

        let hunk_old_start = old_line_num;
        let hunk_new_start = new_line_num;
        let mut hunk_old_count = 0usize;
        let mut hunk_new_count = 0usize;
        let mut hunk_lines = Vec::new();

        while current_op_idx < cluster_end {
            match ops[current_op_idx] {
                DiffOp::Equal(line) => {
                    hunk_lines.push(format!(" {}", line));
                    old_line_num += 1;
                    new_line_num += 1;
                    hunk_old_count += 1;
                    hunk_new_count += 1;
                }
                DiffOp::Delete(line) => {
                    hunk_lines.push(format!("-{}", line));
                    old_line_num += 1;
                    hunk_old_count += 1;
                }
                DiffOp::Insert(line) => {
                    hunk_lines.push(format!("+{}", line));
                    new_line_num += 1;
                    hunk_new_count += 1;
                }
            }
            current_op_idx += 1;
        }

        hunks.push(Hunk {
            old_start: hunk_old_start,
            old_count: hunk_old_count,
            new_start: hunk_new_start,
            new_count: hunk_new_count,
            lines: hunk_lines,
        });
    }

    let mut result = String::new();
    let clean_path = normalized_path
        .trim_start_matches('/')
        .trim_start_matches("./");
    let clean_path = clean_path.strip_prefix("workspace/").unwrap_or(clean_path);
    result.push_str(&format!("--- a/{}\n", clean_path));
    result.push_str(&format!("+++ b/{}\n", clean_path));

    for hunk in hunks {
        result.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));
        for line in hunk.lines {
            result.push_str(&line);
            result.push('\n');
        }
    }

    result
}

struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_content_returns_empty() {
        let diff = generate_unified_diff("src/main.rs", "fn main() {}\n", "fn main() {}\n");
        assert!(diff.is_empty());
    }

    #[test]
    fn test_single_line_replacement() {
        let old = "fn main() {\n    println!(\"Hello\");\n}\n";
        let new = "fn main() {\n    println!(\"Hello, world!\");\n}\n";
        let diff = generate_unified_diff("src/main.rs", old, new);

        assert!(diff.contains("--- a/src/main.rs"));
        assert!(diff.contains("+++ b/src/main.rs"));
        assert!(diff.contains("@@ -1,3 +1,3 @@"));
        assert!(diff.contains("-    println!(\"Hello\");"));
        assert!(diff.contains("+    println!(\"Hello, world!\");"));
    }

    #[test]
    fn test_insertion_and_deletion() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline_new\nline3\nline4\n";
        let diff = generate_unified_diff("test.txt", old, new);

        assert!(diff.contains("-line2"));
        assert!(diff.contains("+line_new"));
        assert!(diff.contains("+line4"));
    }
}
