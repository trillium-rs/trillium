#[macro_export]
macro_rules! assert_status {
    ($conn:expr, $status:expr) => {{
        use std::convert::TryInto;
        let expected_status: trillium::http_types::StatusCode =
            $status.try_into().expect("expected a status code");

        match $conn.inner().status() {
            Some(status) => assert_eq!(*status, expected_status),
            None => panic!("expected status code, but none was set"),
        }
    }};
}

#[macro_export]
macro_rules! assert_not_handled {
    ($conn:expr) => {{
        let conn = $conn;
        assert_eq!(conn.status(), None);
        assert!(conn.inner().response_body().is_none());
        assert!(!conn.is_halted());
    }};
}

#[macro_export]
macro_rules! assert_body {
    ($conn:expr, $body:expr) => {{
        if let Some(mut body) = $conn.inner_mut().take_response_body() {
            use $crate::futures_lite::AsyncReadExt;
            let mut s = String::new();
            $crate::futures_lite::future::block_on(body.read_to_string(&mut s)).expect("read");
            assert_eq!(s, $body);
        } else {
            panic!("response body did not exist");
        }
    }};
}

#[macro_export]
macro_rules! assert_response {
    ($conn:expr, $status:expr, $body:expr) => {{
        let mut conn = $conn;
        $crate::assert_status!(conn, $status);
        $crate::assert_body!(conn, $body);
    }};

    ($conn:expr, $status:expr) => {
        $crate::assert_status!($conn, $status);
    };
}

#[macro_export]
macro_rules! assert_header {
    ($conn:expr, $header_name:expr, $header_value:expr) => {{
        let mut conn = $conn;
        let headers = conn.inner_mut().response_headers();
        assert_eq!(
            headers.get($header_name).map(|h| h.as_str()),
            Some($header_value)
        );
    }};
}

#[macro_export]
macro_rules! assert_headers {
    ($conn:expr, $($header_name:literal => $header_value:expr,)+) => {
        assert_headers!($conn, $($key => $value),+);
    };

    ($conn:expr, $($header_name:literal => $header_value:expr),*) => {
        let mut conn = $conn;
        let headers = conn.inner_mut().response_headers();
        $(
            assert_eq!(
                headers.get($header_name).map(|h| h.as_str()),
                Some($header_value),
                concat!("for header ", $header_name)
            );
        )*
    };
}

#[macro_export]
macro_rules! assert_ok {
    ($conn:expr) => {
        $crate::assert_response!($conn, 200);
    };

    ($conn:expr, $body:expr) => {
        $crate::assert_response!($conn, 200, $body);
    };


    ($conn:expr, $body:expr, $($header_name:literal => $header_value:expr,)+) => {
        assert_ok!($conn, $body, $($header_name => $header_value),+);
    };

    ($conn:expr, $body:expr, $($header_name:literal => $header_value:expr),*) => {
        let mut conn = $conn;
        $crate::assert_ok!(&mut conn, $body);
        $crate::assert_headers!(&mut conn, $($header_name => $header_value),*);
    };
}
