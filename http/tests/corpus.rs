use indoc::formatdoc;
use pretty_assertions::assert_str_eq;
use std::{env, net::Shutdown, path::PathBuf};
use test_harness::test;
use trillium_http::{Conn, KnownHeaderName, Swansong};
use trillium_testing::{RuntimeTrait, TestTransport, harness};
const TEST_DATE: &str = "Tue, 21 Nov 2023 21:27:21 GMT";

async fn handler(mut conn: Conn<TestTransport>) -> Conn<TestTransport> {
    let response_body = formatdoc! {"
        ===request===
        method: {method}
        path: {path}
        version: {version}

        ===headers===
        {headers}
        ",
        method = conn.method(),
        path = conn.path(),
        version = conn.http_version(),
        headers = conn.request_headers()
    };

    conn.response_headers_mut()
        .insert(KnownHeaderName::Date, TEST_DATE);
    conn.response_headers_mut()
        .insert(KnownHeaderName::Server, "corpus-test");

    match conn.request_body().await.read_string().await {
        Ok(request_body) => {
            conn.set_status(200);
            conn.set_response_body(format!("{response_body}===body===\n{request_body}"));
        }

        Err(e) => {
            conn.set_status(500);
            conn.set_response_body(format!("{response_body}===error===\n{e}"));
        }
    };

    conn
}

#[test(harness)]
async fn corpus_test() {
    env_logger::init();
    let runtime = trillium_testing::runtime();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let filter = env::var("CORPUS_TEST_FILTER").unwrap_or_default();
    let corpus_request_files = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|f| {
            let path = f.unwrap().path();
            if path.extension().and_then(|x| x.to_str()) == Some("request") {
                Some(path)
            } else {
                None
            }
        })
        .filter(|f| f.to_str().unwrap().contains(&filter));

    for file in corpus_request_files {
        let request = std::fs::read_to_string(&file)
            .unwrap_or_else(|_| panic!("could not read {}", file.display()))
            .replace(['\r', '\n'], "")
            .replace("\\r", "\r")
            .replace("\\n", "\n");

        let (client, server) = TestTransport::new();
        let swansong = Swansong::new();
        let res = runtime.spawn({
            let swansong = swansong.clone();
            async move { Conn::map(server, swansong, handler).await }
        });

        client.write_all(request);
        client.shutdown(Shutdown::Write);
        let (response, extension) = match res.await.unwrap() {
            Ok(None) => (client.read_available_string().await, "response"),
            Err(e) => (e.to_string(), "error"),
            Ok(Some(_)) => ("".to_string(), "upgrade"),
        };

        let response_file = file.with_extension(extension);

        if option_env!("CORPUS_TEST_WRITE").is_some() {
            std::fs::write(
                response_file,
                response.replace('\r', "\\r").replace('\n', "\\n\n"),
            )
            .unwrap();
        } else {
            let expected_response = std::fs::read_to_string(response_file)
                .unwrap()
                .replace(['\n', '\r'], "")
                .replace("\\r", "\r")
                .replace("\\n", "\n");
            assert_str_eq!(expected_response, response, "\n\n{file:?}");
        }

        swansong.shut_down();
    }
}
