## WebSocket support

WebSockets work a lot like tide's, since I recently wrote that
interface as well. One difference in trillium is that the websocket
connection also contains some aspects of the original http request,
such as request headers, the request path and method, and any state
that has been accumulated by previous handlers in a sequence.

```rust
{{#include ../../../websockets/examples/websockets.rs}}
```


