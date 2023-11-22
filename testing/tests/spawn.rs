use test_harness::test;

#[test(harness = trillium_testing::harness)]
async fn spawn_works() -> trillium_testing::TestResult {
    let fut = trillium_testing::spawn(async {
        std::thread::sleep(std::time::Duration::from_secs(1));
        1
    });
    assert_eq!(1, fut.await.unwrap());
    Ok(())
}

#[test(harness = trillium_testing::harness)]
async fn dropped_spawn_task_still_finishes() -> trillium_testing::TestResult {
    let (tx, rx) = async_channel::unbounded();
    drop(trillium_testing::spawn(async move {
        std::thread::sleep(std::time::Duration::from_secs(1));
        tx.send(1).await.unwrap();
        2
    }));

    assert_eq!(1, rx.recv().await.unwrap());
    Ok(())
}

#[test(harness = trillium_testing::harness)]
async fn panic_in_spawn_returns_none() -> trillium_testing::TestResult {
    let fut = trillium_testing::spawn(async move {
        std::thread::sleep(std::time::Duration::from_secs(1));
        panic!();
    });

    assert!(fut.await.is_none());
    Ok(())
}
