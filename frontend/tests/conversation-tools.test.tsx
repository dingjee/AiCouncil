import { describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { StrictMode } from "react";

import { App } from "../src/App";
import { IpcClient } from "../src/ipc/client";
import type { Transport } from "../src/ipc/transport";
import type {
  BootstrapState,
  ChatEvent,
  UsageRecord,
} from "../src/ipc/types";
import { StoreProvider, useStore, type StoreValue } from "../src/state/store";
import { fixtureConversation, fixtureDetail, fixtureDraft } from "./ipc-contract.test";

/**
 * The top right toolbar.
 *
 * It shows the cost of the **conversation** (the exact sum of the known
 * costs of every usage record of that chat — all branches, all retries) and
 * the export entry.
 */

function usage(
  requestId: string,
  cost: string | null,
  status: UsageRecord["status"],
  prompt = 100,
  completion = 50,
): UsageRecord {
  return {
    request_id: requestId,
    prompt_tokens: prompt,
    completion_tokens: completion,
    total_tokens: prompt + completion,
    cost_text: cost,
    cost_source: cost === null ? null : "api",
    status,
  };
}

class ToolsBackend implements Transport {
  readonly calls: { command: string; args: Record<string, any> }[] = [];
  readonly listeners = new Map<string, Set<(payload: unknown) => void>>();
  /** The ledger of every conversation. */
  readonly ledgers = new Map<string, UsageRecord[]>();
  /** The conversations of the fixture. */
  readonly conversations = [
    fixtureConversation("c1", "First chat"),
    fixtureConversation("c2", "Second chat"),
  ];

  constructor() {
    this.ledgers.set("c1", []);
    this.ledgers.set("c2", []);
  }

  async invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
    this.calls.push({ command, args });
    switch (command) {
      case "bootstrap":
        return this.bootstrap() as T;
      case "load_conversation": {
        const id = String(args.conversation_id);
        const detail = fixtureDetail(id, []);
        return {
          ...detail,
          usage: this.ledgers.get(id) ?? [],
        } as T;
      }
      case "get_draft":
        return fixtureDraft("") as T;
      case "save_draft":
        return { revision: 1 } as T;
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
      case "export_markdown_to_file":
        return { path: "/tmp/chat.md", sections: 1 } as T;
      case "check_egress":
        return { verdict: "allowed", country_code: "US", detail: "" } as T;
      default:
        return undefined as T;
    }
  }

  private bootstrap(): BootstrapState {
    return {
      app_version: "0.2.1",
      settings: {
        max_concurrent_requests: 4,
        assumed_output_tokens: 1000,
        connect_timeout_secs: 15,
        read_idle_timeout_secs: 120,
        body_text_size: 15,
        locale: "en",
        theme_id: "soft_sage",
        sidebar_width: 300,
        sidebar_collapsed: false,
      },
      conversations: this.conversations,
      groups: [],
      pending_new_conversation: null,
      catalog: { entries: [], updated_at_ms: 1 },
      preferences: [],
      credentials: { openrouter: true },
      egress: { verdict: "allowed", country_code: "US", detail: "" },
    };
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    const set = this.listeners.get(event) ?? new Set();
    set.add(handler as (payload: unknown) => void);
    this.listeners.set(event, set);
    return () => set.delete(handler as (payload: unknown) => void);
  }

  emitChat(event: ChatEvent): void {
    this.listeners.get("chat-event")?.forEach((handler) => handler(event));
  }
}

let store: StoreValue | null = null;

function Probe() {
  store = useStore();
  return null;
}

async function mount(backend: ToolsBackend) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root: Root = createRoot(container);
  await act(async () => {
    root.render(
      <StrictMode>
        <StoreProvider client={new IpcClient(backend)}>
          <App />
          <Probe />
        </StoreProvider>
      </StrictMode>,
    );
  });
  await act(async () => {
    await Promise.resolve();
  });
  return {
    root,
    container,
    get store() {
      if (!store) throw new Error("the store is not mounted");
      return store;
    },
  };
}

function unmount(root: Root): Promise<void> {
  return act(async () => {
    root.unmount();
  });
}

function costLabel(container: HTMLElement): string {
  const button = container.querySelector<HTMLElement>(
    "[data-testid='toolbar-cost'] .toolbar__cost",
  );
  return button?.textContent?.trim() ?? "";
}

describe("the toolbar shows the cost of the conversation", () => {
  test("the known costs of every record are summed exactly", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [
      usage("r1", "0.0108", "known"),
      usage("r2", "0.0045", "known"),
    ]);
    const mounted = await mount(backend);

    // 0.0108 + 0.0045 = 0.0153 → 0.02 (the ledger rounds half away from zero)
    expect(costLabel(mounted.container)).toBe("$0.02");

    await unmount(mounted.root);
  });

  test("the records of every branch and retry are counted", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [
      usage("r1", "0.0100", "known"),
      usage("r2", "0.0200", "known"),
      usage("r3", "0.0300", "known"),
    ]);
    const mounted = await mount(backend);

    expect(costLabel(mounted.container)).toBe("$0.06");

    await unmount(mounted.root);
  });

  test("the amount belongs to the open conversation", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [usage("r1", "1.00", "known")]);
    backend.ledgers.set("c2", [usage("r2", "2.00", "known")]);
    const mounted = await mount(backend);

    expect(costLabel(mounted.container)).toBe("$1.00");

    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });

    expect(costLabel(mounted.container)).toBe("$2.00");

    await unmount(mounted.root);
  });

  test("unsettled costs are marked and never shown as zero", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [
      usage("r1", "0.0200", "known"),
      usage("r2", null, "pending"),
    ]);
    const mounted = await mount(backend);

    expect(costLabel(mounted.container)).toBe("$0.02+");

    await unmount(mounted.root);
  });

  test("a chat without any known cost does not claim to be free", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [
      usage("r1", null, "unavailable"),
      usage("r2", null, "pending"),
    ]);
    const mounted = await mount(backend);

    expect(costLabel(mounted.container)).toBe("$?");

    await unmount(mounted.root);
  });

  test("a chat without any usage record is free", async () => {
    const backend = new ToolsBackend();
    const mounted = await mount(backend);

    expect(costLabel(mounted.container)).toBe("$0.00");

    await unmount(mounted.root);
  });

  test("a chat whose cost is zero is free", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [usage("r1", "0.0000", "known")]);
    const mounted = await mount(backend);

    expect(costLabel(mounted.container)).toBe("$0.00");

    await unmount(mounted.root);
  });

  test("a late cost updates the amount", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [usage("r1", null, "pending")]);
    const mounted = await mount(backend);

    expect(costLabel(mounted.container)).toBe("$?");

    // The provider reported the cost late.
    backend.ledgers.set("c1", [usage("r1", "0.0700", "known")]);
    await act(async () => {
      backend.emitChat({
        conversation_id: "c1",
        batch_id: "b1",
        request_id: "r1",
        assistant_message_id: "a1",
        event: {
          type: "usage_updated",
          prompt_tokens: 100,
          completion_tokens: 50,
          cost: "0.0700",
        },
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
    });

    expect(costLabel(mounted.container)).toBe("$0.07");

    await unmount(mounted.root);
  });

  test("the details keep the pending, the unavailable and the tokens", async () => {
    const backend = new ToolsBackend();
    backend.ledgers.set("c1", [
      usage("r1", "0.0200", "known", 1200, 480),
      usage("r2", null, "pending"),
      usage("r3", null, "unavailable"),
    ]);
    const mounted = await mount(backend);

    const button = mounted.container.querySelector<HTMLButtonElement>(
      "[data-testid='toolbar-cost']",
    );
    await act(async () => {
      button?.click();
    });

    const popover = mounted.container.querySelector<HTMLElement>(
      "[data-testid='cost-popover']",
    );
    expect(popover?.textContent).toContain("$0.02");
    expect(popover?.textContent).toContain("pending 1");
    expect(popover?.textContent).toContain("unavailable 1");
    // The tokens of every record are summed as well.
    expect(popover?.textContent).toContain("1,400");
    expect(popover?.textContent).toContain("580");

    await unmount(mounted.root);
  });
});

describe("the export entry keeps its scopes", () => {
  test("the three scopes call the export command", async () => {
    const backend = new ToolsBackend();
    const mounted = await mount(backend);

    const button = mounted.container.querySelector<HTMLButtonElement>(
      "[data-testid='toolbar-export']",
    );
    if (!button) throw new Error("the export entry not found");

    const clickMenuItem = async (label: string) => {
      await act(async () => {
        button.click();
      });
      const menu = mounted.container.querySelector<HTMLElement>(
        "[data-testid='export-menu']",
      );
      const item = [...(menu?.querySelectorAll("button") ?? [])].find(
        (candidate) => candidate.textContent?.includes(label),
      );
      if (!item) throw new Error(`the menu item "${label}" not found`);
      await act(async () => {
        item.click();
      });
    };

    await clickMenuItem("Main branch");
    await clickMenuItem("All branches");

    const exports = backend.calls.filter(
      (call) => call.command === "export_markdown_to_file",
    );
    expect(exports.length).toBe(2);
    expect(exports[0].args.scope).toBe("main");
    expect(exports[1].args.scope).toBe("all_branches");

    await unmount(mounted.root);
  });

  test("the selected scope is offered when answers are checked", async () => {
    const backend = new ToolsBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.toggleBranch("leaf-a");
    });

    const button = mounted.container.querySelector<HTMLButtonElement>(
      "[data-testid='toolbar-export']",
    );
    await act(async () => {
      button?.click();
    });
    const menu = mounted.container.querySelector<HTMLElement>(
      "[data-testid='export-menu']",
    );
    expect(menu?.textContent).toContain("Selected branches (1)");

    const item = [...(menu?.querySelectorAll("button") ?? [])].find(
      (candidate) => candidate.textContent?.includes("Selected branches"),
    );
    await act(async () => {
      item?.click();
    });

    const exports = backend.calls.filter(
      (call) => call.command === "export_markdown_to_file",
    );
    expect(exports[0].args.scope).toBe("selected");
    expect(exports[0].args.message_ids).toEqual(["leaf-a"]);

    await unmount(mounted.root);
  });

  test("the popover closes on Escape and returns the focus", async () => {
    const backend = new ToolsBackend();
    const mounted = await mount(backend);

    const button = mounted.container.querySelector<HTMLButtonElement>(
      "[data-testid='toolbar-export']",
    );
    await act(async () => {
      button?.click();
    });
    expect(
      mounted.container.querySelector("[data-testid='export-menu']"),
    ).not.toBeNull();

    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });

    expect(
      mounted.container.querySelector("[data-testid='export-menu']"),
    ).toBeNull();
    expect(document.activeElement).toBe(button);

    await unmount(mounted.root);
  });

  test("switching the chat closes the popover", async () => {
    const backend = new ToolsBackend();
    const mounted = await mount(backend);

    const button = mounted.container.querySelector<HTMLButtonElement>(
      "[data-testid='toolbar-export']",
    );
    await act(async () => {
      button?.click();
    });
    expect(
      mounted.container.querySelector("[data-testid='export-menu']"),
    ).not.toBeNull();

    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });

    expect(
      mounted.container.querySelector("[data-testid='export-menu']"),
    ).toBeNull();

    await unmount(mounted.root);
  });
});
