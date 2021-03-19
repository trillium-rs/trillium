(() => {
  const sock = (window._devServerWebsocket = new WebSocket(
    `ws://${window.location.host}/_dev_server.ws`
  ));
  sock.addEventListener("message", ({ data }) => {
    const message = JSON.parse(data);
    switch (message.type) {
      case "Restarted":
        window.location.reload();
        break;
      case "CompileError":
        document.body.innerHTML = `<code><pre>${message.error}</pre></code>`;
        document.body.style.cursor = "pointer";
        document.body.style.background = "#fee";
        break;
      case "BuildSuccess":
        document.body.style.cursor = "pointer";
        document.body.style.background = "white";
        break;
      case "BinaryChanged":
      case "Rebuild":
        document.body.style.background = "#eee";
        document.body.style.cursor = "wait";
      default:
        console.log(data);
    }
  });
})();
