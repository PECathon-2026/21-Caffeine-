//! Slash commands + the pure fuzzy subsequence scorer used by the palette
//! and the model picker.

/// All slash commands the composer and palette understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommand {
    Ask,
    Attach,
    Preview,
    Run,
    Plan,
    Approve,
    Reject,
    Steer,
    Status,
    Models,
    Model,
    Roles,
    Connect,
    Settings,
    Sessions,
    History,
    Undo,
    Restore,
    New,
    Help,
    Quit,
}

impl SlashCommand {
    pub const ALL: [SlashCommand; 21] = [
        SlashCommand::Ask,
        SlashCommand::Attach,
        SlashCommand::Preview,
        SlashCommand::Run,
        SlashCommand::Plan,
        SlashCommand::Approve,
        SlashCommand::Reject,
        SlashCommand::Steer,
        SlashCommand::Status,
        SlashCommand::Models,
        SlashCommand::Model,
        SlashCommand::Roles,
        SlashCommand::Connect,
        SlashCommand::Settings,
        SlashCommand::Sessions,
        SlashCommand::History,
        SlashCommand::Undo,
        SlashCommand::Restore,
        SlashCommand::New,
        SlashCommand::Help,
        SlashCommand::Quit,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SlashCommand::Ask => "ask",
            SlashCommand::Attach => "attach",
            SlashCommand::Preview => "preview",
            SlashCommand::Run => "run",
            SlashCommand::Plan => "plan",
            SlashCommand::Approve => "approve",
            SlashCommand::Reject => "reject",
            SlashCommand::Steer => "steer",
            SlashCommand::Status => "status",
            SlashCommand::Models => "models",
            SlashCommand::Model => "model",
            SlashCommand::Roles => "roles",
            SlashCommand::Connect => "connect",
            SlashCommand::Settings => "settings",
            SlashCommand::Sessions => "sessions",
            SlashCommand::History => "history",
            SlashCommand::Undo => "undo",
            SlashCommand::Restore => "restore",
            SlashCommand::New => "new",
            SlashCommand::Help => "help",
            SlashCommand::Quit => "quit",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            SlashCommand::Ask => "ask the architect a question",
            SlashCommand::Attach => "attach an image to the next message",
            SlashCommand::Preview => "show local preview URLs and logs, or stop previews",
            SlashCommand::Run => "submit an objective as a mission",
            SlashCommand::Plan => "show / approve / reject the pending plan",
            SlashCommand::Approve => "approve a pending approval",
            SlashCommand::Reject => "reject a pending approval",
            SlashCommand::Steer => "steer the active mission",
            SlashCommand::Status => "show control-plane status",
            SlashCommand::Models => "browse discovered models",
            SlashCommand::Model => "assign a model to a role (picker or direct)",
            SlashCommand::Roles => "show which model each role runs",
            SlashCommand::Connect => "store an API key for a provider",
            SlashCommand::Settings => "show config and theme file locations",
            SlashCommand::Sessions => "list and resume past chat sessions",
            SlashCommand::History => "browse saved code versions and diffs",
            SlashCommand::Undo => "preview undoing the latest request",
            SlashCommand::Restore => "preview restoring a saved code version",
            SlashCommand::New => "start a new chat session",
            SlashCommand::Help => "show keybindings and commands",
            SlashCommand::Quit => "exit maestro",
        }
    }

    /// Placeholder for the arguments the command expects, shown as a ghost
    /// hint after completion. `None` = command takes no arguments.
    pub fn arg_hint(self) -> Option<&'static str> {
        match self {
            SlashCommand::Ask => Some("<question>"),
            SlashCommand::Attach => Some("<image-path> | clear"),
            SlashCommand::Preview => Some("[stop]"),
            SlashCommand::Run => Some("<objective>"),
            SlashCommand::Plan => Some("[approve|reject]"),
            SlashCommand::Approve => Some("[approval-id]"),
            SlashCommand::Reject => Some("[approval-id] [reason]"),
            SlashCommand::Steer => Some("<directive>"),
            SlashCommand::Model => Some("[role] [provider/model] — bare opens the picker"),
            SlashCommand::Connect => Some("<provider-id>"),
            SlashCommand::Restore => Some("<version>"),
            SlashCommand::Status
            | SlashCommand::Roles
            | SlashCommand::Models
            | SlashCommand::Settings
            | SlashCommand::Sessions
            | SlashCommand::History
            | SlashCommand::Undo
            | SlashCommand::New
            | SlashCommand::Help
            | SlashCommand::Quit => None,
        }
    }

    pub fn parse(name: &str) -> Option<SlashCommand> {
        SlashCommand::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// The bare `/token` being typed, when the composer is in the suggesting
/// shape (single line, no whitespace after the slash yet).
pub fn slash_token(input: &str) -> Option<&str> {
    let token = input.strip_prefix('/')?;
    if token.contains(char::is_whitespace) || input.contains('\n') {
        return None;
    }
    Some(token)
}

/// Live slash-command suggestions for the composer.
///
/// Active while the input is a single-line `/token` with no whitespace yet:
/// the token fuzzy-filters the command list. Returns best-first; empty when
/// the input is not in the suggesting shape (or nothing matches — the popup
/// stays open on zero matches so keystrokes never leak meaning).
pub fn slash_suggestions(input: &str) -> Vec<SlashCommand> {
    let Some(token) = slash_token(input) else {
        return Vec::new();
    };
    if token.is_empty() {
        return SlashCommand::ALL.to_vec();
    }
    let ranked = fuzzy_rank(token, &SlashCommand::ALL, |c| c.name());
    ranked.into_iter().map(|i| SlashCommand::ALL[i]).collect()
}

/// The arg ghost-hint for a completed command: input is `/name` or
/// `/name <partial args>` and the command expects arguments the user has
/// not finished typing. Returns (command, hint) while the hint is useful.
pub fn arg_hint_for(input: &str) -> Option<(SlashCommand, &'static str)> {
    let (cmd, args) = parse_slash(input.trim_end_matches('\n'))?;
    let hint = cmd.arg_hint()?;
    if args.is_empty() {
        Some((cmd, hint))
    } else {
        None
    }
}

/// Split composer input `/cmd rest of args` → (command, trimmed args).
pub fn parse_slash(input: &str) -> Option<(SlashCommand, &str)> {
    let rest = input.strip_prefix('/')?;
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (rest.trim_end(), ""),
    };
    Some((SlashCommand::parse(name)?, args))
}

/// Pure fuzzy subsequence scorer. Returns `None` when `query` is not a
/// subsequence of `candidate` (case-insensitive); higher scores are better.
/// Bonuses: consecutive matches, matches at word starts, early first match.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let cand: Vec<char> = candidate.chars().collect();
    let mut score: i64 = 0;
    let mut ci = 0usize;
    let mut last_hit: Option<usize> = None;
    for qc in query.chars() {
        let qc = qc.to_ascii_lowercase();
        let mut found = None;
        while ci < cand.len() {
            if cand[ci].to_ascii_lowercase() == qc {
                found = Some(ci);
                break;
            }
            ci += 1;
        }
        let hit = found?;
        score += 1;
        if last_hit == Some(hit.wrapping_sub(1)) {
            score += 4; // consecutive
        }
        let word_start = hit == 0 || matches!(cand[hit - 1], ' ' | '-' | '_' | '/' | '.' | ':');
        if word_start {
            score += 3;
        }
        last_hit = Some(hit);
        ci = hit + 1;
    }
    // Prefer earlier first matches and shorter candidates.
    score -= (candidate.len() as i64) / 8;
    Some(score)
}

/// Rank `items` by fuzzy score against `query`, best first, dropping misses.
/// Returns indices into `items`.
pub fn fuzzy_rank<T, F: Fn(&T) -> &str>(query: &str, items: &[T], key: F) -> Vec<usize> {
    let mut scored: Vec<(i64, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy_score(query, key(it)).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slash_commands() {
        assert_eq!(
            parse_slash("/run build the thing"),
            Some((SlashCommand::Run, "build the thing"))
        );
        assert_eq!(parse_slash("/quit"), Some((SlashCommand::Quit, "")));
        assert_eq!(parse_slash("/nope args"), None);
        assert_eq!(parse_slash("run"), None);
    }

    #[test]
    fn fuzzy_requires_subsequence() {
        assert!(fuzzy_score("mdl", "model").is_some());
        assert!(fuzzy_score("mdl", "diff").is_none());
        assert!(fuzzy_score("xyz", "model").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn fuzzy_prefers_word_starts_and_runs() {
        let exact = fuzzy_score("model", "model").unwrap();
        let scattered = fuzzy_score("model", "mo-d-e-l-x-x-x").unwrap();
        assert!(exact > scattered);
        let word_start = fuzzy_score("re", "reject").unwrap();
        let midword = fuzzy_score("re", "core").unwrap();
        assert!(word_start > midword);
    }

    #[test]
    fn fuzzy_rank_orders_best_first() {
        let items = vec!["approve", "reject", "retry", "run"];
        let ranked = fuzzy_rank("re", &items, |s| s);
        assert_eq!(items[ranked[0]], "reject");
        assert!(ranked.iter().all(|&i| i != 0 || items[i].contains('r')));
        // "approve" contains subsequence r,e too — but ranks below word-start hits.
        assert!(ranked.contains(&0));
        assert_ne!(ranked[0], 0);
    }

    #[test]
    fn fuzzy_is_case_insensitive() {
        assert!(fuzzy_score("MDL", "model").is_some());
        assert!(fuzzy_score("mdl", "MODEL").is_some());
    }

    #[test]
    fn suggestions_follow_the_token() {
        assert_eq!(slash_suggestions("/").len(), SlashCommand::ALL.len());
        let st = slash_suggestions("/st");
        // Both prefix-match; the tie breaks by registry order (steer first).
        assert_eq!(st.first().copied(), Some(SlashCommand::Steer));
        assert!(st.contains(&SlashCommand::Status));
        assert!(!st.contains(&SlashCommand::Quit));
        // Not suggesting: no slash, args started, or multiline.
        assert!(slash_suggestions("status").is_empty());
        assert!(slash_suggestions("/steer now").is_empty());
        assert!(slash_suggestions("/st\nx").is_empty());
        assert!(slash_suggestions("/zzz").is_empty());
    }

    #[test]
    fn arg_hint_appears_until_args_start() {
        assert_eq!(
            arg_hint_for("/steer "),
            Some((SlashCommand::Steer, "<directive>"))
        );
        assert_eq!(arg_hint_for("/steer do the thing"), None);
        assert_eq!(arg_hint_for("/help "), None); // takes no args
        assert_eq!(arg_hint_for("plain text"), None);
    }
}
