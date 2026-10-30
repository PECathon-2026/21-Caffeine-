//! Real local preview/browser smoke check; run with an isolated Next.js project path.
use maestro_engine::{
    browser, compile_scopes, deny_ask_decider,
    tools::{ToolCtx, ToolExec, UtilityTool},
    PolicyEngine, SandboxSpec,
};
use maestro_types::{config::EnforcementMode, domain::Role};
use serde_json::json;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("fixture directory"));
    let mut sandbox = SandboxSpec::new(&root);
    sandbox.mode = EnforcementMode::Required;
    let ctx = ToolCtx {
        root: root.clone(),
        workspace: root.clone(),
        base_ref: None,
        role: Role::Coder,
        scopes: compile_scopes(&["**".into()])?,
        policy: PolicyEngine::new(Role::Coder, &[])?,
        checks: vec![],
        sandbox,
        max_read_chars: 20000,
        decider: deny_ask_decider(),
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let result = UtilityTool("preview_start")
        .execute(&ctx, &json!({"port":port}))
        .await?;
    println!("start: {result:?}");
    assert!(browser::active(&root));
    let capture = UtilityTool("browser_inspect")
        .execute(
            &ctx,
            &json!({"url":format!("http://127.0.0.1:{port}"),"width":800,"height":600}),
        )
        .await;
    println!("logs: {}", browser::status(&root, false));
    let stop = UtilityTool("preview_stop")
        .execute(&ctx, &json!({}))
        .await?;
    println!("stop: {stop:?}");
    assert!(!browser::active(&root));
    let maestro_engine::ToolOutcome::Result(v) = capture? else {
        panic!("expected result")
    };
    assert_eq!(v["status"], 200, "{v}");
    assert!(
        v["text"]
            .as_str()
            .unwrap()
            .contains("Maestro preview verified"),
        "{v}"
    );
    assert!(v["reference"].as_str().unwrap().starts_with("[[image:"));
    println!("browser: {v}");
    Ok(())
}
