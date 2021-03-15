#[macro_export]
macro_rules! sequence {
    ($($x:expr),+ $(,)?) => { $crate::Sequence::new()$(.and($x))+ }
}

#[cfg(test)]
mod test {
    #[test]
    fn test() {
        crate::sequence![
            |conn: crate::Conn| async move { conn },
            |conn: crate::Conn| async move { conn }
        ];
    }
}

#[macro_export]
macro_rules! conn_try {
    ($conn:expr, $expr:expr) => {
        conn_try!($conn, $expr, "error")
    };

    ($conn:expr, $expr:expr, $format_str:literal) => {
        match $expr {
            Ok(value) => value,
            Err(error) => {
                log::error!(
                    concat!("{}:{} ", $format_str, ": {}"),
                    file!(),
                    line!(),
                    error
                );
                return $conn.status(500);
            }
        }
    };
}

#[macro_export]
macro_rules! conn_ok {
    ($conn:expr, $expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => return $conn,
        }
    };
}

#[macro_export]
macro_rules! log_error {
    ($expr:expr) => {
        if let Err(err) = $expr {
            log::error!("{}:{} {:?}", file!(), line!(), err);
        }
    };

    ($expr:expr, $message:expr) => {
        if let Err(err) = $expr {
            log::error!("{}:{} {} {:?}", file!(), line!(), $message, err);
        }
    };
}
