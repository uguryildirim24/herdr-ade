// Test-only mock provider for the guard check (SPEC-pi v2 §7, T7).
// Answers OpenAI-compatible chat completions with a fixed failure:
//   node mock-provider.js limit 19890 /var/tmp/ade-a5/guard-test/log/hits.log
// `limit` -> 429 with retry-after, `login` -> 401, `context` -> 400 with a
// context-length message (class `error`, not `login`), `ok` -> a tiny stream.
// Never used by the plugin; throwaway fixtures only.
const http = require("node:http");
const fs = require("node:fs");

const mode = process.argv[2] || "limit";
const port = Number(process.argv[3] || 19890);
const hits = process.argv[4] || "";

function hit() {
  if (!hits) return;
  try {
    fs.appendFileSync(hits, `${mode}\n`);
  } catch {
    // The check asserts on the log; a lost fixture line fails the check.
  }
}

const server = http.createServer((req, res) => {
  if (!req.url.includes("/chat/completions")) {
    res.writeHead(404);
    res.end();
    return;
  }
  hit();
  if (mode === "limit") {
    res.writeHead(429, { "content-type": "application/json", "retry-after": "30" });
    res.end(
      JSON.stringify({
        error: { message: "Rate limit reached for mock-model", type: "rate_limit_error" },
      }),
    );
    return;
  }
  if (mode === "login") {
    res.writeHead(401, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        error: { message: "Incorrect API key provided: mock-key", type: "invalid_request_error" },
      }),
    );
    return;
  }
  if (mode === "context") {
    res.writeHead(400, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        error: {
          message: "This model's maximum context length is 128000 tokens",
          type: "invalid_request_error",
        },
      }),
    );
    return;
  }
  res.writeHead(200, { "content-type": "text/event-stream" });
  res.write(
    'data: {"id":"1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant","content":"ok"},"index":0,"finish_reason":null}]}\n\n',
  );
  res.write(
    'data: {"id":"1","object":"chat.completion.chunk","choices":[{"delta":{},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}\n\n',
  );
  res.write("data: [DONE]\n\n");
  res.end();
});

server.listen(port, "127.0.0.1", () => {
  console.log(`mock-provider ${mode} on 127.0.0.1:${port}`);
});
