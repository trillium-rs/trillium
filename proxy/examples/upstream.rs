use querystrong::QueryStrong;
use trillium::Conn;
use trillium_api::{ApiConnExt, json};
use trillium_forwarding::Forwarding;
use trillium_http::Status;
use trillium_logger::Logger;

pub fn main() {
    env_logger::init();
    trillium_smol::run((
        Logger::new(),
        Forwarding::trust_always(),
        |mut conn: Conn| async move {
            let query = QueryStrong::parse(conn.querystring()).unwrap_or_default();
            let skip_body = query.get("skip_body").is_some();
            let body = if skip_body {
                None
            } else {
                match conn.request_body().await.read_string().await {
                    Ok(body) => Some(body),
                    Err(e) => {
                        return conn
                            .with_status(Status::InternalServerError)
                            .with_body(e.to_string());
                    }
                }
            };

            let json = json!({
                "path": conn.path(),
                "method": conn.method(),
                "headers": conn.request_headers(),
                "ip": conn.inner().peer_ip(),
                "query": query,
                "body": body,
                "version": conn.inner().http_version()
            });
            conn.with_json(&json).with_status(Status::Ok)
        },
    ));
}
