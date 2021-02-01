fn main() {
    myco_aws_lambda::run(|conn: myco::Conn| async move { conn.ok("hello!") });
}
