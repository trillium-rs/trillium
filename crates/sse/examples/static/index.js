var source = new EventSource("/sse");
source.onmessage = function (event) {
  let li = document.createElement("li");
  li.innerHTML = `<code><pre>${event.data}</pre></code>`;
  window.document.querySelector("#messages").appendChild(li);
};
