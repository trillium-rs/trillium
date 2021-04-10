use std::collections::HashMap;
use trillium::http_types::StatusCode;
use trillium::{BoxedTransport, Conn};
use trillium_http::Conn as HttpConn;

#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbMultiHeadersResponse {
    pub is_base64_encoded: bool,
    pub status_code: u16,
    pub status_description: String,
    pub multi_value_headers: HashMap<String, Vec<String>>,
    pub body: Option<String>,
}

impl AlbMultiHeadersResponse {
    pub async fn from_conn(conn: Conn) -> Self {
        let mut conn = conn.into_inner();
        let status = *conn.status().unwrap_or(&StatusCode::NotFound);
        let (body, is_base64_encoded) = response_body(&mut conn).await;

        let multi_value_headers =
            conn.response_headers()
                .iter()
                .fold(HashMap::new(), |mut h, (n, v)| {
                    let v: Vec<_> = v.iter().map(|h| h.to_string()).collect();
                    h.insert(n.to_string(), v);
                    h
                });

        Self {
            is_base64_encoded,
            status_code: status as u16,
            status_description: format!("{} {}", status as u16, status.canonical_reason()),
            multi_value_headers,
            body,
        }
    }
}

#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbResponse {
    pub is_base64_encoded: bool,
    pub status_code: u16,
    pub status_description: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

async fn response_body(conn: &mut HttpConn<BoxedTransport>) -> (Option<String>, bool) {
    match conn.take_response_body() {
        Some(body) => {
            let bytes = body.into_bytes().await.unwrap();
            match String::from_utf8(bytes) {
                Ok(string) => (Some(string), false),
                Err(e) => (Some(base64::encode(e.into_bytes())), true),
            }
        }
        None => (None, false),
    }
}

impl AlbResponse {
    pub async fn from_conn(conn: Conn) -> Self {
        let mut conn = conn.into_inner();
        let status = *conn.status().unwrap_or(&StatusCode::NotFound);
        let (body, is_base64_encoded) = response_body(&mut conn).await;
        let headers = conn
            .response_headers()
            .iter()
            .fold(HashMap::new(), |mut h, (n, v)| {
                h.insert(n.to_string(), v.to_string());
                h
            });

        Self {
            is_base64_encoded,
            status_code: status as u16,
            status_description: format!("{} {}", status as u16, status.canonical_reason()),
            headers,
            body,
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum LambdaResponse {
    Alb(AlbResponse),
    AlbMultiHeaders(AlbMultiHeadersResponse),
}
