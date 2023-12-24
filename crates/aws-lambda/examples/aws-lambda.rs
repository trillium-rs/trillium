fn main() {
    trillium_aws_lambda::run(|conn: trillium::Conn| async move { conn.ok("hello!") });
}
