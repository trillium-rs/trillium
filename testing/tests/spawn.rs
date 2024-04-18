use std::time::Duration;
use test_harness::test;
use trillium_testing::{harness, runtime, RuntimeTrait, TestResult};

#[test(harness)]
async fn spawn_works() -> TestResult {
    let runtime = runtime();
    let rt = runtime.clone();
    let fut = rt.spawn(async move {
        let runtime = runtime;
        runtime.delay(Duration::from_secs(1)).await;
        1
    });
    assert_eq!(1, fut.await.unwrap());
    Ok(())
}

#[test(harness)]
async fn dropped_spawn_task_still_finishes() -> TestResult {
    let (tx, rx) = async_channel::unbounded();

    runtime().spawn(async move {
        runtime().delay(Duration::from_secs(1)).await;
        tx.send(1).await.unwrap();
        2
    });

    assert_eq!(1, rx.recv().await.unwrap());
    Ok(())
}

#[test(harness)]
async fn panic_in_spawn_returns_none() -> TestResult {
    let runtime = runtime();
    let rt = runtime.clone();
    let fut = rt.spawn(async move {
        let runtime = runtime;
        runtime.delay(Duration::from_secs(1)).await;
        panic!();
    });

    assert!(fut.await.is_none());
    Ok(())
}
