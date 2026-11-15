import { describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { StrictMode } from "react";

import { App } from "../src/App";
import { IpcClient } from "../src/ipc/client";
import type { Generation, Message } from "../src/ipc/types";
import type { Transport } from "../src/ipc/transport";
import { StoreProvider } from "../src/state/store";
import {
  fixtureBootstrap,
  fixtureConversation,
  fixtureDraft,
} from "./ipc-contract.test";

/**
 * Structure of the repaired surface.
 *
 * These tests assert the **structure** (which element exists, which
 * attribute it has, in which order). The geometry — is the composer on
 * screen, does the thread really scroll — is verified in a real browser by
 * `dev/geometry.ts`; happy-dom has no layout engine and must not be used
 * to claim a pixel.
 */

const LONG_QUESTION = "L".repeat(600);

function question(id: string, content: string): Message {
  return {
    id,
    conversation_id: "c1",
    parent_id: null,
    role: "user",
    content,
    attachments: [],
    created_at_ms: 1,
  };
}

function answer(id: string, parentId: string, content: string): Message {
  return {
    id,
    conversation_id: "c1",
    parent_id: parentId,
    role: "assistant",
    content,
    attachments: [],
    created_at_ms: 2,
  };
}

function generation(
  id: string,
  assistantId: string,
  modelId: string,
  options: { retryRootId?: string | null } = {},
): Generation {
  return {
    request_id: id,
    batch_id: "b1",
    user_message_id: "q1",
    assistant_message_id: assistantId,
    provider_id: "p1",
    model_id: modelId,
    status: "completed",
    system_prompt: null,
    price_prompt: null,
    price_completion: null,
    remote_generation_id: null,
    error: null,
    retry_of: null,
    retry_root_id: options.retryRootId ?? null,
    retry_not_before_ms: null,
    dispatch_phase: "remote_accepted",
    failure_kind: null,
    failure_message: null,
    started_at_ms: 1,
    finished_at_ms: 2,
  };
}

interface Scenario {
  messages: Message[];
  generations: Generation[];
}

const SINGLE_MODEL: Scenario = {
  messages: [question("q1", "Question?"), answer("a1", "q1", "Answer body")],
  generations: [generation("g1", "a1", "vendor/alpha")],
};

const PARALLEL_MODELS: Scenario = {
  messages: [
    question("q1", "Question?"),
    answer("a1", "q1", "Answer from alpha"),
    answer("a2", "q1", "Answer from beta"),
  ],
  generations: [
    generation("g1", "a1", "vendor/alpha"),
    generation("g2", "a2", "vendor/beta"),
  ],
};

const RETRY_SAME_MODEL: Scenario = {
  messages: [
    question("q1", "Question?"),
    answer("a1", "q1", "First attempt"),
    answer("a2", "q1", "Second attempt"),
  ],
  generations: [
    generation("g1", "a1", "vendor/alpha", { retryRootId: "g1" }),
    generation("g2", "a2", "vendor/alpha", { retryRootId: "g1" }),
  ],
};

class ThreadBackend implements Transport {
  readonly calls: { command: string; args: Record<string, any> }[] = [];
  readonly listeners = new Map<string, Set<(payload: unknown) => void>>();

  constructor(private readonly scenario: Scenario) {}

  async invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
    this.calls.push({ command, args });
    switch (command) {
      case "bootstrap":
        return fixtureBootstrap([
          fixtureConversation("c1", "First chat", "a1"),
        ]) as T;
      case "load_conversation":
        return {
          conversation: fixtureConversation("c1", "First chat", "a1"),
          messages: this.scenario.messages,
          generations: this.scenario.generations,
          usage: [],
          edit_latest: null,
        } as T;
      case "get_draft":
        return fixtureDraft("") as T;
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

async function mountApp(scenario: Scenario = SINGLE_MODEL) {
  const backend = new ThreadBackend(scenario);
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
  return { root, container, backend };
}

function unmount(root: Root): Promise<void> {
  return act(async () => {
    root.unmount();
  });
}

describe("the shell has no right-hand panel", () => {
  test("the models and settings panels are popups, not layout columns", async () => {
    const { root, container } = await mountApp();

    // No side column exists at all.
    expect(container.querySelector(".right-panel")).toBeNull();
    expect(container.querySelector("aside.right-panel")).toBeNull();

    // The chat is the only column of the shell.
    const main = container.querySelector("main.main");
    expect(main).not.toBeNull();
    expect(main?.parentElement?.classList.contains("app")).toBe(true);

    await unmount(root);
  });

  test("the two entries share the bottom left corner of the sidebar", async () => {
    const { root, container } = await mountApp();

    const footer = container.querySelector(".sidebar__footer");
    expect(footer).not.toBeNull();
    const buttons = [...(footer?.querySelectorAll("button") ?? [])];
    expect(buttons.length).toBe(2);
    expect(buttons[0].textContent).toContain("Models");
    expect(buttons[1].textContent).toContain("Settings");
    // Both of them are direct children of the same row.
    expect(buttons[0].parentElement).toBe(buttons[1].parentElement);

    // The header carries no second entry any more.
    const toolbar = container.querySelector(".main__toolbar");
    expect(toolbar?.textContent).not.toContain("Models");
    expect(toolbar?.textContent).not.toContain("Settings");

    await unmount(root);
  });

  test("the toolbar shows the cost and the export, the composer does not", async () => {
    const { root, container } = await mountApp();

    // The top right toolbar: the amount, then the export.
    const toolbar = container.querySelector(".main__toolbar");
    expect(toolbar?.querySelector("[data-testid='toolbar-cost']")).not.toBeNull();
    expect(toolbar?.querySelector("[data-testid='toolbar-export']")).not.toBeNull();

    // The composer does not repeat the export, the usage or the cost.
    const composer = container.querySelector(".composer");
    expect(composer?.querySelector("[data-testid='toolbar-export']")).toBeNull();
    expect(composer?.textContent).not.toContain("Usage");
    expect(composer?.querySelector(".composer__status")?.textContent).not.toContain(
      "$",
    );

    await unmount(root);
  });

  test("the models button opens an accessible dialog outside the chat column", async () => {
    const { root, container } = await mountApp();

    // The entry lives in the bottom left corner of the sidebar.
    const trigger = [...container.querySelectorAll<HTMLButtonElement>(
      ".sidebar__footer button",
    )].find((button) => button.textContent?.includes("Models"));
    if (!trigger) throw new Error("models button not found");

    await act(async () => {
      trigger.focus();
      trigger.click();
    });

    const dialog = document.body.querySelector<HTMLElement>(
      "[data-testid='modal-models-dialog']",
    );
    expect(dialog).not.toBeNull();
    expect(dialog?.getAttribute("role")).toBe("dialog");
    expect(dialog?.getAttribute("aria-modal")).toBe("true");

    // The dialog has an accessible name.
    const labelledBy = dialog?.getAttribute("aria-labelledby");
    expect(labelledBy).toBeTruthy();
    const label = document.getElementById(labelledBy ?? "");
    expect(label?.textContent?.trim().length).toBeGreaterThan(0);

    // The dialog lives outside the app column: it cannot resize the chat.
    expect(container.querySelector("[data-testid='modal-models-dialog']")).toBeNull();

    // It has a close button.
    expect(
      dialog?.querySelector("[data-testid='modal-close']"),
    ).not.toBeNull();

    await unmount(root);
  });

  test("Escape closes the dialog and the focus returns to the trigger", async () => {
    const { root, container } = await mountApp();

    const trigger = [...container.querySelectorAll<HTMLButtonElement>(
      ".sidebar__footer button",
    )].find((button) => button.textContent?.includes("Models"));
    if (!trigger) throw new Error("models button not found");

    await act(async () => {
      trigger.focus();
      trigger.click();
    });

    const dialog = document.body.querySelector<HTMLElement>(
      "[data-testid='modal-models-dialog']",
    );
    if (!dialog) throw new Error("dialog not open");

    await act(async () => {
      dialog.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });

    expect(
      document.body.querySelector("[data-testid='modal-models-dialog']"),
    ).toBeNull();
    expect(document.activeElement).toBe(trigger);

    await unmount(root);
  });

  test("the settings popup behaves like the models popup", async () => {
    const { root, container } = await mountApp();

    const trigger = [...container.querySelectorAll<HTMLButtonElement>(
      ".sidebar__footer button",
    )].find((button) => button.textContent?.includes("Settings"));
    if (!trigger) throw new Error("settings button not found");

    await act(async () => {
      trigger.click();
    });
    const dialog = document.body.querySelector<HTMLElement>(
      "[data-testid='modal-settings-dialog']",
    );
    expect(dialog?.getAttribute("role")).toBe("dialog");
    expect(dialog?.getAttribute("aria-modal")).toBe("true");
    // The body of the dialog scrolls on its own.
    expect(dialog?.querySelector("[data-testid='modal-settings-body']")).not.toBeNull();

    await act(async () => {
      dialog
        ?.querySelector<HTMLButtonElement>("[data-testid='modal-close']")
        ?.click();
    });
    expect(
      document.body.querySelector("[data-testid='modal-settings-dialog']"),
    ).toBeNull();

    await unmount(root);
  });
  test("the settings popup names the real credential paths", async () => {
    const { root, container } = await mountApp();

    const trigger = [...container.querySelectorAll<HTMLButtonElement>(
      ".sidebar__footer button",
    )].find((button) => button.textContent?.includes("Settings"));
    if (!trigger) throw new Error("settings button not found");

    await act(async () => {
      trigger.click();
    });
    const dialog = document.body.querySelector<HTMLElement>(
      "[data-testid='modal-settings-dialog']",
    );
    const text = dialog?.textContent ?? "";

    // The runtime reads the stable Application Support file; the help text
    // must name that path, the real override variable and the first import.
    expect(text).toContain("~/Library/Application Support/AiCouncil/.env");
    expect(text).toContain("AICOUNCIL_ENV_PATH");
    expect(text).toContain("data/.env");
    expect(text).toContain("Reload credentials");

    // Paths and variables that do not exist must never be advertised.
    expect(text).not.toContain("~/.aicouncil/.env");
    expect(text).not.toContain("AICOUNCIL_ENV_FILE");

    await unmount(root);
  });
});

describe("the question body collapses, the footer does not", () => {
  test("a long question keeps its controls outside the collapsed body", async () => {
    const { root, container } = await mountApp({
      messages: [question("q1", LONG_QUESTION), answer("a1", "q1", "Answer")],
      generations: [generation("g1", "a1", "vendor/alpha")],
    });

    const body = container.querySelector<HTMLElement>(
      "[data-testid='question-body']",
    );
    const footer = container.querySelector<HTMLElement>(
      "[data-testid='question-actions']",
    );
    expect(body?.getAttribute("data-collapsed")).toBe("true");
    expect(footer).not.toBeNull();
    // The footer is a sibling of the body, never inside the clipped box.
    expect(body && footer ? body.contains(footer) : false).toBe(false);
    expect(footer?.querySelector("[data-testid='question-toggle']")).not.toBeNull();

    await act(async () => {
      footer
        ?.querySelector<HTMLButtonElement>("[data-testid='question-toggle']")
        ?.click();
    });

    expect(
      container.querySelector("[data-testid='question-body']")?.getAttribute("data-collapsed"),
    ).toBe("false");
    // The footer survives the expansion.
    expect(
      container.querySelector("[data-testid='question-actions']"),
    ).not.toBeNull();

    await unmount(root);
  });
});

describe("the answer actions follow the text", () => {
  test("the actions live in a footer below the answer", async () => {
    const { root, container } = await mountApp();

    const card = container.querySelector<HTMLElement>("[data-testid='answer']");
    const footer = container.querySelector<HTMLElement>(
      "[data-testid='answer-actions']",
    );
    const body = container.querySelector<HTMLElement>("[data-testid='answer'] .md");
    expect(card).not.toBeNull();
    expect(footer).not.toBeNull();
    // The footer comes after the text, so it can never cover it.
    expect(
      body && footer
        ? body.compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING
        : 0,
    ).toBeTruthy();
    expect(footer?.querySelector("[data-testid='answer-copy']")).not.toBeNull();

    await unmount(root);
  });
});

describe("single-model conversations stay simple", () => {
  test("no answer tab strip and no branch controls for one model", async () => {
    const { root, container } = await mountApp(SINGLE_MODEL);

    expect(container.querySelector("[data-testid='answer-tabs']")).toBeNull();
    expect(
      container.querySelector("[data-testid='answer-branch-select']"),
    ).toBeNull();
    expect(
      container.querySelector("[data-testid='answer-branch-continue']"),
    ).toBeNull();

    await unmount(root);
  });

  test("a retry of the same model is not a comparison", async () => {
    const { root, container } = await mountApp(RETRY_SAME_MODEL);

    expect(container.querySelector("[data-testid='answer-tabs']")).toBeNull();
    expect(
      container.querySelector("[data-testid='answer-branch-select']"),
    ).toBeNull();
    // The older attempt stays reachable through its own control.
    expect(container.querySelector("[data-testid='answer-attempts']")).not.toBeNull();

    await unmount(root);
  });

  test("a parallel conversation keeps the tab strip and the branch controls", async () => {
    const { root, container } = await mountApp(PARALLEL_MODELS);

    expect(container.querySelector("[data-testid='answer-tabs']")).not.toBeNull();
    expect(
      container.querySelector("[data-testid='answer-branch-select']"),
    ).not.toBeNull();
    expect(
      container.querySelector("[data-testid='answer-branch-continue']"),
    ).not.toBeNull();

    await unmount(root);
  });
});
