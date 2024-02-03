use std::time::{Duration, Instant};

use async_io::Timer;

fn main() {
    trillium_smol::run(|mut conn: trillium::Conn| async move {
        let start = Instant::now();
        let count = conn
            .cancel_on_disconnect(async move {
                let count = fastrand::u8(..10);
                for i in 0..count {
                    Timer::after(Duration::from_millis(500)).await;
                    println!("{i}: {:?} elapsed", start.elapsed());
                }
                count
            })
            .await;
        if let Some(count) = count {
            println!("completed");
            conn.ok(format!("ok: {count}"))
        } else {
            println!("disconnected");
            conn
        }
    });
}
