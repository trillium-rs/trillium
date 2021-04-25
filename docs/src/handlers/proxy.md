# Proxy

Trillium includes a custom http client implementation in order to
support reverse proxying requests. There are two tls implementations
for this client.

```rust
{{#include ../../../proxy/examples/proxy.rs}}
```
