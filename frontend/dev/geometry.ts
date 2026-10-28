/**
 * Real-browser geometry harness (development only).
 *
 * The unit tests run in happy-dom, which has **no layout engine**: they can
 * show that a node exists, never that it is on screen. The user reported
 * broken geometry in the actual browser, so the geometry is verified here,
 * in a real Chrome, with real input events.
 *
 * What it checks (all of it against the mock harness, never against real
 * user data):
 *
 *   1. the chat column keeps its width and height through 20 sidebar
 *      collapse/expand cycles (no collapsed main, no scroll or draft loss);
 *   2. the composer stays visible and editable at 880x520 and 1150x680;
 *   3. a real wheel event scrolls the thread (and only the thread);
 *   4. expanding a long question grows the **body** while the footer with
 *      the controls stays visible and clickable;
 *   5. the answer actions sit below the text, at the bottom right;
 *   6. the Models/Settings popups scroll on their own and do not move the
 *      chat; `Escape` closes them and the focus comes back;
 *   7. a single-model conversation shows neither an answer tab strip nor
 *      branch controls, a parallel conversation shows both.
 *
 * Usage (from `frontend/`):
 *
 *     /opt/homebrew/bin/bun dev/geometry.ts
 *
 * Ports and paths are unique to this task: Vite 5199, CDP 19221, temp
 * directory `aicouncil-ui-A-*`.
 */

import { mkdtemp } from "node:fs/promises";
import { join } from "node:path";

const CDP_PORT = Number(process.env.A_CDP_PORT ?? 19221);
const VITE_PORT = Number(process.env.A_VITE_PORT ?? 5199);
const PAGE_URL = `http://127.0.0.1:${VITE_PORT}/dev/index.html?fixture=long`;
const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const TEMP_ROOT =
  process.env.A_TEMP_DIR ??
  "/private/var/folders/dt/hqqgsgbj2kx2y6zk22dvzrc80000gn/T/opencode";

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
  top: number;
  bottom: number;
  left: number;
  right: number;
}

interface CheckResult {
  name: string;
  ok: boolean;
  details: string;
}

const results: CheckResult[] = [];
let failures = 0;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function report(name: string, ok: boolean, details: string): void {
  results.push({ name, ok, details });
  if (!ok) failures += 1;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}  ${details}`);
}

// ---------------------------------------------------------------- CDP client

interface Target {
  type: string;
  url: string;
  webSocketDebuggerUrl: string;
}

class Cdp {
  private id = 0;
  private readonly pending = new Map<
    number,
    { resolve(value: unknown): void; reject(error: unknown): void }
  >();
  readonly socket: WebSocket;

  constructor(url: string) {
    this.socket = new WebSocket(url);
    this.socket.addEventListener("message", (event) => {
      const data = JSON.parse(String(event.data)) as {
        id?: number;
        result?: unknown;
        error?: unknown;
      };
      if (data.id === undefined) return;
      const entry = this.pending.get(data.id);
      if (!entry) return;
      this.pending.delete(data.id);
      if (data.error) entry.reject(data.error);
      else entry.resolve(data.result);
    });
  }

  ready(): Promise<void> {
    return new Promise((resolve) => {
      this.socket.addEventListener("open", () => resolve());
    });
  }

  send(method: string, params: Record<string, unknown> = {}): Promise<unknown> {
    this.id += 1;
    const messageId = this.id;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(messageId);
        reject(new Error(`CDP timeout: ${method}`));
      }, 20000);
      this.pending.set(messageId, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      this.socket.send(JSON.stringify({ id: messageId, method, params }));
    });
  }

  async evaluate<T>(expression: string): Promise<T> {
    const result = (await this.send("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    })) as {
      result?: { value?: T };
      exceptionDetails?: { text?: string; exception?: { description?: string } };
    };
    if (result.exceptionDetails) {
      throw new Error(
        `evaluate failed: ${
          result.exceptionDetails.exception?.description ??
          result.exceptionDetails.text
        }`,
      );
    }
    return result.result?.value as T;
  }
}

// ---------------------------------------------------------------- page helpers

async function rectOf(cdp: Cdp, selector: string): Promise<Rect | null> {
  return cdp.evaluate<Rect | null>(`(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height,
             top: r.top, bottom: r.bottom, left: r.left, right: r.right };
  })()`);
}

/** Rectangle of the element a JS expression resolves to. */
async function rectOfExpr(cdp: Cdp, expression: string): Promise<Rect | null> {
  return cdp.evaluate<Rect | null>(`(() => {
    const el = ${expression};
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height,
             top: r.top, bottom: r.bottom, left: r.left, right: r.right };
  })()`);
}

async function scrollIntoView(cdp: Cdp, selector: string, index = 0): Promise<void> {
  await cdp.evaluate(`(() => {
    const list = document.querySelectorAll(${JSON.stringify(selector)});
    const el = list[${index}];
    if (!el) return false;
    el.scrollIntoView({ block: "center", inline: "nearest" });
    return true;
  })()`);
  await sleep(120);
}

async function clickAt(cdp: Cdp, x: number, y: number): Promise<void> {
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseMoved",
    x,
    y,
  });
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x,
    y,
    button: "left",
    buttons: 1,
    clickCount: 1,
  });
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x,
    y,
    button: "left",
    buttons: 0,
    clickCount: 1,
  });
  await sleep(140);
}

async function clickSelector(cdp: Cdp, selector: string, index = 0): Promise<void> {
  const box = await cdp.evaluate<{ x: number; y: number } | null>(`(() => {
    const list = document.querySelectorAll(${JSON.stringify(selector)});
    const el = list[${index}];
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  if (!box) throw new Error(`click target not found: ${selector}[${index}]`);
  await clickAt(cdp, box.x, box.y);
}

/** Click the element a JS expression resolves to (re-reads the position). */
async function clickExpr(cdp: Cdp, expression: string): Promise<void> {
  const box = await boxOfExpr(cdp, expression);
  if (!box) throw new Error("click target not found");
  await clickAt(cdp, box.x, box.y);
}

/** Centre of the element a selector resolves to (`null` when missing). */
async function boxOf(
  cdp: Cdp,
  selector: string,
  index = 0,
): Promise<{ x: number; y: number } | null> {
  return cdp.evaluate<{ x: number; y: number } | null>(`(() => {
    const list = document.querySelectorAll(${JSON.stringify(selector)});
    const el = list[${index}];
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return null;
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
}

async function boxOfExpr(
  cdp: Cdp,
  expression: string,
): Promise<{ x: number; y: number } | null> {
  return cdp.evaluate<{ x: number; y: number } | null>(`(() => {
    const el = ${expression};
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return null;
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
}

async function wheel(cdp: Cdp, x: number, y: number, deltaY: number): Promise<void> {
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseWheel",
    x,
    y,
    deltaX: 0,
    deltaY,
  });
  await sleep(180);
}

async function setViewport(cdp: Cdp, width: number, height: number): Promise<void> {
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width,
    height,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await sleep(200);
}

async function pressEscape(cdp: Cdp): Promise<void> {
  await pressKey(cdp, "Escape", 27);
}

/** A real key press (the browser performs the default action). */
async function pressKey(
  cdp: Cdp,
  key: string,
  code: number,
  modifiers = 0,
): Promise<void> {
  await cdp.send("Input.dispatchKeyEvent", {
    type: "rawKeyDown",
    key,
    code: key,
    windowsVirtualKeyCode: code,
    nativeVirtualKeyCode: code,
    modifiers,
  });
  await cdp.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key,
    code: key,
    windowsVirtualKeyCode: code,
    nativeVirtualKeyCode: code,
    modifiers,
  });
  await sleep(80);
}

// ---------------------------------------------------------------- process glue

async function waitForHttp(url: string, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return true;
    } catch {
      /* not up yet */
    }
    await sleep(200);
  }
  return false;
}

async function main(): Promise<void> {
  const tempDir = await mkdtemp(join(TEMP_ROOT, "aicouncil-ui-A-"));
  const profileDir = join(tempDir, "chrome-profile");
  const captureDir = join(tempDir, "captures");

  const vite = Bun.spawn(
    [
      "/opt/homebrew/bin/bun",
      "node_modules/vite/bin/vite.js",
      "--mode",
      "mock",
      "--port",
      String(VITE_PORT),
      "--strictPort",
      "--host",
      "127.0.0.1",
    ],
    {
      cwd: new URL("..", import.meta.url).pathname,
      stdout: "pipe",
      stderr: "pipe",
    },
  );

  const chrome = Bun.spawn(
    [
      CHROME,
      "--headless=new",
      `--remote-debugging-port=${CDP_PORT}`,
      `--user-data-dir=${profileDir}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-background-timer-throttling",
      "--disable-renderer-backgrounding",
      "--force-device-scale-factor=1",
      "--window-size=1150,680",
      PAGE_URL,
    ],
    { stdout: "pipe", stderr: "pipe" },
  );

  let cdp: Cdp | null = null;
  const cleanup = () => {
    try {
      cdp?.socket.close();
    } catch {
      /* ignore */
    }
    chrome.kill();
    vite.kill();
  };
  process.on("SIGINT", () => {
    cleanup();
    process.exit(130);
  });
  process.on("SIGTERM", () => {
    cleanup();
    process.exit(143);
  });

  try {
    const viteUp = await waitForHttp(`http://127.0.0.1:${VITE_PORT}/dev/index.html`, 30000);
    if (!viteUp) throw new Error("vite dev server did not start");
    const cdpUp = await waitForHttp(`http://127.0.0.1:${CDP_PORT}/json/version`, 30000);
    if (!cdpUp) throw new Error("chrome devtools endpoint did not start");

    const targets = (await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`)).json()) as Target[];
    const page = targets.find(
      (entry) => entry.type === "page" && entry.webSocketDebuggerUrl,
    );
    if (!page) throw new Error("no debuggable page");
    cdp = new Cdp(page.webSocketDebuggerUrl);
    await cdp.ready();
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");

    // Wait until the app painted the synthetic chat.
    let booted = false;
    for (let attempt = 0; attempt < 60; attempt += 1) {
      const ready = await cdp.evaluate<boolean>(
        `!!document.querySelector(".thread") && document.querySelectorAll(".turn").length > 3`,
      );
      if (ready) {
        booted = true;
        break;
      }
      await sleep(250);
    }
    if (!booted) throw new Error("the mock app did not render the thread");

    try {
      await runChecks(cdp, captureDir);
    } catch (error) {
      report(
        "harness/error",
        false,
        `the run aborted: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  } finally {
    cleanup();
    await sleep(300);
    chrome.kill("SIGKILL");
    vite.kill("SIGKILL");
    console.log(`\ntemp dir: ${tempDir}`);
    console.log(`captures: ${captureDir}`);
  }

  console.log(
    `\n${results.length - failures}/${results.length} geometry checks passed`,
  );
  if (failures > 0) process.exitCode = 1;
}

async function capture(cdp: Cdp, path: string): Promise<void> {
  const shot = (await cdp.send("Page.captureScreenshot", {
    format: "png",
  })) as { data: string };
  await Bun.write(path, Buffer.from(shot.data, "base64"));
}

async function runChecks(cdp: Cdp, captureDir: string): Promise<void> {
  // ------------------------------------------------------------ 1. shell
  await setViewport(cdp, 1150, 680);
  const expandedMain = await rectOf(cdp, "[data-testid='main']");
  const expandedComposer = await rectOf(cdp, ".composer");
  report(
    "shell/initial",
    Boolean(
      expandedMain &&
        expandedComposer &&
        expandedMain.width > 600 &&
        expandedMain.height >= 600 &&
        expandedComposer.bottom <= 680,
    ),
    `main=${JSON.stringify(expandedMain)} composer.bottom=${expandedComposer?.bottom}`,
  );

  // ------------------------------------------------- 2. 20 toggle cycles
  const before = await cdp.evaluate<{ scrollTop: number; draft: string }>(
    `(() => {
      const thread = document.querySelector(".thread");
      if (thread) thread.scrollTop = 120;
      const input = document.querySelector("textarea.composer__input");
      if (input) {
        const setter = Object.getOwnPropertyDescriptor(
          window.HTMLTextAreaElement.prototype, "value").set;
        setter.call(input, "draft that must survive the toggles");
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
      return { scrollTop: thread ? thread.scrollTop : -1,
               draft: input ? input.value : "" };
    })()`,
  );
  await sleep(150);

  let minWidth = Number.POSITIVE_INFINITY;
  let maxWidth = 0;
  let minHeight = Number.POSITIVE_INFINITY;
  let maxHeight = 0;
  let composerLost = 0;
  let toggleError = "";
  for (let cycle = 0; cycle < 20; cycle += 1) {
    const collapseBox = await boxOf(cdp, ".sidebar__header .btn--icon");
    if (!collapseBox) {
      toggleError = "the sidebar collapse button is not reachable";
      break;
    }
    await clickAt(cdp, collapseBox.x, collapseBox.y);
    await sleep(60);
    const collapsed = await rectOf(cdp, "[data-testid='main']");
    if (!collapsed) {
      toggleError = "the main column disappeared after collapsing";
      break;
    }
    minWidth = Math.min(minWidth, collapsed.width);
    maxWidth = Math.max(maxWidth, collapsed.width);
    minHeight = Math.min(minHeight, collapsed.height);
    maxHeight = Math.max(maxHeight, collapsed.height);
    const composer = await rectOf(cdp, ".composer");
    if (!composer || composer.bottom > 681 || composer.top < 0) composerLost += 1;

    const expandBox = await boxOf(cdp, ".main__header .btn--icon");
    if (!expandBox) {
      toggleError = "the sidebar expand button is not reachable";
      break;
    }
    await clickAt(cdp, expandBox.x, expandBox.y);
    await sleep(60);
    const restored = await rectOf(cdp, "[data-testid='main']");
    if (!restored) {
      toggleError = "the main column disappeared after expanding";
      break;
    }
    minWidth = Math.min(minWidth, restored.width);
    maxWidth = Math.max(maxWidth, restored.width);
    minHeight = Math.min(minHeight, restored.height);
    maxHeight = Math.max(maxHeight, restored.height);
    const composer2 = await rectOf(cdp, ".composer");
    if (!composer2 || composer2.bottom > 681 || composer2.top < 0) composerLost += 1;
  }

  const after = await cdp.evaluate<{ scrollTop: number; draft: string }>(
    `(() => {
      const thread = document.querySelector(".thread");
      const input = document.querySelector("textarea.composer__input");
      return { scrollTop: thread ? thread.scrollTop : -1,
               draft: input ? input.value : "" };
    })()`,
  );
  report(
    "sidebar/20-cycles",
    toggleError === "" &&
      minWidth > 600 &&
      maxHeight <= 681 &&
      minHeight >= 600 &&
      composerLost === 0 &&
      after.draft === before.draft &&
      Math.abs(after.scrollTop - before.scrollTop) <= 2,
    `main width ${minWidth.toFixed(1)}..${maxWidth.toFixed(1)} height ${minHeight.toFixed(1)}..${maxHeight.toFixed(1)} composerLost=${composerLost} draft=${after.draft === before.draft ? "kept" : "LOST"} scrollTop ${before.scrollTop}->${after.scrollTop}${toggleError ? ` ERROR: ${toggleError}` : ""}`,
  );

  await cdp.evaluate(
    `(() => {
      const input = document.querySelector("textarea.composer__input");
      if (!input) return;
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype, "value").set;
      setter.call(input, "");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`,
  );

  // ------------------------------------- 3. composer visible and editable
  for (const [width, height] of [
    [880, 520],
    [1150, 680],
    [1280, 800],
  ] as const) {
    await setViewport(cdp, width, height);
    const composer = await rectOf(cdp, ".composer");
    const input = await rectOf(cdp, "textarea.composer__input");
    const visible =
      Boolean(composer && input) &&
      composer!.bottom <= height + 1 &&
      composer!.top >= 0 &&
      input!.height > 10 &&
      input!.bottom <= height + 1;
    await clickSelector(cdp, "textarea.composer__input");
    await cdp.send("Input.insertText", { text: "typed by the harness" });
    await sleep(120);
    const typed = await cdp.evaluate<string>(
      `(() => { const el = document.querySelector("textarea.composer__input");
                return el ? el.value : ""; })()`,
    );
    const afterType = await rectOf(cdp, ".composer");
    report(
      `composer/${width}x${height}`,
      visible &&
        typed.includes("typed by the harness") &&
        Boolean(afterType) &&
        afterType!.bottom <= height + 1 &&
        afterType!.top >= 0,
      `visible=${visible} typed=${JSON.stringify(typed)} composer=${JSON.stringify(afterType)}`,
    );
    await cdp.evaluate(
      `(() => {
        const el = document.querySelector("textarea.composer__input");
        if (!el) return;
        const setter = Object.getOwnPropertyDescriptor(
          window.HTMLTextAreaElement.prototype, "value").set;
        setter.call(el, "");
        el.dispatchEvent(new Event("input", { bubbles: true }));
      })()`,
    );
  }

  // ------------------------------------------- 4. real wheel scrolls thread
  await setViewport(cdp, 1150, 680);
  await cdp.evaluate(
    `(() => { const t = document.querySelector(".thread"); if (t) t.scrollTop = 0; })()`,
  );
  await sleep(120);
  const threadBox = await rectOf(cdp, ".thread");
  if (!threadBox) throw new Error("no thread");
  const beforeWheel = await cdp.evaluate<number>(
    `document.querySelector(".thread").scrollTop`,
  );
  await wheel(
    cdp,
    threadBox.left + threadBox.width / 2,
    threadBox.top + threadBox.height / 2,
    320,
  );
  const afterWheel = await cdp.evaluate<number>(
    `document.querySelector(".thread").scrollTop`,
  );
  const pageScroll = await cdp.evaluate<number>(
    `document.scrollingElement ? document.scrollingElement.scrollTop : 0`,
  );
  report(
    "thread/wheel",
    afterWheel > beforeWheel + 20 && pageScroll === 0,
    `scrollTop ${beforeWheel} -> ${afterWheel} (page scroll ${pageScroll})`,
  );

  // ------------------------------------- 5. long question: body vs footer
  const longIndex = await cdp.evaluate<number>(`(() => {
    const list = [...document.querySelectorAll('[data-testid="question"]')];
    return list.findIndex((question) =>
      question.querySelector('[data-testid="question-body"][data-collapsed="true"]'));
  })()`);
  if (longIndex < 0) throw new Error("the fixture has no long question");
  const q = (selector: string) =>
    `document.querySelectorAll('[data-testid="question"]')[${longIndex}]` +
    `.querySelector(${JSON.stringify(selector)})`;
  const scrollQuestionIntoView = async (block: string) => {
    await cdp.evaluate(`(() => {
      const el = ${q("[data-testid='question']")};
      if (el) el.scrollIntoView({ block: ${JSON.stringify(block)}, inline: "nearest" });
    })()`);
    await sleep(150);
  };
  await scrollQuestionIntoView("center");

  const collapsedBody = await rectOfExpr(cdp, q("[data-testid='question-body']"));
  const footerBefore = await rectOfExpr(cdp, q("[data-testid='question-actions']"));
  await clickExpr(cdp, q("[data-testid='question-toggle']"));
  const expandedBody = await rectOfExpr(cdp, q("[data-testid='question-body']"));
  const footerAfter = await rectOfExpr(cdp, q("[data-testid='question-actions']"));
  const clickable = await cdp.evaluate<boolean>(`(() => {
    const button = ${q("[data-testid='question-toggle']")};
    if (!button) return false;
    const r = button.getBoundingClientRect();
    if (r.bottom > window.innerHeight || r.top < 0) return false;
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return !!hit && (hit === button || button.contains(hit));
  })()`);
  report(
    "question/expand",
    Boolean(
      collapsedBody &&
        expandedBody &&
        footerBefore &&
        footerAfter &&
        expandedBody.height > collapsedBody.height + 20 &&
        footerAfter.bottom <= 681 &&
        footerAfter.top >= 0 &&
        clickable,
    ),
    `body ${collapsedBody?.height.toFixed(1)} -> ${expandedBody?.height.toFixed(1)} footer bottom ${footerAfter?.bottom} clickable=${clickable}`,
  );
  // Collapse again: the body grew, so the button moved — re-read it.
  await scrollQuestionIntoView("start");
  await clickExpr(cdp, q("[data-testid='question-toggle']"));
  const reCollapsed = await rectOfExpr(cdp, q("[data-testid='question-body']"));
  report(
    "question/collapse",
    Boolean(
      reCollapsed &&
        collapsedBody &&
        reCollapsed.height <= collapsedBody.height + 2,
    ),
    `body ${expandedBody?.height.toFixed(1)} -> ${reCollapsed?.height.toFixed(1)}`,
  );

  // ------------------------------------------- 6. answer action placement
  await scrollIntoView(cdp, "[data-testid='answer']", 0);
  const answer = await rectOfExpr(
    cdp,
    `document.querySelectorAll("[data-testid='answer']")[0]`,
  );
  const answerActions = await rectOfExpr(
    cdp,
    `document.querySelectorAll("[data-testid='answer']")[0]` +
      `.querySelector("[data-testid='answer-actions']")`,
  );
  const answerBody = await rectOfExpr(
    cdp,
    `document.querySelectorAll("[data-testid='answer']")[0].querySelector(".md")`,
  );
  const questionBox = await rectOfExpr(
    cdp,
    `document.querySelectorAll("[data-testid='question']")[${longIndex}]`,
  );
  const questionActions = await rectOfExpr(cdp, q("[data-testid='question-actions']"));
  const questionBody = await rectOfExpr(cdp, q("[data-testid='question-body']"));
  report(
    "actions/placement",
    Boolean(
      answer &&
        answerActions &&
        answerBody &&
        questionBox &&
        questionActions &&
        questionBody &&
        // right aligned with the text block, not floating over it
        Math.abs(answerActions.right - answerBody.right) <= 2 &&
        answerActions.top >= answerBody.bottom - 1 &&
        Math.abs(questionActions.right - questionBody.right) <= 2 &&
        questionActions.top >= questionBody.bottom - 1,
    ),
    `answer actions right=${answerActions?.right.toFixed(1)} (text ${answerBody?.right.toFixed(1)}) top=${answerActions?.top.toFixed(1)} (text bottom ${answerBody?.bottom.toFixed(1)}); question actions right=${questionActions?.right.toFixed(1)} (text ${questionBody?.right.toFixed(1)}) top=${questionActions?.top.toFixed(1)} (text bottom ${questionBody?.bottom.toFixed(1)})`,
  );

  // ------------------------------------------------- 7. Models/Settings popup
  for (const [label, testId] of [
    ["models", "modal-models"],
    ["settings", "modal-settings"],
  ] as const) {
    await setViewport(cdp, 880, 520);
    const mainBefore = await rectOf(cdp, "[data-testid='main']");
    const scrollBefore = await cdp.evaluate<number>(
      `document.querySelector(".thread").scrollTop`,
    );
    await clickSelector(cdp, ".main__toolbar .btn", label === "models" ? 0 : 1);
    const dialog = await rectOf(cdp, `[data-testid='${testId}-dialog']`);
    const semantics = await cdp.evaluate<{ role: string; modal: string } | null>(
      `(() => {
        const el = document.querySelector("[data-testid='${testId}-dialog']");
        if (!el) return null;
        return { role: el.getAttribute("role") || "",
                 modal: el.getAttribute("aria-modal") || "" };
      })()`,
    );
    const mainAfter = await rectOf(cdp, "[data-testid='main']");
    const scrollAfter = await cdp.evaluate<number>(
      `document.querySelector(".thread").scrollTop`,
    );
    const modalBody = await rectOf(cdp, `[data-testid='${testId}-body']`);
    const modalScrollBefore = await cdp.evaluate<number>(
      `(() => { const el = document.querySelector("[data-testid='${testId}-body']");
                return el ? el.scrollTop : -1; })()`,
    );
    const modalScrollable = await cdp.evaluate<boolean>(
      `(() => { const el = document.querySelector("[data-testid='${testId}-body']");
                if (!el) return false;
                return el.scrollHeight > el.clientHeight + 4; })()`,
    );
    if (modalBody) {
      await wheel(
        cdp,
        modalBody.left + modalBody.width / 2,
        modalBody.top + modalBody.height / 2,
        260,
      );
    }
    const modalScrollAfter = await cdp.evaluate<number>(
      `(() => { const el = document.querySelector("[data-testid='${testId}-body']");
                return el ? el.scrollTop : -1; })()`,
    );
    const scrollEnd = await cdp.evaluate<number>(
      `document.querySelector(".thread").scrollTop`,
    );
    const scrollsInside = modalScrollable
      ? modalScrollAfter > modalScrollBefore
      : modalScrollAfter === modalScrollBefore;
    report(
      `modal/${label}`,
      Boolean(
        dialog &&
          semantics &&
          semantics.role === "dialog" &&
          semantics.modal === "true" &&
          dialog.width <= 880 &&
          dialog.height <= 520 &&
          dialog.top >= 0 &&
          dialog.left >= 0 &&
          mainBefore &&
          mainAfter &&
          Math.abs(mainBefore.width - mainAfter.width) < 1 &&
          Math.abs(mainBefore.height - mainAfter.height) < 1 &&
          scrollBefore === scrollAfter &&
          scrollEnd === scrollBefore &&
          scrollsInside,
      ),
      `dialog=${JSON.stringify(dialog)} semantics=${JSON.stringify(semantics)} main ${mainBefore?.width.toFixed(1)}x${mainBefore?.height.toFixed(1)} -> ${mainAfter?.width.toFixed(1)}x${mainAfter?.height.toFixed(1)} thread scroll ${scrollBefore} -> ${scrollEnd} modal body scroll ${modalScrollBefore} -> ${modalScrollAfter} (scrollable=${modalScrollable})`,
    );

    await capture(cdp, join(captureDir, `modal-${label}-880x520.png`));

    await pressEscape(cdp);
    const closed = await cdp.evaluate<boolean>(
      `!document.querySelector("[data-testid='${testId}-dialog']")`,
    );
    const focusBack = await cdp.evaluate<string>(
      `(() => { const el = document.activeElement;
                return el ? (el.getAttribute("aria-haspopup") || el.tagName) : "none"; })()`,
    );
    report(
      `modal/${label}/escape`,
      closed && focusBack === "dialog",
      `closed=${closed} focus=${focusBack}`,
    );
  }

  // close button as well
  await setViewport(cdp, 1150, 680);
  await clickSelector(cdp, ".main__toolbar .btn", 0);
  await clickSelector(cdp, "[data-testid='modal-close']");
  const closedByButton = await cdp.evaluate<boolean>(
    `!document.querySelector("[data-testid='modal-models-dialog']")`,
  );
  report("modal/close-button", closedByButton, `closed=${closedByButton}`);

  // ------------------------------- 7b. focus trap, selection and composer
  await scrollIntoView(cdp, "[data-testid='answer']", 0);
  const selected = await cdp.evaluate<string>(`(() => {
    const md = document.querySelector("[data-testid='answer'] .md");
    if (!md) return "";
    const target = md.querySelector("p") || md;
    const range = document.createRange();
    range.selectNodeContents(target);
    const sel = window.getSelection();
    if (!sel) return "";
    sel.removeAllRanges();
    sel.addRange(range);
    document.dispatchEvent(new Event("selectionchange"));
    return sel.toString();
  })()`);
  await sleep(200);
  await cdp.evaluate(`(() => {
    const input = document.querySelector("textarea.composer__input");
    if (!input) return;
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLTextAreaElement.prototype, "value").set;
    setter.call(input, "draft while the dialog is open");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);

  await clickSelector(cdp, ".main__toolbar .btn", 0);
  const selectionAfterOpen = await cdp.evaluate<string>(
    `window.getSelection() ? window.getSelection().toString() : ""`,
  );
  for (let step = 0; step < 14; step += 1) {
    await pressKey(cdp, "Tab", 9);
  }
  const focusInside = await cdp.evaluate<boolean>(`(() => {
    const dialog = document.querySelector("[data-testid='modal-models-dialog']");
    if (!dialog) return false;
    return dialog.contains(document.activeElement);
  })()`);
  for (let step = 0; step < 6; step += 1) {
    await pressKey(cdp, "Tab", 9, 8); // Shift+Tab
  }
  const focusInsideBack = await cdp.evaluate<boolean>(`(() => {
    const dialog = document.querySelector("[data-testid='modal-models-dialog']");
    if (!dialog) return false;
    return dialog.contains(document.activeElement);
  })()`);
  const composerKept = await cdp.evaluate<string>(
    `(() => { const el = document.querySelector("textarea.composer__input");
              return el ? el.value : ""; })()`,
  );
  report(
    "modal/focus-trap",
    focusInside && focusInsideBack,
    `tab=${focusInside} shift-tab=${focusInsideBack}`,
  );
  report(
    "modal/preserves-chat",
    selectionAfterOpen === selected &&
      composerKept.includes("draft while the dialog is open"),
    `selection ${selected.length} -> ${selectionAfterOpen.length} (open), composer ${JSON.stringify(composerKept)} (the focus walk may collapse the selection, the dialog restores it on close)`,
  );
  await pressEscape(cdp);
  const selectionAfterClose = await cdp.evaluate<string>(
    `window.getSelection() ? window.getSelection().toString() : ""`,
  );
  report(
    "modal/selection-after-close",
    selectionAfterClose.length > 0,
    `selection after close: ${selectionAfterClose.length} chars`,
  );
  await cdp.evaluate(`(() => {
    const input = document.querySelector("textarea.composer__input");
    if (!input) return;
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLTextAreaElement.prototype, "value").set;
    setter.call(input, "");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);

  // ---------------------------------- 8. single model vs parallel controls
  const singleTabs = await cdp.evaluate<number>(
    `document.querySelectorAll("[data-testid='answer-tabs']").length`,
  );
  const singleBranch = await cdp.evaluate<number>(
    `document.querySelectorAll("[data-testid='answer-branch-select']").length`,
  );
  report(
    "controls/single-model",
    singleTabs === 0 && singleBranch === 0,
    `tabs=${singleTabs} branch=${singleBranch}`,
  );

  const switched = await cdp.evaluate<boolean>(`(() => {
    const items = [...document.querySelectorAll(".chat-item")];
    const target = items.find((item) =>
      item.textContent.includes("Parallel model comparison"));
    if (!target) return false;
    target.click();
    return true;
  })()`);
  await sleep(600);
  const parallelTabs = await cdp.evaluate<number>(
    `document.querySelectorAll("[data-testid='answer-tabs']").length`,
  );
  const parallelBranch = await cdp.evaluate<number>(
    `document.querySelectorAll("[data-testid='answer-branch-select']").length`,
  );
  report(
    "controls/parallel-model",
    switched && parallelTabs > 0 && parallelBranch > 0,
    `switched=${switched} tabs=${parallelTabs} branch=${parallelBranch}`,
  );

  await capture(cdp, join(captureDir, "parallel-conversation.png"));

  // back to the single conversation for the final capture
  await cdp.evaluate(`(() => {
    const items = [...document.querySelectorAll(".chat-item")];
    const target = items.find((item) =>
      item.textContent.includes("Imported research chat"));
    target?.click();
  })()`);
  await sleep(500);
  await capture(cdp, join(captureDir, "single-conversation.png"));
}

await main();
