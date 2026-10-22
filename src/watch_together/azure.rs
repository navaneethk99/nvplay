//! Optional wake-up for the owner's explicitly configured Azure deployment.
use super::{Emitter, Event};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct Deployment {
    url: String,
    resource_group: String,
    vm: String,
    subscription: String,
}
fn deployment(endpoint: &str) -> Option<Deployment> {
    let bytes = std::fs::read(
        crate::settings::directory()
            .ok()?
            .join("watch-deployment.json"),
    )
    .ok()?;
    let saved: Deployment = serde_json::from_slice(&bytes).ok()?;
    // A custom service address must never start an unrelated Azure resource.
    if super::signaling_url(&saved.url).ok()?.as_str() != endpoint {
        return None;
    }
    if [&saved.resource_group, &saved.vm].iter().any(|s| {
        s.is_empty()
            || s.len() > 90
            || !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.()".contains(c))
    }) {
        return None;
    }
    if saved.subscription.len() != 36
        || !saved
            .subscription
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return None;
    }
    Some(saved)
}
async fn healthy(origin: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    ehttp::fetch(
        ehttp::Request::get(format!("{}/health", origin.trim_end_matches('/')))
            .with_timeout(Some(Duration::from_secs(3))),
        move |result| {
            let _ = tx.send(result.is_ok_and(|r| r.status == 200));
        },
    );
    tokio::time::timeout(Duration::from_secs(3), rx)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
}
pub(super) async fn ensure_running(endpoint: &str, emit: &Emitter) -> Result<(), String> {
    let Some(saved) = deployment(endpoint) else {
        return Ok(());
    };
    if healthy(&saved.url).await {
        return Ok(());
    }
    emit.send(Event::Status(
        "Waking the watch service… This can take a couple of minutes.".into(),
    ));
    let az = crate::streaming::find_tool("NVPLAY_AZURE_CLI", "az").ok_or(
        "The watch service is asleep. Install Azure CLI and sign in with az login to wake it.",
    )?;
    let mut command = tokio::process::Command::from(crate::streaming::tool_command(&az));
    command
        .kill_on_drop(true)
        .args([
            "vm",
            "start",
            "--subscription",
            &saved.subscription,
            "--resource-group",
            &saved.resource_group,
            "--name",
            &saved.vm,
            "--only-show-errors",
            "--output",
            "none",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let result = tokio::time::timeout(Duration::from_secs(240), command.status())
        .await
        .map_err(|_| "Starting the watch service timed out. Check Azure and try again.")?
        .map_err(|_| "Could not run Azure CLI to wake the watch service.")?;
    if !result.success() {
        return Err("Could not wake the watch service. Run az login in Terminal, check your Azure subscription, and try again.".into());
    }
    for _ in 0..45 {
        if healthy(&saved.url).await {
            emit.send(Event::Status("Creating your viewing link…".into()));
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err("Azure started, but the watch service is not ready. Try again shortly.".into())
}
