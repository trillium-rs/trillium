/**
build a new [`trillium::Sequence`](crate::Sequence). see that type for more information.

```
let macro_sequence = trillium::sequence![trillium_logger::DevLogger, "hello"];
let literal_sequence = trillium::Sequence::new().then(trillium_logger::DevLogger).then("hello");
assert_eq!(format!("{:?}", macro_sequence), format!("{:?}", literal_sequence));
```
*/

#[macro_export]
macro_rules! sequence {
    ($($x:expr),+ $(,)?) => { $crate::Sequence::new()$(.then($x))+ }
}

/**

*/
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
                return $conn.with_status(500).halt();
            }
        }
    };
}

#[macro_export]
macro_rules! conn_ok {
    ($conn:expr, $result:expr) => {
        match $result {
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
