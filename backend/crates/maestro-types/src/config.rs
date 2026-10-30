use crate::domain::Role;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `maestro.toml` at the project root.
///
/// Keys are `camelCase` on disk (serde `rename_all`); unknown keys are
/// rejected loudly (`deny_unknown_fields`) so documented-but-misspelled or
/// snake_case keys never get silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct MaestroConfig {
    pub version: u32,
    pub providers: BTreeMap<String, ProviderConfig>,
    /// role name → assignment. Missing roles fall back to `default_model`.
    pub roles: BTreeMap<String, RoleAssignment>,
    pub runtime: RuntimeConfig,
    pub safety: SafetyConfig,
    pub integration: IntegrationConfig,
    /// Pre-approved project checks, runnable by index via `run_check`.
    pub checks: Vec<CheckConfig>,
}

impl Default for MaestroConfig {
    fn default() -> Self {
        #[cfg(feature = "test-support")]
        {
            Self::test_config()
        }
        #[cfg(not(feature = "test-support"))]
        {
            let provider = ProviderConfig {
                kind: ProviderKind::OpenaiResponses,
                base_url: None,
                api_key_env: Some("OPENAI_API_KEY".into()),
                headers: BTreeMap::new(),
                requests_per_minute: None,
                tokens_per_minute: None,
                deployment: None,
                reasoning_effort: None,
            };
            Self {
                version: 1,
                providers: BTreeMap::from([("gpt".into(), provider)]),
                roles: Role::ALL
                    .into_iter()
                    .map(|role| {
                        (
                            role.as_str().into(),
                            RoleAssignment {
                                model: ModelRef {
                                    provider: "gpt".into(),
                                    model: "gpt-6.1-sol".into(),
                                },
                                fallbacks: vec![],
                                max_context_chars: default_context_chars(),
                                max_output_tokens: Some(8192),
                            },
                        )
                    })
                    .collect(),
                runtime: RuntimeConfig::default(),
                safety: SafetyConfig::default(),
                integration: IntegrationConfig::default(),
                checks: vec![],
            }
        }
    }
}

#[cfg(feature = "test-support")]
impl MaestroConfig {
    /// Deterministic fixtures are available only in explicitly enabled test builds.
    pub fn test_config() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            "mock".to_string(),
            ProviderConfig {
                kind: ProviderKind::Mock,
                base_url: None,
                api_key_env: None,
                headers: BTreeMap::new(),
                requests_per_minute: None,
                tokens_per_minute: None,
                deployment: None,
                reasoning_effort: None,
            },
        );
        let mut roles = BTreeMap::new();
        for role in Role::ALL {
            roles.insert(
                role.as_str().to_string(),
                RoleAssignment {
                    model: ModelRef {
                        provider: "mock".into(),
                        model: role.as_str().into(),
                    },
                    fallbacks: vec![],
                    max_context_chars: default_context_chars(),
                    max_output_tokens: None,
                },
            );
        }
        MaestroConfig {
            version: 1,
            providers,
            roles,
            runtime: RuntimeConfig::default(),
            safety: SafetyConfig::default(),
            integration: IntegrationConfig::default(),
            checks: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenaiCompatible,
    OpenaiResponses,
    #[cfg(feature = "test-support")]
    Mock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Env var holding the API key. Stored credentials are injected here if unset.
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Token-bucket rate limit; None = unlimited.
    #[serde(default)]
    pub requests_per_minute: Option<u32>,
    /// Conservative rolling-minute input estimate + reserved output budget.
    /// Configure one provider ID per deployment to keep quotas independent.
    #[serde(default)]
    pub tokens_per_minute: Option<u32>,
    /// Pin a deployment instead of listing the provider's global model catalog.
    /// Use a separate provider ID per deployment when quotas differ.
    #[serde(default)]
    pub deployment: Option<String>,
    /// Explicit Responses reasoning effort. Omitted uses the model default.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

impl ProviderKind {
    pub fn is_mock(self) -> bool {
        #[cfg(feature = "test-support")]
        {
            self == Self::Mock
        }
        #[cfg(not(feature = "test-support"))]
        {
            false
        }
    }
}

/// "provider/model" reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    /// Parse "provider/model" (model may itself contain '/').
    pub fn parse(s: &str) -> Option<ModelRef> {
        let (provider, model) = s.split_once('/')?;
        if provider.is_empty() || model.is_empty() {
            return None;
        }
        Some(ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
        })
    }
}

impl std::fmt::Display for ModelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleAssignment {
    pub model: ModelRef,
    #[serde(default)]
    pub fallbacks: Vec<ModelRef>,
    #[serde(default = "default_context_chars")]
    pub max_context_chars: usize,
    /// Per-turn output-token cap passed to the provider. `None` uses the
    /// transport default; raise for roles that write whole files.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

pub fn default_context_chars() -> usize {
    24_000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntime {
    Native,
    Langgraph,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Four PS-001 agent contracts; legacy role keys remain readable.
    pub guild_roles: bool,
    /// Native is retained for compatibility; LangGraph runs the installed IPC worker.
    pub agent_runtime: AgentRuntime,
    pub langgraph_command: Vec<String>,
    pub max_concurrent_tasks: usize,
    pub max_attempts_per_task: u32,
    pub max_turns_per_attempt: u32,
    pub max_total_tokens: i64,
    /// Hard mission cost ceiling in micro-USD. 0 = unlimited.
    pub max_cost_micro_usd: i64,
    pub lease_ttl_ms: i64,
    pub provider_retries: u32,
    /// Halt after planning until the plan is explicitly approved.
    pub plan_approval: bool,
    /// Fail an attempt after this many identical consecutive tool calls.
    pub loop_detection_window: u32,
    pub max_delegation_depth: u32,
    pub max_tasks_per_mission: usize,
    /// Provider request timeout.
    pub request_timeout_ms: i64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            guild_roles: false,
            agent_runtime: AgentRuntime::Native,
            langgraph_command: vec![],
            max_concurrent_tasks: 2,
            max_attempts_per_task: 2,
            max_turns_per_attempt: 24,
            max_total_tokens: 500_000,
            max_cost_micro_usd: 0,
            lease_ttl_ms: 90_000,
            provider_retries: 2,
            plan_approval: false,
            loop_detection_window: 3,
            max_delegation_depth: 2,
            max_tasks_per_mission: 50,
            request_timeout_ms: 180_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementMode {
    /// Fail closed when the mechanism is unavailable.
    Required,
    /// Fall back to the unisolated path with a warning event.
    Preferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Read,
    Edit,
    Check,
    Delegate,
    Network,
    ExternalDirectory,
}

impl PolicyAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyAction::Read => "read",
            PolicyAction::Edit => "edit",
            PolicyAction::Check => "check",
            PolicyAction::Delegate => "delegate",
            PolicyAction::Network => "network",
            PolicyAction::ExternalDirectory => "external_directory",
        }
    }
}

/// One ordered policy rule; the LAST matching rule wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    pub action: PolicyAction,
    /// Glob over the resource (path, url, role…). "*" and "**" supported.
    pub resource: String,
    pub effect: PolicyEffect,
    /// Restrict the rule to a role; None = all roles.
    #[serde(default)]
    pub role: Option<Role>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SafetyConfig {
    /// "strict" | "balanced" | "permissive" — expands to a base rule set.
    pub profile: String,
    /// Extra ordered rules appended after the profile's rules.
    pub rules: Vec<PolicyRule>,
    pub sandbox: EnforcementMode,
    pub sandbox_network: bool,
    pub worktrees: EnforcementMode,
    pub max_check_output_chars: usize,
    pub check_timeout_ms: i64,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        SafetyConfig {
            profile: "balanced".into(),
            rules: vec![],
            sandbox: EnforcementMode::Required,
            sandbox_network: false,
            worktrees: EnforcementMode::Required,
            max_check_output_chars: 20_000,
            check_timeout_ms: 300_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStrategy {
    MissionBranch,
    Direct,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct IntegrationConfig {
    pub strategy: IntegrationStrategy,
    /// Require an explicit approval before the final fast-forward.
    pub final_approval: bool,
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        IntegrationConfig {
            strategy: IntegrationStrategy::MissionBranch,
            final_approval: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckConfig {
    pub name: String,
    /// argv, executed without a shell.
    pub command: Vec<String>,
}
