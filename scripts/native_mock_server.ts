// GLM-F: local OpenRouter-compatible mock for the native smoke test ONLY
// (debug workflow; never ships). Serves /models, /key and SSE
// /chat/completions on 127.0.0.1 so the app's AICOUNCIL_BASE_URL override
// gives a fully offline send/stream/cancel path with zero real spend.
//
// Frames match what the backend parser consumes (src/providers/*):
//   delta:  {"choices":[{"delta":{"content":"..."}}]}
//   finish: {"id":"mock-gen-N","choices":[{"delta":{},"finish_reason":"stop"}]}
//   usage:  {"id":"mock-gen-N","choices":[],"usage":{"prompt_tokens":5,"completion_tokens":9,"cost":"0.000014"}}
//   end:    data: [DONE]
//
// Models: "mock-model-1" (answers immediately), "mock-slow-model"
// (streams one delta then holds the connection open for cancellation tests).
const PORT = Number(process.env.MOCK_PORT ?? 18901);

function replyText(prompt) {
  return `Mock reply: ${prompt}`;
}

function frame(obj) {
  return `data: ${JSON.stringify(obj)}\n\n`;
}

let genSeq = 0;

Bun.serve({
  hostname: "127.0.0.1",
  port: PORT,
  // 默认 10s 空闲超时会掐断慢速 SSE 长连接(取消测试需要它活着)。
  idleTimeout: 120,
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname.endsWith("/models")) {
      return Response.json({
        data: [
          {
            id: "mock-model-1",
            name: "Mock Model 1",
            context_length: 8192,
            pricing: { prompt: "0.000001", completion: "0.000002" },
            supported_parameters: ["temperature"],
          },
        ],
      });
    }
    if (url.pathname.endsWith("/key")) {
      return Response.json({ data: { label: "mock", usage: "0.1", limit: "10" } });
    }
    if (url.pathname.endsWith("/chat/completions")) {
      const body = await req.json();
      // 标题服务(stream:false):返回一行标题文本(JSON,非 SSE)
      if (body?.stream === false) {
        return Response.json({
          choices: [{ message: { content: "Smoke Title 标题" } }],
        });
      }
      const model = body?.model ?? "mock-model-1";
      const msgs = Array.isArray(body?.messages) ? body.messages : [];
      const lastUser = [...msgs].reverse().find((m) => m.role === "user");
      const prompt =
        typeof lastUser?.content === "string" ? lastUser.content : "smoke";
      const genId = `mock-gen-${++genSeq}`;
      const slow = model === "mock-slow-model";
      const stream = new ReadableStream({
        async start(controller) {
          const enc = new TextEncoder();
          const send = (obj) => controller.enqueue(enc.encode(frame(obj)));
          try {
            send({ id: genId, choices: [{ delta: { role: "assistant" } }] });
            if (slow) {
              send({ id: genId, choices: [{ delta: { content: "slow…" } }] });
              // 长挂连接,等待客户端取消(60s 后自行关闭兜底)。
              // 期间发心跳帧:真实 SSE 长连接同样有心跳,而且它让
              // "取消"能在下一个心跳被读到(否则只能等连接被关闭)。
              const heartbeat = setInterval(() => {
                try {
                  send({ id: genId, choices: [{ delta: {} }] });
                } catch {
                  clearInterval(heartbeat);
                }
              }, 500);
              try {
                await Bun.sleep(60000);
              } finally {
                clearInterval(heartbeat);
              }
            } else {
              const text = replyText(prompt);
              for (const chunk of [text.slice(0, 5), text.slice(5)]) {
                send({ id: genId, choices: [{ delta: { content: chunk } }] });
                await Bun.sleep(30);
              }
              send({
                id: genId,
                choices: [{ delta: {}, finish_reason: "stop" }],
              });
              send({
                id: genId,
                choices: [],
                usage: {
                  prompt_tokens: 5,
                  completion_tokens: 9,
                  cost: "0.000014",
                },
              });
            }
            controller.enqueue(enc.encode("data: [DONE]\n\n"));
          } catch {
            // 客户端取消导致写入失败:正常路径,静默关闭
          } finally {
            controller.close();
          }
        },
      });
      return new Response(stream, {
        headers: { "content-type": "text/event-stream" },
      });
    }
    return new Response("not found", { status: 404 });
  },
});

console.log(`MOCK_READY http://127.0.0.1:${PORT}`);
