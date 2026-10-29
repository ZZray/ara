//! Controlled ordinary-process lifecycle probe. No external model or product.
#[path = "../tests/common/mod.rs"]
mod common;
use anyhow::{ensure, Result};
use ara_agent_lite_harness::{start, TaskStop};
use common::*;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(args.len() == 2, "usage: lifecycle_probe owner|contender|normal ABSOLUTE_ROOT");
    let root = PathBuf::from(&args[1]);
    ensure!(root.is_absolute() && root.is_dir(), "existing absolute root required");
    let provider = Provider::new(vec![write_answer("correct"), text("Submitted.")]);
    let fault = if args[0] == "owner" { Fault::SlowReview } else { Fault::None };
    let host = Arc::new(Host::new(&root, &provider, fault));
    let spec = host.spec(if args[0] == "owner" {
        "owner-task"
    } else if args[0] == "normal" {
        "normal-task"
    } else {
        "contender-task"
    });
    if args[0] == "contender" {
        let rejected = start(spec, host.request("contender-run"), host).is_err();
        let calls = provider.requests.lock().unwrap().len();
        ensure!(rejected && calls == 0, "contender acquired or called provider");
        println!("{}", json!({"pid":std::process::id(),"admission":"rejected","provider_requests":calls}));
        return Ok(());
    }
    let handle = start(spec, host.request("original-run"), host.clone())?;
    if args[0] == "owner" {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !host.review_started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let ready = root.join("review-ready.json");
        std::fs::write(
            &ready,
            serde_json::to_vec(
                &json!({"pid":std::process::id(),"review":"active","provider_requests":provider.requests.lock().unwrap().len()}),
            )?,
        )?;
        std::fs::OpenOptions::new().write(true).open(ready)?.sync_all()?;
        tokio::time::timeout(Duration::from_secs(20), async {
            while !root.join("cancel-now").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        handle.cancel();
    }
    let outcome = handle.wait().await;
    if args[0] == "owner" {
        ensure!(
            outcome.stop == TaskStop::Cancelled && host.review_joined.load(Ordering::SeqCst),
            "cancellation did not join"
        );
    } else {
        ensure!(outcome.stop == TaskStop::AwaitingHostAcceptance, "normal run did not verify: {outcome:?}");
    }
    println!(
        "{}",
        json!({"pid":std::process::id(),"outcome":outcome.as_ref(),"review_joined":host.review_joined.load(Ordering::SeqCst),"provider_requests":provider.requests.lock().unwrap().len()})
    );
    Ok(())
}
