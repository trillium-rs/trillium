use super::{
    Driver, default_config, poll_inbound,
    test_support::{pipe, run_to_completion},
};
use futures_lite::future::poll_fn;
use swansong::Swansong;
use tungstenite::{Message, protocol::Role};

#[test]
fn feeding_past_the_write_buffer_waits_for_a_slow_reader() {
    const COUNT: usize = 64;
    const LEN: usize = 1000;

    let swansong = Swansong::new();
    let (server, client) = pipe(512);
    let mut server = swansong.interrupt(Driver::new(
        Box::new(server),
        Vec::new(),
        Role::Server,
        default_config().write_buffer_size(4 * 1024),
    ));
    let mut client = swansong.interrupt(Driver::new(
        Box::new(client),
        Vec::new(),
        Role::Client,
        default_config(),
    ));

    let sending = async {
        for i in 0..COUNT {
            poll_fn(|cx| server.poll_ready_to_write(cx)).await.unwrap();
            server.write(Message::binary(vec![i as u8; LEN])).unwrap();
        }
        poll_fn(|cx| server.poll_flush(cx)).await.unwrap();
    };

    let receiving = async {
        for i in 0..COUNT {
            let message = poll_fn(|cx| poll_inbound(&mut client, cx)).await;
            assert_eq!(
                message.unwrap().unwrap(),
                Message::binary(vec![i as u8; LEN])
            );
        }
    };

    assert!(
        run_to_completion(vec![Box::pin(sending), Box::pin(receiving)]),
        "stalled"
    );
}
