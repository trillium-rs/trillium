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
