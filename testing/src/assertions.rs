#[macro_export]
macro_rules! assert_status {
    ($conn:expr, $status:expr) => {{
        use std::convert::TryInto;
        let expected_status: $crate::StatusCode =
            $status.try_into().expect("expected a status code");

        match $conn.status() {
            Some(status) => assert_eq!(status, expected_status),
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
        let body = $conn.take_body_string().expect("body should exist");
        assert_eq!(body.trim_end(), $body.trim_end());
    }};
}

#[macro_export]
macro_rules! assert_body_contains {
    ($conn:expr, $pattern:expr) => {{
        let body = $conn.take_body_string().expect("body should exist");
        assert!(
            body.contains($pattern),
            "\nexpected \n`{}`\n to contain `{}`\n but it did not",
            &body,
            $pattern
        );
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

    ($conn:expr, $status:expr, $body:expr, $($header_name:literal => $header_value:expr,)+) => {
        assert_response!($conn, $status, $body, $($header_name => $header_value),+);
    };

    ($conn:expr, $status:expr, $body:expr, $($header_name:literal => $header_value:expr),*) => {
        let mut conn = $conn;
        $crate::assert_response!(&mut conn, $status, $body);
        $crate::assert_headers!(&mut conn, $($header_name => $header_value),*);
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
        $crate::assert_response!($conn, 200, $body, $($header_name => $header_value),*);
    };
}
