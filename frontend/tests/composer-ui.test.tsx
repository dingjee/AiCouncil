import { describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { StrictMode } from "react";

import { App } from "../src/App";
import { IpcClient } from "../src/ipc/client";
import type { Transport } from "../src/ipc/transport";
import { StoreProvider } from "../src/state/store";
import {
  fixtureBootstrap,
  fixtureConversation,
  fixtureDetail,
  fixtureDraft,
} from "./ipc-contract.test";

/**
 * Component level tests.
 *
 * They drive the real DOM: the composer must not send while an IME
 * composition is active, and the chat surface must expose the accessible
 * structure the native app has.
 */

class UiBackend implements Transport {
  readonly calls: { command: string; args: Record<string, any> }[] = [];
  readonly listeners = new Map<string, Set<(payload: unknown) => void>>();

  constructor(
    private readonly conversations = [
      fixtureConversation("c1", "First chat"),
      fixtureConversation("c2", "Second chat"),
    ],
  ) {}

  async invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
    this.calls.push({ command, args });
    switch (command) {
      case "bootstrap":
        return fixtureBootstrap(this.conversations) as T;
      case "load_conversation": {
        const id = String(args.conversation_id);
        const title =
          this.conversations.find((entry) => entry.id === id)?.title ?? "Chat";
        return fixtureDetail(
          id,
          [
            {
              id: "q1",
              conversation_id: id,
              parent_id: null,
              role: "user",
              content: "Question?",
              attachments: [],
              created_at_ms: 0,
            },
            {
              id: "a1",
              conversation_id: id,
              parent_id: "q1",
              role: "assistant",
              content: "Answer body",
              attachments: [],
              created_at_ms: 1,
            },
          ],
          title,
        ) as T;
      }
      case "get_draft":
        return fixtureDraft("") as T;
      case "update_settings":
        return {
          max_concurrent_requests: 4,
          assumed_output_tokens: 1000,
          connect_timeout_secs: 15,
          read_idle_timeout_secs: 120,
          body_text_size: 15,
          locale: "en",
          theme_id: "soft_sage",
          sidebar_width: Number(args.sidebar_width ?? 300),
          sidebar_collapsed: false,
        } as T;
      case "check_egress":
        return { verdict: "allowed", country_code: "US", detail: "" } as T;
      case "list_preferences":
        return [
          {
            provider_id: "p1",
            model_id: "vendor/alpha",
            enabled: true,
            sort_order: 0,
            is_default: true,
            overrides: null,
          },
        ] as T;
      default:
        return undefined as T;
    }
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    const set = this.listeners.get(event) ?? new Set();
    set.add(handler as (payload: unknown) => void);
    this.listeners.set(event, set);
    return () => set.delete(handler as (payload: unknown) => void);
  }
}

async function mountApp(backend: UiBackend) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root: Root = createRoot(container);
  await act(async () => {
    root.render(
      <StrictMode>
        <StoreProvider client={new IpcClient(backend)}>
          <App />
        </StoreProvider>
      </StrictMode>,
    );
  });
  await act(async () => {
    await Promise.resolve();
  });
  return { root, container };
}

function textarea(container: HTMLElement): HTMLTextAreaElement {
  const element = container.querySelector("textarea.composer__input");
  if (!element) throw new Error("composer textarea not found");
  return element as HTMLTextAreaElement;
}

function key(
  init: KeyboardEventInit & { isComposing?: boolean },
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key: "Enter",
    bubbles: true,
    cancelable: true,
    ...init,
  });
  if (init.isComposing !== undefined) {
    Object.defineProperty(event, "isComposing", {
      value: init.isComposing,
      configurable: true,
    });
  }
  return event;
}

describe("composer IME safety", () => {
  test("Enter during an IME composition does not send", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);
    const field = textarea(container);

    await act(async () => {
      field.dispatchEvent(key({ isComposing: true }));
    });

    expect(backend.calls.filter((call) => call.command === "send_message")).toHaveLength(0);

    await act(async () => {
      root.unmount();
    });
  });

  test("Enter outside a composition sends", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);
    const field = textarea(container);

    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(field, "hello");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });

    await act(async () => {
      field.dispatchEvent(key({}));
    });

    const sends = backend.calls.filter((call) => call.command === "send_message");
    expect(sends).toHaveLength(1);
    expect(sends[0].args.request.mode).toBe("normal");
    expect(sends[0].args.request.prompt).toBe("hello");

    await act(async () => {
      root.unmount();
    });
  });

  test("Shift+Enter inserts a newline instead of sending", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);
    const field = textarea(container);

    await act(async () => {
      field.dispatchEvent(key({ shiftKey: true }));
    });

    expect(backend.calls.filter((call) => call.command === "send_message")).toHaveLength(0);

    await act(async () => {
      root.unmount();
    });
  });

  test("the composition flag from the browser is honoured", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);
    const field = textarea(container);

    await act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
      );
    });
    expect(backend.calls.filter((call) => call.command === "send_message")).toHaveLength(0);

    await act(async () => {
      root.unmount();
    });
  });
});

describe("accessible structure", () => {
  test("renders the chat list, the thread and the composer", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);

    expect(container.querySelector("aside.sidebar")).not.toBeNull();
    expect(container.querySelector("main.main")).not.toBeNull();
    expect(container.querySelector("textarea.composer__input")).not.toBeNull();
    expect(container.textContent).toContain("First chat");
    expect(container.textContent).toContain("Second chat");
    expect(container.textContent).toContain("Answer body");

    // The message region is a live log for screen readers.
    const log = container.querySelector('[role="log"]');
    expect(log?.getAttribute("aria-live")).toBe("polite");

    // The composer is labelled.
    const composer = container.querySelector("textarea.composer__input");
    expect(composer?.getAttribute("aria-label")).toBeTruthy();

    await act(async () => {
      root.unmount();
    });
  });

  test("the sidebar search filters the chat list", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);

    const search = container.querySelector<HTMLInputElement>(
      'input[type="search"]',
    );
    if (!search) throw new Error("search input not found");

    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype,
        "value",
      )?.set;
      setter?.call(search, "Second");
      search.dispatchEvent(new Event("input", { bubbles: true }));
    });

    const sidebar = container.querySelector("aside.sidebar");
    expect(sidebar?.textContent).toContain("Second chat");
    expect(sidebar?.textContent).not.toContain("First chat");

    await act(async () => {
      root.unmount();
    });
  });
});

describe("no secret persistence in the browser", () => {
  test("the app never writes to localStorage or sessionStorage", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);

    const field = textarea(container);
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(field, "a secret answer");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });

    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);

    await act(async () => {
      root.unmount();
    });
  });
});

describe("sidebar resizing", () => {
  test("the divider is exposed as an accessible separator", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);

    const divider = container.querySelector('[role="separator"]');
    expect(divider).not.toBeNull();
    expect(divider?.getAttribute("aria-orientation")).toBe("vertical");
    expect(divider?.getAttribute("aria-valuenow")).toBe("300");

    await act(async () => {
      root.unmount();
    });
  });

  test("the arrow keys resize the sidebar and persist the width", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);
    const divider = container.querySelector('[role="separator"]');
    if (!divider) throw new Error("divider not found");

    await act(async () => {
      divider.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
      );
    });

    const update = backend.calls.find((call) => call.command === "update_settings");
    // `update_settings` takes one struct parameter named `patch` → nested.
    expect(update?.args.patch?.sidebar_width).toBe(316);

    await act(async () => {
      root.unmount();
    });
  });
});

describe("export menu", () => {
  test("offers the selected branches only when answers are checked", async () => {
    const backend = new UiBackend();
    const { root, container } = await mountApp(backend);

    const exportButton = [
      ...container.querySelectorAll<HTMLButtonElement>(".composer__box button"),
    ].find((button) => button.getAttribute("aria-haspopup") === "menu");
    if (!exportButton) throw new Error("export button not found");

    await act(async () => {
      exportButton.click();
    });
    expect(container.textContent).toContain("Main branch");
    expect(container.textContent).toContain("All branches");
    expect(container.textContent).not.toContain("Selected branches");

    await act(async () => {
      root.unmount();
    });
  });
});
