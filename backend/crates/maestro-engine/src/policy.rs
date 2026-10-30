//! Policy evaluation: ordered glob rules, last match wins, with mandatory
//! hard denials that no rule set can override.

use globset::{Glob, GlobMatcher};
use maestro_types::config::{PolicyAction, PolicyEffect, PolicyRule};
use maestro_types::domain::Role;
use maestro_types::{MaestroError, Result};

struct CompiledRule {
    action: PolicyAction,
    matcher: GlobMatcher,
    effect: PolicyEffect,
    role: Option<Role>,
}

/// A per-role policy evaluator over an ordered rule list
/// (profile rules first, config extras appended).
pub struct PolicyEngine {
    role: Role,
    rules: Vec<CompiledRule>,
}

impl PolicyEngine {
    /// Compile the rule list for one role. Invalid glob patterns are a
    /// [`MaestroError::Config`] error.
    pub fn new(role: Role, rules: &[PolicyRule]) -> Result<Self> {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let matcher = Glob::new(&rule.resource)
                .map_err(|e| {
                    MaestroError::Config(format!("invalid policy glob '{}': {e}", rule.resource))
                })?
                .compile_matcher();
            compiled.push(CompiledRule {
                action: rule.action,
                matcher,
                effect: rule.effect,
                role: rule.role,
            });
        }
        Ok(PolicyEngine {
            role,
            rules: compiled,
        })
    }

    /// The role this engine evaluates for.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Evaluate `action` on `resource` (a path, url, or role name).
    ///
    /// Hard, non-overridable denials are checked before any rule:
    /// 1. `Edit` for read-only roles (reviewer, security).
    /// 2. `ExternalDirectory`, always.
    /// 3. `Read`/`Edit` of any path whose file name is `.env` or starts with
    ///    `.env.` — except `.env.example`.
    ///
    /// Otherwise the LAST matching rule wins; when nothing matches the
    /// defaults are: read/edit/check/delegate allow, network and
    /// external_directory deny.
    pub fn evaluate(&self, action: PolicyAction, resource: &str) -> PolicyEffect {
        if action == PolicyAction::Edit && self.role.is_read_only() {
            return PolicyEffect::Deny;
        }
        if action == PolicyAction::ExternalDirectory {
            return PolicyEffect::Deny;
        }
        if matches!(action, PolicyAction::Read | PolicyAction::Edit) && is_secret_file(resource) {
            return PolicyEffect::Deny;
        }

        let mut effect: Option<PolicyEffect> = None;
        for rule in &self.rules {
            if rule.action != action {
                continue;
            }
            if let Some(role) = rule.role {
                if role != self.role {
                    continue;
                }
            }
            if rule.matcher.is_match(resource) {
                effect = Some(rule.effect); // last match wins
            }
        }
        effect.unwrap_or(default_effect(action))
    }
}

/// Built-in default when no rule matches.
fn default_effect(action: PolicyAction) -> PolicyEffect {
    match action {
        PolicyAction::Read | PolicyAction::Edit | PolicyAction::Check | PolicyAction::Delegate => {
            PolicyEffect::Allow
        }
        PolicyAction::Network | PolicyAction::ExternalDirectory => PolicyEffect::Deny,
    }
}

/// True for paths whose file name is `.env` or `.env.*` — except
/// `.env.example`, which stays readable.
pub fn is_secret_file(resource: &str) -> bool {
    let name = resource.rsplit('/').next().unwrap_or(resource);
    name == ".env" || (name.starts_with(".env.") && name != ".env.example")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        action: PolicyAction,
        resource: &str,
        effect: PolicyEffect,
        role: Option<Role>,
    ) -> PolicyRule {
        PolicyRule {
            action,
            resource: resource.to_string(),
            effect,
            role,
        }
    }

    fn allow_everything() -> Vec<PolicyRule> {
        [
            PolicyAction::Read,
            PolicyAction::Edit,
            PolicyAction::Check,
            PolicyAction::Delegate,
            PolicyAction::Network,
            PolicyAction::ExternalDirectory,
        ]
        .into_iter()
        .map(|a| rule(a, "**", PolicyEffect::Allow, None))
        .collect()
    }

    #[test]
    fn last_match_wins() {
        let rules = vec![
            rule(PolicyAction::Edit, "**", PolicyEffect::Allow, None),
            rule(PolicyAction::Edit, "src/**", PolicyEffect::Deny, None),
            rule(PolicyAction::Edit, "src/lib.rs", PolicyEffect::Ask, None),
        ];
        let engine = PolicyEngine::new(Role::Coder, &rules).unwrap();
        assert_eq!(
            engine.evaluate(PolicyAction::Edit, "README.md"),
            PolicyEffect::Allow
        );
        assert_eq!(
            engine.evaluate(PolicyAction::Edit, "src/main.rs"),
            PolicyEffect::Deny
        );
        assert_eq!(
            engine.evaluate(PolicyAction::Edit, "src/lib.rs"),
            PolicyEffect::Ask
        );
    }

    #[test]
    fn role_scoped_rules_only_apply_to_that_role() {
        let rules = vec![rule(
            PolicyAction::Delegate,
            "**",
            PolicyEffect::Deny,
            Some(Role::Designer),
        )];
        let designer = PolicyEngine::new(Role::Designer, &rules).unwrap();
        let coder = PolicyEngine::new(Role::Coder, &rules).unwrap();
        assert_eq!(
            designer.evaluate(PolicyAction::Delegate, "coder"),
            PolicyEffect::Deny
        );
        assert_eq!(
            coder.evaluate(PolicyAction::Delegate, "coder"),
            PolicyEffect::Allow
        );
    }

    #[test]
    fn hard_denials_survive_allow_everything() {
        for role in [Role::Reviewer, Role::Security] {
            let engine = PolicyEngine::new(role, &allow_everything()).unwrap();
            assert_eq!(
                engine.evaluate(PolicyAction::Edit, "src/main.rs"),
                PolicyEffect::Deny,
                "{role} must never edit"
            );
        }
        let coder = PolicyEngine::new(Role::Coder, &allow_everything()).unwrap();
        assert_eq!(
            coder.evaluate(PolicyAction::ExternalDirectory, "/etc"),
            PolicyEffect::Deny
        );
        assert_eq!(
            coder.evaluate(PolicyAction::Read, ".env"),
            PolicyEffect::Deny
        );
        assert_eq!(
            coder.evaluate(PolicyAction::Edit, "config/.env.production"),
            PolicyEffect::Deny
        );
    }

    #[test]
    fn env_example_stays_readable() {
        let engine = PolicyEngine::new(Role::Coder, &[]).unwrap();
        assert_eq!(
            engine.evaluate(PolicyAction::Read, ".env.example"),
            PolicyEffect::Allow
        );
        assert_eq!(
            engine.evaluate(PolicyAction::Read, "app/.env.example"),
            PolicyEffect::Allow
        );
        assert_eq!(
            engine.evaluate(PolicyAction::Read, "app/.env.local"),
            PolicyEffect::Deny
        );
        // Not a dotfile match: envelope.rs is fine.
        assert_eq!(
            engine.evaluate(PolicyAction::Read, "src/envelope.rs"),
            PolicyEffect::Allow
        );
    }

    #[test]
    fn defaults_apply_when_nothing_matches() {
        let engine = PolicyEngine::new(Role::Coder, &[]).unwrap();
        assert_eq!(
            engine.evaluate(PolicyAction::Read, "src/a.rs"),
            PolicyEffect::Allow
        );
        assert_eq!(
            engine.evaluate(PolicyAction::Edit, "src/a.rs"),
            PolicyEffect::Allow
        );
        assert_eq!(
            engine.evaluate(PolicyAction::Check, "build"),
            PolicyEffect::Allow
        );
        assert_eq!(
            engine.evaluate(PolicyAction::Network, "https://example.com"),
            PolicyEffect::Deny
        );
    }

    #[test]
    fn invalid_glob_is_config_error() {
        let rules = vec![rule(PolicyAction::Read, "src/{", PolicyEffect::Allow, None)];
        assert!(matches!(
            PolicyEngine::new(Role::Coder, &rules),
            Err(MaestroError::Config(_))
        ));
    }
}
