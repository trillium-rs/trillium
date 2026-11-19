use async_tungstenite::{WebSocketStream, client_async, tungstenite::Message};
use futures_lite::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use trillium_channels::{ChannelConn, ChannelEvent, ChannelHandler, channel};
use trillium_testing::{Runtime, ServerConnector, TestTransport, test, with_runtime};

struct Lobby;

impl ChannelHandler for Lobby {
    async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        if event.topic() == "rooms:lobby" {
            conn.allow_join(&event, &()).await;
        } else {
            conn.reject(&event, &json!({"reason": "unknown room"}))
                .await;
        }
    }

    async fn incoming_message(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        match event.event() {
            "new:msg" => {
                conn.reply_ok(&event, &()).await;
                conn.broadcast(event);
            }
            "reject" => {
                // An ordinary send must still be filtered on an unjoined topic,
                // even if the event name matches a reply.
                conn.send_event(event.build_reply("phx_reply", &())).await;
                conn.reject(&event, &json!({"reason": "forbidden"})).await;
            }
            "fail" => {
                conn.mark_channel_errored(&event, &json!({"reason": "failed"}))
                    .await
            }
            _ => {}
        }
    }
}

struct Client<'a> {
    socket: WebSocketStream<TestTransport>,
    runtime: &'a Runtime,
}

impl<'a> Client<'a> {
    async fn connect(
        runtime: &'a Runtime,
        server: &ServerConnector<impl trillium::Handler>,
        url: &str,
    ) -> Self {
        let (socket, _) = runtime
            .timeout(
                Duration::from_secs(5),
                client_async(url, server.connect(false).await),
            )
            .await
            .expect("WebSocket handshake timed out")
            .unwrap();
        Self { socket, runtime }
    }

    async fn send(&mut self, frame: Value) {
        self.socket
            .send(Message::text(frame.to_string()))
            .await
            .unwrap();
    }

    async fn receive(&mut self) -> Value {
        let frame = self
            .runtime
            .timeout(Duration::from_secs(5), self.socket.next())
            .await
            .expect("timed out waiting for a channel frame")
            .expect("WebSocket closed")
            .unwrap();
        serde_json::from_str(frame.to_text().unwrap()).unwrap()
    }
}

#[test(harness = with_runtime)]
async fn broadcasts_are_independent_of_the_senders_join(runtime: Runtime) {
    let server = ServerConnector::new(channel(Lobby));
    let v2_url = "ws://localhost/socket/websocket?token=example&vsn=2.0.0";
    let mut alice = Client::connect(&runtime, &server, v2_url).await;
    let mut bob = Client::connect(&runtime, &server, v2_url).await;
    let mut legacy = Client::connect(&runtime, &server, "ws://localhost/socket/websocket").await;

    let ok = json!({"status": "ok", "response": null});
    for (client, join_ref) in [(&mut alice, "1"), (&mut bob, "2")] {
        client
            .send(json!([join_ref, join_ref, "rooms:lobby", "phx_join", {}]))
            .await;
        assert_eq!(
            client.receive().await,
            json!([join_ref, join_ref, "rooms:lobby", "phx_reply", ok])
        );
    }
    legacy
        .send(json!({"ref": "3", "topic": "rooms:lobby", "event": "phx_join", "payload": {}}))
        .await;
    assert_eq!(
        legacy.receive().await,
        json!({"join_ref": null, "ref": "3", "topic": "rooms:lobby", "event": "phx_reply", "payload": ok})
    );

    alice
        .send(json!([null, "4", "phoenix", "heartbeat", {}]))
        .await;
    assert_eq!(
        alice.receive().await,
        json!([null, "4", "phoenix", "phx_reply", ok])
    );

    let payload = json!({"body": "hello"});
    alice
        .send(json!(["1", "5", "rooms:lobby", "new:msg", payload]))
        .await;
    let broadcast = json!([null, null, "rooms:lobby", "new:msg", payload]);
    assert_eq!(bob.receive().await, broadcast);
    assert_eq!(
        legacy.receive().await,
        json!({"join_ref": null, "ref": null, "topic": "rooms:lobby", "event": "new:msg", "payload": payload})
    );

    // Broadcast and direct replies are delivered from separate queues.
    let reply = json!(["1", "5", "rooms:lobby", "phx_reply", ok]);
    let first = alice.receive().await;
    let second = alice.receive().await;
    assert!(
        (first == reply && second == broadcast) || (first == broadcast && second == reply),
        "unexpected frames: {first}, {second}"
    );
}

#[test(harness = with_runtime)]
async fn rejection_replies_do_not_require_or_create_subscriptions(runtime: Runtime) {
    let server = ServerConnector::new(channel(Lobby));
    let mut client = Client::connect(
        &runtime,
        &server,
        "ws://localhost/socket/websocket?vsn=2.0.0",
    )
    .await;

    client
        .send(json!(["1", "1", "rooms:unknown", "phx_join", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["1", "1", "rooms:unknown", "phx_reply", {"status": "error", "response": {"reason": "unknown room"}}])
    );

    client
        .send(json!(["1", "2", "rooms:unknown", "reject", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["1", "2", "rooms:unknown", "phx_reply", {"status": "error", "response": {"reason": "forbidden"}}])
    );
}

#[test(harness = with_runtime)]
async fn leave_is_acknowledged_and_channel_errors_are_lifecycle_events(runtime: Runtime) {
    let server = ServerConnector::new(channel(Lobby));
    let mut client = Client::connect(
        &runtime,
        &server,
        "ws://localhost/socket/websocket?vsn=2.0.0",
    )
    .await;
    let ok = json!({"status": "ok", "response": null});

    client
        .send(json!(["1", "1", "rooms:lobby", "phx_join", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["1", "1", "rooms:lobby", "phx_reply", ok])
    );

    client
        .send(json!(["1", "2", "rooms:lobby", "fail", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["1", "2", "rooms:lobby", "phx_error", {"reason": "failed"}])
    );

    client
        .send(json!(["1", "3", "rooms:lobby", "phx_leave", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["1", "3", "rooms:lobby", "phx_reply", ok])
    );

    client
        .send(json!(["1", "4", "rooms:lobby", "reject", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["1", "4", "rooms:lobby", "phx_reply", {"status": "error", "response": {"reason": "forbidden"}}])
    );
}
