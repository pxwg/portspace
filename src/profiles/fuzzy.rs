//! Pi-style bounded normalization matching, not semantic or edit-distance matching.
use super::pi::EditInput;
use crate::{ErrorCode, Result, WorkspaceError};
use unicode_normalization::UnicodeNormalization;

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
fn fuzzy(text: &str) -> String {
    text.nfkc()
        .collect::<String>()
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .map(|c| match c {
            '\u{2018}'..='\u{201b}' => '\'',
            '\u{201c}'..='\u{201f}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            _ => c,
        })
        .collect()
}
fn invalid(code: ErrorCode, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::new(code, message)
}
struct Match {
    start: usize,
    end: usize,
    replacement: String,
    index: usize,
}
fn apply(base: &str, matches: &[Match], offset: usize) -> String {
    let mut result = base.to_string();
    for m in matches.iter().rev() {
        result.replace_range(m.start - offset..m.end - offset, &m.replacement);
    }
    result
}

pub fn edit_text(original: &str, input: &EditInput) -> Result<String> {
    if input.edits.is_empty() {
        return Err(invalid(
            ErrorCode::InvalidInput,
            "Edit tool input is invalid. edits must contain at least one replacement.",
        ));
    }
    let (bom, text) = original
        .strip_prefix('\u{feff}')
        .map(|s| ("\u{feff}", s))
        .unwrap_or(("", original));
    let crlf = text
        .find("\r\n")
        .zip(text.find('\n'))
        .is_some_and(|(a, b)| a < b);
    let original_lf = lf(text);
    let edits: Vec<_> = input
        .edits
        .iter()
        .map(|e| (lf(&e.old_text), lf(&e.new_text)))
        .collect();
    if edits.iter().any(|(old, _)| old.is_empty()) {
        return Err(invalid(
            ErrorCode::InvalidInput,
            "oldText must not be empty",
        ));
    }
    let normalized = fuzzy(&original_lf);
    let use_fuzzy = edits.iter().any(|(old, _)| {
        !original_lf.contains(old) && !fuzzy(old).is_empty() && normalized.contains(&fuzzy(old))
    });
    let base = if use_fuzzy { &normalized } else { &original_lf };
    let mut matches = Vec::new();
    for (index, (old, new)) in edits.iter().enumerate() {
        let normalized_old = fuzzy(old);
        // Whitespace-only fuzzy needles have no safe positional interpretation.
        let found = base.find(old).map(|at| (at, old.len())).or_else(|| {
            if use_fuzzy && !normalized_old.is_empty() {
                base.find(&normalized_old)
                    .map(|at| (at, normalized_old.len()))
            } else {
                None
            }
        });
        let (start, length) = found.ok_or_else(|| invalid(ErrorCode::Conflict, format!("Could not find edits[{index}] in {}. The oldText must match exactly or under supported Unicode/whitespace normalization.", input.path)))?;
        // Pi checks duplicates in normalized space even when an exact match exists.
        let count = if normalized_old.is_empty() {
            base.match_indices(old).count()
        } else {
            normalized.match_indices(&normalized_old).count()
        };
        if count > 1 {
            return Err(invalid(
                ErrorCode::AmbiguousEdit,
                format!(
                    "Found {count} occurrences of edits[{index}] in {}. Each oldText must be unique.",
                    input.path
                ),
            ));
        }
        matches.push(Match {
            start,
            end: start + length,
            replacement: new.clone(),
            index,
        });
    }
    matches.sort_by_key(|m| m.start);
    for pair in matches.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(invalid(
                ErrorCode::Conflict,
                format!(
                    "edits[{}] and edits[{}] overlap in {}",
                    pair[0].index, pair[1].index, input.path
                ),
            ));
        }
    }
    let updated = if !use_fuzzy {
        apply(base, &matches, 0)
    } else {
        // Rewrite only affected line groups from normalized space. Unchanged lines
        // retain original Unicode and trailing whitespace, even around duplicates.
        let original_lines: Vec<_> = original_lf.split_inclusive('\n').collect();
        let base_lines: Vec<_> = base.split_inclusive('\n').collect();
        if original_lines.len() != base_lines.len() {
            return Err(invalid(
                ErrorCode::Conflict,
                "normalization changed line structure",
            ));
        }
        let mut at = 0;
        let spans: Vec<_> = base_lines
            .iter()
            .map(|l| {
                let start = at;
                at += l.len();
                (start, at)
            })
            .collect();
        let mut groups: Vec<(usize, usize, usize, usize)> = Vec::new();
        for (i, m) in matches.iter().enumerate() {
            let first = spans
                .iter()
                .position(|(start, end)| m.start >= *start && m.start < *end)
                .ok_or_else(|| invalid(ErrorCode::Conflict, "invalid normalized match range"))?;
            let last = spans
                .iter()
                .position(|(_, end)| *end >= m.end)
                .ok_or_else(|| invalid(ErrorCode::Conflict, "invalid normalized match end"))?
                + 1;
            if let Some(group) = groups.last_mut().filter(|g| first < g.1) {
                group.1 = group.1.max(last);
                group.3 = i + 1;
            } else {
                groups.push((first, last, i, i + 1));
            }
        }
        let mut result = String::new();
        let mut line = 0;
        for (first, last, mi, mj) in groups {
            result.extend(original_lines[line..first].iter().copied());
            let start = spans[first].0;
            result.push_str(&apply(
                &base[start..spans[last - 1].1],
                &matches[mi..mj],
                start,
            ));
            line = last;
        }
        result.extend(original_lines[line..].iter().copied());
        result
    };
    if updated == original_lf {
        return Err(invalid(
            ErrorCode::Conflict,
            format!(
                "No changes made to {}. The replacement produced identical content.",
                input.path
            ),
        ));
    }
    Ok(format!(
        "{bom}{}",
        if crlf {
            updated.replace('\n', "\r\n")
        } else {
            updated
        }
    ))
}
