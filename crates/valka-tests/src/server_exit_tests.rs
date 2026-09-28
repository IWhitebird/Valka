use std::time::Duration;

use tokio::sync::watch;
use valka_server::server::{ExitReason, wait_for_exit};
use valka_wal::fault::faulty_memory_store;

use crate::integration::helpers::{TestNode, task_req};

#[tokio::test]
async fn signal_is_a_clean_exit() {
    let (_ptx, prx) = watch::channel(None);
    let (_stx, srx) = watch::channel(false);
    let reason = wait_for_exit(async {}, prx, srx).await;
    assert_eq!(reason, ExitReason::Signal);
    assert!(reason.is_clean());
}

#[tokio::test]
async fn poisoned_writer_is_an_unclean_exit() {
    let (ptx, prx) = watch::channel(None);
    let (_stx, srx) = watch::channel(false);
    ptx.send_replace(Some("bucket down".into()));
    let reason = wait_for_exit(std::future::pending(), prx, srx).await;
    assert_eq!(reason, ExitReason::Poisoned("bucket down".into()));
    assert!(!reason.is_clean());
}

#[tokio::test]
async fn failed_server_task_is_an_unclean_exit() {
    let (_ptx, prx) = watch::channel(None);
    let (stx, srx) = watch::channel(false);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        stx.send_replace(true);
    });
    let reason = wait_for_exit(std::future::pending(), prx, srx).await;
    assert_eq!(reason, ExitReason::Failed);
}

#[tokio::test]
async fn dropped_channels_do_not_trigger_an_exit() {
    let (ptx, prx) = watch::channel(None);
    let (stx, srx) = watch::channel(false);
    drop(ptx);
    drop(stx);
    let reason = tokio::time::timeout(
        Duration::from_millis(50),
        wait_for_exit(std::future::pending(), prx, srx),
    )
    .await;
    assert!(reason.is_err(), "a closed channel is not a stop condition");
}

#[tokio::test(start_paused = true)]
async fn node_whose_bucket_fails_decides_to_exit() {
    let (store, faults) = faulty_memory_store(11);
    let node = TestNode::on_store(store, "exit-node").await;
    let (_stx, srx) = watch::channel(false);
    let exit = tokio::spawn(wait_for_exit(
        std::future::pending(),
        node.engine.poison_watch(),
        srx,
    ));
    faults.set_puts_down(true);
    assert!(node.engine.create_task(task_req("q", "t")).await.is_err());
    let reason = exit.await.unwrap();
    assert!(matches!(reason, ExitReason::Poisoned(_)), "got {reason:?}");
}
