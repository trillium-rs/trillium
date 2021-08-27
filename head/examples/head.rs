fn main() {
    trillium_smol::run((
        trillium_head::Head::new(),
        "this body should not be sent \
         if you make a HEAD request, but the \
         content-length and other headers should be correct",
    ))
}
