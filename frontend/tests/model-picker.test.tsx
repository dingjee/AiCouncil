import { describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { StrictMode } from "react";

import { App } from "../src/App";
import { IpcClient } from "../src/ipc/client";
import type { Transport } from "../src/ipc/transport";
import type {
  BootstrapState,
  CatalogEntry,
  ModelPreferenceEntry,
} from "../src/ipc/types";
import { StoreProvider, useStore, type StoreValue } from "../src/state/store";
import {
  fixtureConversation,
  fixtureDetail,
  fixtureDraft,
} from "./ipc-contract.test";

/**
 * The composer picker is a selection surface.
 *
 * It may only offer the models of "My models" (the enabled preferences):
 * the full catalogue is managed in the Models panel, never from here.
 */

const CATALOG: CatalogEntry[] = [
  {
    id: "vendor/alpha",
    provider_id: "p1",
    name: "Vendor: Alpha",
    family: null,
    context_length: null,
    prompt_price: null,
    completion_price: null,
    supports_streaming: true,
    supports_temperature: true,
  },
  {
    id: "vendor/beta",
    provider_id: "p1",
    name: "Vendor: Beta",
    family: null,
    context_length: null,
    prompt_price: null,
    completion_price: null,
    supports_streaming: true,
    supports_temperature: true,
  },
  {
    id: "vendor/gamma",
    provider_id: "p1",
    name: "Vendor: Gamma",
    family: null,
    context_length: null,
    prompt_price: null,
    completion_price: null,
    supports_streaming: true,
    supports_temperature: true,
  },
];

const PREFERENCES: ModelPreferenceEntry[] = [
  {
    provider_id: "p1",
    model_id: "vendor/alpha",
    enabled: true,
    sort_order: 0,
    is_default: true,
    overrides: null,
  },
  {
    provider_id: "p1",
    model_id: "vendor/beta",
    enabled: true,
    sort_order: 1,
    is_default: false,
    overrides: null,
  },
  {
    provider_id: "p1",
    model_id: "vendor/gamma",
    enabled: false,
    sort_order: 2,
    is_default: false,
    overrides: null,
  },
];

class PickerBackend implements Transport {
  readonly calls: { command: string; args: Record<string, any> }[] = [];
  readonly listeners = new Map<string, Set<(payload: unknown) => void>>();
  /** The preferences the backend holds (the writes are applied). */
  private stored: ModelPreferenceEntry[];

  constructor(
    readonly preferences: ModelPreferenceEntry[] = PREFERENCES,
    readonly catalog: CatalogEntry[] = CATALOG,
  ) {
    this.stored = preferences.map((entry) => ({ ...entry }));
  }

  async invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
    this.calls.push({ command, args });
    switch (command) {
      case "bootstrap":
        return this.bootstrap() as T;
      case "load_conversation":
        return fixtureDetail(String(args.conversation_id), []) as T;
      case "get_draft":
        return fixtureDraft("") as T;
      case "save_draft":
        return { revision: 1 } as T;
      case "list_preferences":
        return this.stored as T;
      case "set_model_preference": {
        // The backend upserts the preference.
        const patch = args.preference as ModelPreferenceEntry;
        const existing = this.stored.find(
          (entry) => entry.model_id === args.model_id,
        );
        if (existing) {
          this.stored = this.stored.map((entry) =>
            entry.model_id === args.model_id ? { ...entry, ...patch } : entry,
          );
        } else {
          this.stored = [
            ...this.stored,
            {
              ...patch,
              provider_id: String(args.provider_id),
              model_id: String(args.model_id),
            },
          ];
        }
        return null as T;
      }
      case "check_egress":
        return { verdict: "allowed", country_code: "US", detail: "" } as T;
      case "send_message":
        return {
          conversation_id: String(args.request?.conversation_id ?? ""),
          batch_id: "b1",
          primary_request_id: "r1",
          assistant_of: {},
          adopted_main_leaf: null,
        } as T;
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
      conversations: [fixtureConversation("c1", "First chat")],
      groups: [],
      pending_new_conversation: null,
      catalog: { entries: this.catalog, updated_at_ms: 1 },
      preferences: this.preferences,
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
}

let store: StoreValue | null = null;

function Probe() {
  store = useStore();
  return null;
}

async function mount(backend: PickerBackend) {
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

/** Open the picker through the composer button. */
async function openPicker(container: HTMLElement): Promise<void> {
  const trigger = container.querySelector<HTMLButtonElement>(
    ".composer__models button[aria-haspopup='dialog']",
  );
  if (!trigger) throw new Error("the picker button not found");
  await act(async () => {
    trigger.click();
  });
}

function pickerRows(container: HTMLElement): HTMLElement[] {
  const picker = container.querySelector<HTMLElement>(".picker");
  if (!picker) throw new Error("the picker is not open");
  return [...picker.querySelectorAll<HTMLElement>(".picker__row")];
}

describe("the picker only offers My models", () => {
  test("the disabled models of the catalogue are not listed", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const picker = mounted.container.querySelector<HTMLElement>(".picker");
    expect(picker).not.toBeNull();
    // Two enabled models …
    expect(pickerRows(mounted.container).length).toBe(2);
    expect(picker?.textContent).toContain("Alpha");
    expect(picker?.textContent).toContain("Beta");
    // … and no catalogue model that is not enabled.
    expect(picker?.textContent).not.toContain("Gamma");

    await unmount(mounted.root);
  });

  test("a model that left the catalogue is shown with its id", async () => {
    const preferences: ModelPreferenceEntry[] = [
      ...PREFERENCES,
      {
        provider_id: "p1",
        model_id: "gone/from-catalog",
        enabled: true,
        sort_order: 3,
        is_default: false,
        overrides: null,
      },
    ];
    const backend = new PickerBackend(preferences);
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const picker = mounted.container.querySelector<HTMLElement>(".picker");
    expect(picker?.textContent).toContain("gone/from-catalog");
    // The user keeps the model: it is still an enabled preference.
    expect(
      mounted.store.state.preferences.find(
        (entry) => entry.model_id === "gone/from-catalog",
      )?.enabled,
    ).toBe(true);

    await unmount(mounted.root);
  });

  test("the empty state explains itself and offers the Models panel", async () => {
    const backend = new PickerBackend([
      {
        provider_id: "p1",
        model_id: "vendor/gamma",
        enabled: false,
        sort_order: 0,
        is_default: false,
        overrides: null,
      },
    ]);
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const picker = mounted.container.querySelector<HTMLElement>(".picker");
    expect(pickerRows(mounted.container).length).toBe(0);
    expect(picker?.textContent).toContain("No models enabled yet");

    const manage = [...(picker?.querySelectorAll("button") ?? [])].find(
      (button) => button.textContent?.includes("Manage models"),
    );
    expect(manage).toBeDefined();

    await act(async () => {
      manage?.click();
    });

    // The picker closed and the Models panel opened.
    expect(mounted.container.querySelector(".picker")).toBeNull();
    expect(
      document.body.querySelector("[data-testid='modal-models-dialog']"),
    ).not.toBeNull();

    await unmount(mounted.root);
  });

  test("the search never reveals a model that is not enabled", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    await act(async () => {
      mounted.store.dispatch({ type: "picker/search", query: "gamma" });
    });

    const picker = mounted.container.querySelector<HTMLElement>(".picker");
    expect(picker?.textContent).not.toContain("Gamma");
    expect(pickerRows(mounted.container).length).toBe(0);

    await act(async () => {
      mounted.store.dispatch({ type: "picker/search", query: "alph" });
    });
    expect(pickerRows(mounted.container).length).toBe(1);

    await unmount(mounted.root);
  });
});

describe("the picker toggles exactly once", () => {
  test("a click on the checkbox toggles the model once", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const checkbox = mounted.container.querySelector<HTMLInputElement>(
      ".picker__row input[type='checkbox']",
    );
    expect(checkbox).not.toBeNull();
    // The boot picked the enabled default once.
    expect(mounted.store.state.modelSelection.checked).toEqual(["vendor/alpha"]);
    expect(checkbox?.checked).toBe(true);

    await act(async () => {
      checkbox?.click();
    });

    expect(mounted.store.state.modelSelection.checked).toEqual([]);

    await act(async () => {
      checkbox?.click();
    });
    expect(mounted.store.state.modelSelection.checked).toEqual(["vendor/alpha"]);

    await unmount(mounted.root);
  });

  test("a click on the row toggles the model once", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const row = pickerRows(mounted.container)[0];
    await act(async () => {
      row.click();
    });

    expect(mounted.store.state.modelSelection.checked).toEqual([]);

    await act(async () => {
      row.click();
    });
    expect(mounted.store.state.modelSelection.checked).toEqual(["vendor/alpha"]);

    await unmount(mounted.root);
  });

  test("the keyboard toggles the model once", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const row = pickerRows(mounted.container)[0];
    await act(async () => {
      row.dispatchEvent(
        new KeyboardEvent("keydown", { key: " ", bubbles: true }),
      );
    });

    expect(mounted.store.state.modelSelection.checked).toEqual([]);

    await unmount(mounted.root);
  });

  test("the star does not toggle the model a second time", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const star = mounted.container.querySelector<HTMLButtonElement>(
      ".picker__row button[aria-pressed]",
    );
    expect(star).not.toBeNull();

    await act(async () => {
      star?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });

    // The star only moves the primary; the selection stays untouched.
    expect(mounted.store.state.modelSelection.checked).toEqual(["vendor/alpha"]);

    await unmount(mounted.root);
  });

  test("select all only selects the enabled models", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const selectAll = [
      ...mounted.container.querySelectorAll<HTMLButtonElement>(".picker__footer button"),
    ].find((button) => button.textContent?.includes("Select all"));
    expect(selectAll).toBeDefined();

    await act(async () => {
      selectAll?.click();
    });

    expect(mounted.store.state.modelSelection.checked.sort()).toEqual([
      "vendor/alpha",
      "vendor/beta",
    ]);
    // Several models: the send asks them in parallel.
    expect(mounted.store.state.modelSelection.parallel).toBe(true);

    await unmount(mounted.root);
  });

  test("the picker never enables a model of the catalogue", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    // There is no "enabled" switch in the picker any more.
    const labels = [...mounted.container.querySelectorAll(".picker label")];
    expect(labels.length).toBe(0);

    await unmount(mounted.root);
  });
});

describe("the picker is a transient surface", () => {
  test("checking a model keeps the picker open", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const checkbox = mounted.container.querySelector<HTMLInputElement>(
      ".picker__row input[type='checkbox']",
    );
    await act(async () => {
      checkbox?.click();
    });

    expect(mounted.container.querySelector(".picker")).not.toBeNull();

    await unmount(mounted.root);
  });

  test("a press outside closes the picker", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    await act(async () => {
      document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    });

    expect(mounted.container.querySelector(".picker")).toBeNull();

    await unmount(mounted.root);
  });

  test("the focus leaving the picker closes it", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const outside = document.createElement("button");
    document.body.appendChild(outside);
    await act(async () => {
      outside.dispatchEvent(new Event("focusin", { bubbles: true }));
    });

    expect(mounted.container.querySelector(".picker")).toBeNull();
    outside.remove();

    await unmount(mounted.root);
  });

  test("the window losing the focus closes the picker", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    await act(async () => {
      window.dispatchEvent(new Event("blur"));
    });

    expect(mounted.container.querySelector(".picker")).toBeNull();

    await unmount(mounted.root);
  });

  test("Escape closes the picker and gives the focus back to the trigger", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const trigger = mounted.container.querySelector<HTMLButtonElement>(
      ".composer__models button[aria-haspopup='dialog']",
    );

    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });

    expect(mounted.container.querySelector(".picker")).toBeNull();
    expect(document.activeElement).toBe(trigger);

    await unmount(mounted.root);
  });

  test("the trigger closes the picker without reopening it", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    const trigger = mounted.container.querySelector<HTMLButtonElement>(
      ".composer__models button[aria-haspopup='dialog']",
    );
    expect(trigger).not.toBeNull();

    // A press on the trigger must not close the popup: its own click
    // handler toggles it (close + reopen would break the toggle).
    await act(async () => {
      trigger?.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    });
    expect(mounted.container.querySelector(".picker")).not.toBeNull();

    await act(async () => {
      trigger?.click();
    });
    expect(mounted.container.querySelector(".picker")).toBeNull();

    await unmount(mounted.root);
  });

  test("a press on a text inside the picker keeps it open", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await openPicker(mounted.container);

    // The group title is not focusable: the press moves the focus to the
    // body — that is an internal blur, not a dismissal.
    const title = mounted.container.querySelector<HTMLElement>(
      ".picker__group-title",
    );
    expect(title).not.toBeNull();
    await act(async () => {
      title?.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      title?.dispatchEvent(new Event("focusout", { bubbles: true }));
    });

    expect(mounted.container.querySelector(".picker")).not.toBeNull();

    await unmount(mounted.root);
  });
});

describe("the send uses exactly the checked models", () => {
  async function send(backend: PickerBackend) {
    const mounted = await mount(backend);
    await act(async () => {
      mounted.store.actions.setComposerText("hello");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });
    return mounted;
  }

  test("one checked model sends one target", async () => {
    const backend = new PickerBackend();
    const mounted = await send(backend);

    const call = backend.calls.find((entry) => entry.command === "send_message");
    expect(call?.args.request.selection.kind).toBe("single");
    expect(call?.args.request.selection.targets).toEqual([
      { provider_id: "p1", model_id: "vendor/alpha" },
    ]);

    await unmount(mounted.root);
  });

  test("several checked models send the same question to all of them", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await act(async () => {
      mounted.store.actions.setChecked(["vendor/alpha", "vendor/beta"]);
    });
    await act(async () => {
      mounted.store.actions.setComposerText("hello");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });

    const call = backend.calls.find((entry) => entry.command === "send_message");
    expect(call?.args.request.selection.kind).toBe("same_question_multi");
    expect(call?.args.request.selection.targets).toEqual([
      { provider_id: "p1", model_id: "vendor/alpha" },
      { provider_id: "p1", model_id: "vendor/beta" },
    ]);
    expect(call?.args.request.selection.primary).toBe(0);

    await unmount(mounted.root);
  });

  test("no checked model disables the send (no silent fallback)", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);
    await act(async () => {
      mounted.store.actions.setChecked([]);
    });
    await act(async () => {
      mounted.store.actions.setComposerText("hello");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });

    expect(
      backend.calls.some((entry) => entry.command === "send_message"),
    ).toBe(false);
    expect(mounted.store.state.toast?.kind).toBe("error");

    await unmount(mounted.root);
  });
});

describe("the send target stays valid", () => {
  test("removing the primary model falls back to an enabled one", async () => {
    const backend = new PickerBackend();
    const mounted = await mount(backend);

    // The primary model is "vendor/alpha" (the default).
    expect(mounted.store.state.modelSelection.primary).toBe("vendor/alpha");

    await act(async () => {
      await mounted.store.actions.setModelEnabled("vendor/alpha", false);
    });

    // The selection fell back to a model that is still enabled …
    expect(mounted.store.state.modelSelection.primary).toBe("vendor/beta");

    // … and the send uses it.
    await act(async () => {
      mounted.store.actions.setComposerText("hello");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });

    const send = backend.calls.find((call) => call.command === "send_message");
    expect(send?.args.request.selection.targets).toEqual([
      { provider_id: "p1", model_id: "vendor/beta" },
    ]);

    await unmount(mounted.root);
  });

  test("without any enabled model the send is blocked", async () => {
    const backend = new PickerBackend([
      {
        provider_id: "p1",
        model_id: "vendor/alpha",
        enabled: true,
        sort_order: 0,
        is_default: true,
        overrides: null,
      },
    ]);
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.setModelEnabled("vendor/alpha", false);
      mounted.store.actions.setComposerText("hello");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });

    expect(
      backend.calls.some((call) => call.command === "send_message"),
    ).toBe(false);
    expect(mounted.store.state.toast?.kind).toBe("error");

    await unmount(mounted.root);
  });
});
