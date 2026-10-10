use super::*;

#[tokio::test]
async fn events_reach_the_receiver() {
    let (tx, mut rx) = start();
    tx.info("hello");
    let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    // A tick may arrive first; drain until the notification shows up.
    let mut event = received;
    for _ in 0..50 {
        if let AppEvent::Notify(text, severity) = &event {
            assert_eq!(text, "hello");
            assert_eq!(*severity, Severity::Information);
            return;
        }
        event = rx.recv().await.expect("channel closed");
    }
    panic!("notification never arrived");
}
