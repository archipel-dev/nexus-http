# Collection runner

The runner sends the requests of a workspace or folder in tree order, feeds each response into
the history (so `{% response %}` chaining works) and evaluates every request's assertions.

## Streaming requests (SSE and WebSocket)

Requests with the Server-Sent Events or WebSocket protocol are **not** sent as plain GET.
The runner uses a *bounded receive*: it connects through the same `stream` module as the
request view, collects messages until `stream_messages` (default 3) arrived or
`stream_timeout` (default 3 s) elapsed, then closes the connection.

- The received messages become the response body, one per line, so assertions such as
  `body contains …` apply. Status is `200` (SSE) or `101` (WebSocket); the method column
  shows `SSE` / `WS`.
- A step that connects (even with zero messages) and passes its assertions is shown as
  **Streamed** and counted in the "N streamed" tag; it is not a failure.
- Failing to connect, or an error before the stream opened, is a real failure.

Decision: bounded receive over skipping, because the stream module already exposes
`recv_timeout`, so it costs ~80 lines and keeps streaming endpoints covered by CI runs.
