use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use std::collections::HashMap;
use trillium::Conn;
use trillium_http::Status;

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
    pub async fn from_conn(mut conn: Conn) -> Self {
        let status = conn.status().unwrap_or(Status::NotFound);
        let (body, is_base64_encoded) = response_body(&mut conn).await;

        let multi_value_headers = conn
            .inner()
            .response_headers()
            .iter()
            .map(|(n, v)| (n.to_string(), v.iter().map(|v| v.to_string()).collect()))
            .collect();

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

async fn response_body(conn: &mut Conn) -> (Option<String>, bool) {
    match conn.inner_mut().take_response_body() {
        Some(body) => {
            let bytes = body.into_bytes().await.unwrap();
            match String::from_utf8(bytes.to_vec()) {
                Ok(string) => (Some(string), false),
                Err(e) => (Some(BASE64.encode(e.into_bytes())), true),
            }
        }
        None => (None, false),
    }
}

impl AlbResponse {
    pub async fn from_conn(mut conn: Conn) -> Self {
        let status = conn.status().unwrap_or(Status::NotFound);
        let (body, is_base64_encoded) = response_body(&mut conn).await;
        let headers =
            conn.inner()
                .response_headers()
                .iter()
                .fold(HashMap::new(), |mut h, (n, v)| {
                    if let Some(one) = v.one() {
                        h.insert(n.to_string(), one.to_string());
                    }
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
