/**
 * Small CDP driver used to capture the UI states of the mock harness.
 * Development tool only; not part of the app bundle.
 */

const PORT = process.env.CDP_PORT ?? "9222";
const URL_TARGET = process.env.PAGE_URL ?? "http://localhost:5199/dev/index.html";

interface Target {
  type: string;
  url: string;
  webSocketDebuggerUrl: string;
}

async function targets(): Promise<Target[]> {
  const response = await fetch(`http://127.0.0.1:${PORT}/json`);
  return (await response.json()) as Target[];
}

function connect(url: string) {
  const socket = new WebSocket(url);
  let id = 0;
  const pending = new Map<
    number,
    { resolve(value: unknown): void; reject(error: unknown): void }
  >();

  socket.addEventListener("message", (event) => {
    const data = JSON.parse(String(event.data)) as {
      id?: number;
      result?: unknown;
      error?: unknown;
    };
    if (data.id === undefined) return;
    const entry = pending.get(data.id);
    if (!entry) return;
    pending.delete(data.id);
    if (data.error) entry.reject(data.error);
    else entry.resolve(data.result);
  });

  const ready = new Promise<void>((resolve) => {
    socket.addEventListener("open", () => resolve());
  });

  return {
    ready,
    socket,
    send(method: string, params: Record<string, unknown> = {}) {
      id += 1;
      const messageId = id;
      return new Promise<unknown>((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(messageId);
          reject(new Error(`CDP timeout: ${method}`));
        }, 15000);
        pending.set(messageId, {
          resolve: (value) => {
            clearTimeout(timer);
            resolve(value);
          },
          reject: (error) => {
            clearTimeout(timer);
            reject(error);
          },
        });
        socket.send(JSON.stringify({ id: messageId, method, params }));
      });
    },
  };
}

async function main() {
  const list = await targets();
  const page = list.find(
    (entry) => entry.type === "page" && entry.webSocketDebuggerUrl,
  );
  if (!page) throw new Error("no debuggable page");
  const client = connect(page.webSocketDebuggerUrl);
  await client.ready;

  await client.send("Page.enable");
  await client.send("Runtime.enable");
  await client.send("Page.navigate", { url: URL_TARGET });
  await new Promise((resolve) => setTimeout(resolve, 2500));

  const shots: { name: string; script: string }[] = [
    {
      name: "panel-models",
      script: `(() => {
        const buttons = [...document.querySelectorAll("header.main__header button")];
        const models = buttons.find((b) => b.textContent.includes("Models"));
        models?.click();
        return models?.textContent ?? "not found";
      })()`,
    },
    {
      name: "panel-settings",
      script: `(() => {
        const buttons = [...document.querySelectorAll("header.main__header button")];
        const settings = buttons.find((b) => b.textContent.includes("Settings"));
        settings?.click();
        return settings?.textContent ?? "not found";
      })()`,
    },
    {
      name: "model-picker",
      script: `(() => {
        const pickers = [...document.querySelectorAll(".composer__models button")];
        pickers[0]?.click();
        return pickers[0]?.textContent ?? "not found";
      })()`,
    },
    {
      name: "send-stream",
      script: `(() => {
        const field = document.querySelector("textarea.composer__input");
        const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
        setter.call(field, "Explain the Soft Sage palette");
        field.dispatchEvent(new Event("input", { bubbles: true }));
        return "typed";
      })()`,
    },
    {
      name: "streaming",
      script: `(() => {
        const buttons = [...document.querySelectorAll(".composer__box button")];
        const send = buttons.find((b) => b.textContent.includes("Send"));
        send?.click();
        return "sent";
      })()`,
    },
  ];

  for (const shot of shots) {
    const result = await client.send("Runtime.evaluate", {
      expression: shot.script,
      returnByValue: true,
    });
    await new Promise((resolve) => setTimeout(resolve, 700));
    const capture = (await client.send("Page.captureScreenshot", {
      format: "png",
    })) as { data: string };
    const out = process.env.OUT_DIR ?? "/tmp";
    await Bun.write(`${out}/${shot.name}.png`, Buffer.from(capture.data, "base64"));
    console.log(shot.name, JSON.stringify(result));
  }

  client.socket.close();
}

await main();

export {};
