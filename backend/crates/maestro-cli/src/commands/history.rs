use super::emit_json;
use crate::{
    context::{ensure_daemon, Ctx},
    output::{render_table, short, tint_diff},
};
use maestro_types::Result;

pub async fn history(ctx: &Ctx, save: Option<&str>, diff: Option<&str>) -> Result<i32> {
    let client = ensure_daemon(&ctx.root).await?;
    if let Some(label) = save {
        let v = client.save_version(label).await?;
        if ctx.json {
            emit_json(&v)?;
        } else {
            println!("Saved {} — {}", v.id, v.label);
        }
    } else if let Some(id) = diff {
        let diff = client.version_diff(id).await?;
        if ctx.json {
            emit_json(&serde_json::json!({"unified":diff}))?;
        } else {
            println!("{}", tint_diff(&diff, &ctx.style));
        }
    } else {
        let h = client.code_history().await?;
        if ctx.json {
            emit_json(&h)?;
        } else {
            let rows: Vec<_> = h
                .versions
                .iter()
                .map(|v| {
                    vec![
                        if Some(&v.id) == h.current_id.as_ref() {
                            "*".into()
                        } else {
                            "".into()
                        },
                        short(&v.id),
                        v.kind.clone(),
                        v.label.clone(),
                    ]
                })
                .collect();
            print!(
                "{}",
                render_table(&["", "VERSION", "KIND", "DESCRIPTION"], &rows, &ctx.style)
            );
            println!("maestro history --diff <version> · maestro restore <version> · maestro undo");
        }
    }
    Ok(0)
}

pub async fn restore(ctx: &Ctx, id: Option<&str>, token: Option<&str>) -> Result<i32> {
    let client = ensure_daemon(&ctx.root).await?;
    if let Some(token) = token {
        let result = client.restore_version(id, token).await?;
        if ctx.json {
            emit_json(&result)?;
        } else {
            println!(
                "Restored {}. Recovery version: {}. Start a new request to continue.",
                result.version.id, result.recovery_id
            );
        }
    } else {
        let preview = client.preview_restore(id).await?;
        if ctx.json {
            emit_json(&preview)?;
        } else {
            println!(
                "Restore preview: {}\n{}",
                preview.target.label,
                tint_diff(&preview.unified, &ctx.style)
            );
            if !preview.conflicts.is_empty() {
                println!(
                    "Conflicts (files unchanged): {}",
                    preview.conflicts.join(", ")
                );
            } else {
                println!(
                    "{} unfinished missions will stop. Existing conversation history is retained.",
                    preview.affected_missions.len()
                );
                let command = id
                    .map(|id| format!("restore {id}"))
                    .unwrap_or_else(|| "undo".into());
                println!(
                    "Apply this preview: maestro {command} --apply {}",
                    preview.token
                );
            }
        }
    }
    Ok(0)
}
