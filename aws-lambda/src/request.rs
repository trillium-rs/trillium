#![allow(dead_code)]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Deserializer};
use std::{collections::HashMap, str::FromStr};
use trillium_http::{Conn as HttpConn, Method, Synthetic};

#[cfg(test)]
mod test {

    const JSON: &str = r#"{"requestContext":{"elb":{"targetGroupArn":"arn:aws:elasticloadbalancing:us-west-2:915490588716:targetgroup/rust-lambda/a6825ef90a29cea9"}},"httpMethod":"GET","path":"/template/anything-here","multiValueQueryStringParameters":{},"multiValueHeaders":{"content-length":["0"],"cookie":["trillium.sid=aXqp%2F9p06OurE0NrgmU4H0O5fCfYmiVehIb+W7J3lH0%3DLAAAAAAAAABMTzNzV3JpZEZrclhnekNVdithMi82R0o1UUkwTTZ5SjUyUjlCSVdNdC9NPQEeAAAAAAAAADIwMjEtMDItMDFUMTk6Mjg6MDcuMTcxMDkzNzEwWgEAAAAAAAAABQAAAAAAAABjb3VudAEAAAAAAAAANA%3D%3D"],"host":["rust-lambda-1068582226.us-west-2.elb.amazonaws.com"],"x-amzn-trace-id":["Root=1-60174c71-6ea8cbb45b214504613872a1"],"x-forwarded-for":["8.45.45.25"],"x-forwarded-port":["80"],"x-forwarded-proto":["http"]},"body":"","isBase64Encoded":false}"#;

    #[test]
    fn test() {
        let _t: serde_json::Result<super::LambdaRequest> = serde_json::from_str(JSON);
    }
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbRequest {
    #[serde(deserialize_with = "deserialize_method")]
    pub http_method: Method,
    pub path: String,
    pub query_string_parameters: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub request_context: serde_json::Value,
    pub is_base64_encoded: bool,
    pub body: Option<String>,
}

fn deserialize_method<'de, D>(deserializer: D) -> Result<Method, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = String::deserialize(deserializer)?;
    Method::from_str(&buf).map_err(serde::de::Error::custom)
}

// fn deserialize_cron_schedule<'de, D>(deserializer: D) -> Result<cron::Schedule, D::Error>
// where D: Deserializer<'de> {
impl AlbRequest {
    pub async fn into_conn(self) -> HttpConn<Synthetic> {
        let Self {
            http_method,
            path,
            headers,
            is_base64_encoded,
            body,
            //            request_context,
            //            query_string_parameters,
            ..
        } = self;
        let body = standardize_body(body, is_base64_encoded);
        let mut conn = HttpConn::new_synthetic(http_method, path, body);
        conn.request_headers_mut().extend(headers);
        conn
    }
}

fn standardize_body(body: Option<String>, is_base64_encoded: bool) -> Option<Vec<u8>> {
    body.map(|s| {
        if is_base64_encoded {
            BASE64.decode(s).unwrap()
        } else {
            s.into_bytes()
        }
    })
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbMultiHeadersRequest {
    #[serde(deserialize_with = "deserialize_method")]
    pub http_method: Method,
    pub path: String,
    pub multi_value_query_string_parameters: HashMap<String, Vec<String>>,
    pub multi_value_headers: HashMap<String, Vec<String>>,
    pub request_context: serde_json::Value,
    pub is_base64_encoded: bool,
    pub body: Option<String>,
}

impl AlbMultiHeadersRequest {
    pub async fn into_conn(self) -> HttpConn<Synthetic> {
        let Self {
            http_method,
            path,
            //multi_value_query_string_parameters,
            multi_value_headers,
            //request_context,
            is_base64_encoded,
            body,
            ..
        } = self;
        let body = standardize_body(body, is_base64_encoded);
        let mut conn = HttpConn::new_synthetic(http_method, path, body);
        conn.request_headers_mut().extend(multi_value_headers);

        conn
    }
}

#[derive(serde::Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum LambdaRequest {
    Alb(AlbRequest),
    AlbMultiHeaders(AlbMultiHeadersRequest),
}
