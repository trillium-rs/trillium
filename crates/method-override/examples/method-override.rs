/// make a post request to http://localhost:8080/?_method=delete to
/// see the method overridden
fn main() {
    trillium_smol::run((
        trillium_method_override::MethodOverride::new(),
        |conn: trillium::Conn| async move {
            let body = format!("method was: {}", conn.method());
            conn.with_body(body)
        },
    ))
}
