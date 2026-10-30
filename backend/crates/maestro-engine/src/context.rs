//! Context compilation: bounded project instructions and transcript
//! compaction that always keeps the newest entries.

use std::path::Path;

/// Total character budget for concatenated project instructions.
pub const INSTRUCTIONS_BUDGET: usize = 16_000;

/// Minimum transcript budget honored by [`compact_transcript`].
pub const TRANSCRIPT_FLOOR_CHARS: usize = 4_000;

/// Instruction files read (in priority order) by
/// [`load_project_instructions`].
pub const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "MAESTRO.md", ".maestro/instructions.md"];

/// Marker appended when a section is cut to fit its budget.
pub const TRUNCATION_MARKER: &str = "\n…[truncated]";

/// Concatenate `AGENTS.md`, `MAESTRO.md`, and `.maestro/instructions.md`
/// (those that exist) under a 16k character budget, each with a provenance
/// header, cutting with a truncation marker when the budget runs out.
/// Missing files are silently skipped; the result may be empty.
pub fn load_project_instructions(root: &Path) -> String {
    let mut out = String::new();
    let mut remaining = INSTRUCTIONS_BUDGET;

    for rel in INSTRUCTION_FILES {
        let path = root.join(rel);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        let header = format!("## From {rel}\n");
        let overhead = header.chars().count() + 1; // trailing newline
        if remaining <= overhead + TRUNCATION_MARKER.chars().count() {
            break;
        }
        out.push_str(&header);
        let budget = remaining - overhead;
        let body_len = content.chars().count();
        if body_len <= budget {
            out.push_str(content);
            remaining -= overhead + body_len;
        } else {
            let keep = budget.saturating_sub(TRUNCATION_MARKER.chars().count());
            let clipped: String = content.chars().take(keep).collect();
            out.push_str(&clipped);
            out.push_str(TRUNCATION_MARKER);
            remaining = 0;
        }
        out.push('\n');
        if remaining == 0 {
            break;
        }
    }
    out
}

/// What produced a transcript entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptKind {
    /// A model turn (its raw JSON reply).
    Turn,
    /// A tool result fed back to the model.
    ToolResult,
}

/// One line of an attempt's transcript.
#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub kind: TranscriptKind,
    pub text: String,
}

impl TranscriptEntry {
    pub fn turn(text: impl Into<String>) -> Self {
        TranscriptEntry {
            kind: TranscriptKind::Turn,
            text: text.into(),
        }
    }
    pub fn tool_result(text: impl Into<String>) -> Self {
        TranscriptEntry {
            kind: TranscriptKind::ToolResult,
            text: text.into(),
        }
    }
}

/// Keep the NEWEST contiguous entries fitting `max_chars` (floored at 4k;
/// the newest entry is always kept even when it alone exceeds the budget).
/// Returns the kept entries in chronological order plus the omitted count.
pub fn compact_transcript(
    entries: &[TranscriptEntry],
    max_chars: usize,
) -> (Vec<&TranscriptEntry>, usize) {
    let budget = max_chars.max(TRANSCRIPT_FLOOR_CHARS);
    let mut kept: Vec<&TranscriptEntry> = Vec::new();
    let mut used = 0usize;

    for entry in entries.iter().rev() {
        let len = entry.text.chars().count();
        if kept.is_empty() || used + len <= budget {
            used += len;
            kept.push(entry);
        } else {
            break;
        }
    }
    kept.reverse();
    let omitted = entries.len() - kept.len();
    (kept, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_concatenated_with_provenance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "agents rules").unwrap();
        std::fs::create_dir_all(dir.path().join(".maestro")).unwrap();
        std::fs::write(dir.path().join(".maestro/instructions.md"), "local notes").unwrap();

        let out = load_project_instructions(dir.path());
        assert!(out.contains("## From AGENTS.md"));
        assert!(out.contains("agents rules"));
        assert!(out.contains("## From .maestro/instructions.md"));
        assert!(out.contains("local notes"));
        assert!(!out.contains("## From MAESTRO.md"), "missing file skipped");
    }

    #[test]
    fn no_instruction_files_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_project_instructions(dir.path()), "");
    }

    #[test]
    fn instructions_respect_budget_with_marker() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(INSTRUCTIONS_BUDGET * 2);
        std::fs::write(dir.path().join("AGENTS.md"), &big).unwrap();
        std::fs::write(dir.path().join("MAESTRO.md"), "should not fit").unwrap();

        let out = load_project_instructions(dir.path());
        assert!(out.chars().count() <= INSTRUCTIONS_BUDGET + 1);
        assert!(out.contains("…[truncated]"));
        assert!(!out.contains("should not fit"));
    }

    #[test]
    fn compaction_keeps_newest() {
        let entries: Vec<TranscriptEntry> = (0..10)
            .map(|i| TranscriptEntry::turn(format!("entry-{i}-{}", "x".repeat(1500))))
            .collect();
        let (kept, omitted) = compact_transcript(&entries, 4_000);
        assert!(!kept.is_empty());
        assert_eq!(kept.len() + omitted, 10);
        // Newest is last and always present.
        assert!(kept.last().unwrap().text.starts_with("entry-9"));
        // Kept entries are the newest contiguous suffix.
        assert!(kept
            .first()
            .unwrap()
            .text
            .starts_with(&format!("entry-{}", 10 - kept.len())));
    }

    #[test]
    fn compaction_floor_and_oversized_newest() {
        // Budget below the floor is raised to 4k.
        let entries = vec![
            TranscriptEntry::tool_result("a".repeat(3_000)),
            TranscriptEntry::turn("b".repeat(900)),
        ];
        let (kept, omitted) = compact_transcript(&entries, 1);
        assert_eq!(kept.len(), 2);
        assert_eq!(omitted, 0);

        // The newest entry survives even when alone over budget.
        let entries = vec![
            TranscriptEntry::turn("old"),
            TranscriptEntry::turn("z".repeat(50_000)),
        ];
        let (kept, omitted) = compact_transcript(&entries, 4_000);
        assert_eq!(kept.len(), 1);
        assert_eq!(omitted, 1);
        assert!(kept[0].text.starts_with('z'));
    }

    #[test]
    fn empty_transcript() {
        let (kept, omitted) = compact_transcript(&[], 4_000);
        assert!(kept.is_empty());
        assert_eq!(omitted, 0);
    }
}
