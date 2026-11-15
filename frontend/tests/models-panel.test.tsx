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
 * The Models panel is the management surface of the models.
 *
 * These tests cover the behaviour the user asked for:
 *   - a catalogue model **without** a preference can be added to "My models"
 *     and removed again;
 *   - the write is persisted with the right payload (the provider id of the
 *     catalogue, the untouched sort order / default / overrides);
 *   - a failed write restores the previous state and reports the error;
 *   - a fast double click is not undone by the late reply of the first one.
 */

const CATALOG: CatalogEntry[] = [
  {
    id: "vendor/alpha",
    provider_id: "p1",
    name: "Vendor: Alpha",
    family: null,
    context_length: null,
    prompt_price: "1.00",
    completion_price: "2.00",
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
    id: "other/gamma",
    provider_id: "p2",
    name: "Other: Gamma",
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
    overrides: { temperature: 0.2 },
  },
];

class ModelsBackend implements Transport {
  readonly calls: { command: string; args: Record<string, any> }[] = [];
  readonly listeners = new Map<string, Set<(payload: unknown) => void>>();
  /** The preferences the backend would return after a restart. */
  preferences: ModelPreferenceEntry[] = PREFERENCES.map((entry) => ({ ...entry }));
  /** When set, the next `set_model_preference` fails with this message. */
  failNextWrite: string | null = null;
  /** Deferred writes, so a late reply can be simulated. */
  readonly pendingWrites: {
    args: Record<string, any>;
    resolve(): void;
    reject(error: unknown): void;
  }[] = [];
  deferWrites = false;

  constructor(readonly catalog: CatalogEntry[] = CATALOG) {}

  async invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
    this.calls.push({ command, args });
    switch (command) {
      case "bootstrap":
        return this.bootstrap() as T;
      case "load_conversation":
        return fixtureDetail(String(args.conversation_id), []) as T;
      case "get_draft":
        return fixtureDraft("") as T;
      case "list_preferences":
        return this.preferences as T;
      case "set_model_preference": {
        if (this.failNextWrite) {
          const message = this.failNextWrite;
          this.failNextWrite = null;
          throw new Error(message);
        }
        if (this.deferWrites) {
          await new Promise<void>((resolve, reject) => {
            this.pendingWrites.push({ args, resolve, reject });
          });
        }
        const patch = args.preference as ModelPreferenceEntry;
        const existing = this.preferences.find(
          (entry) => entry.model_id === args.model_id,
        );
        if (existing) {
          this.preferences = this.preferences.map((entry) =>
            entry.model_id === args.model_id ? { ...entry, ...patch } : entry,
          );
        } else {
          this.preferences = [
            ...this.preferences,
            {
              ...patch,
              provider_id: String(args.provider_id),
              model_id: String(args.model_id),
            },
          ];
        }
        return null as T;
      }
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

async function mount(backend: ModelsBackend) {
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

/** The rows of the catalogue section of the Models panel. */
function catalogRows(): HTMLElement[] {
  const body = document.body.querySelector<HTMLElement>(
    "[data-testid='modal-models-body']",
  );
  if (!body) throw new Error("the models panel is not open");
  const rows = [...body.querySelectorAll<HTMLElement>(".model-row")];
  // The first rows belong to "My models"; the catalogue follows.
  return rows.filter((row) => row.querySelector("input[type='checkbox']"));
}

describe("the Models panel manages the whole catalogue", () => {
  test("a catalogue model without a preference joins My models", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    // "vendor/beta" is in the catalogue but has no preference at all.
    expect(
      mounted.store.state.preferences.some(
        (entry) => entry.model_id === "vendor/beta",
      ),
    ).toBe(false);

    await act(async () => {
      await mounted.store.actions.setModelEnabled("vendor/beta", true);
    });

    // The state knows the model now …
    const preference = mounted.store.state.preferences.find(
      (entry) => entry.model_id === "vendor/beta",
    );
    expect(preference?.enabled).toBe(true);
    // … with the provider id of the catalogue and a free sort order.
    expect(preference?.provider_id).toBe("p1");
    expect(preference?.sort_order).toBe(1);
    expect(preference?.is_default).toBe(false);
    expect(preference?.overrides).toBeNull();

    // The write carries exactly that payload.
    const write = backend.calls.find(
      (call) => call.command === "set_model_preference",
    );
    expect(write?.args.model_id).toBe("vendor/beta");
    expect(write?.args.provider_id).toBe("p1");
    expect(write?.args.preference).toEqual({
      enabled: true,
      sort_order: 1,
      is_default: false,
      overrides: null,
    });

    // The backend holds the same preference → a restart shows the model.
    expect(
      backend.preferences.find((entry) => entry.model_id === "vendor/beta")
        ?.enabled,
    ).toBe(true);

    await unmount(mounted.root);
  });

  test("the panel shows the model in My models and the catalogue", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.openModal("models");
    });

    const body = document.body.querySelector<HTMLElement>(
      "[data-testid='modal-models-body']",
    );
    expect(body).not.toBeNull();
    // The catalogue lists every model, including the ones without a
    // preference.
    expect(body?.textContent).toContain("Beta");

    await act(async () => {
      await mounted.store.actions.setModelEnabled("vendor/beta", true);
    });

    const sections = [...(body?.querySelectorAll("section") ?? [])];
    const mine = sections[0];
    expect(mine?.textContent).toContain("Beta");

    await unmount(mounted.root);
  });

  test("the search really filters the catalogue", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.openModal("models");
    });

    const body = document.body.querySelector<HTMLElement>(
      "[data-testid='modal-models-body']",
    );
    expect(body?.textContent).toContain("Gamma");

    await act(async () => {
      mounted.store.dispatch({ type: "model/search", query: "gamma" });
    });

    expect(body?.textContent).toContain("Gamma");
    expect(body?.textContent).not.toContain("Beta");

    await act(async () => {
      mounted.store.dispatch({ type: "model/search", query: "nothing" });
    });
    expect(body?.textContent).not.toContain("Gamma");

    await unmount(mounted.root);
  });

  test("removing a model keeps its order, its default flag and its overrides", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.setModelEnabled("vendor/alpha", false);
    });

    const write = backend.calls.find(
      (call) => call.command === "set_model_preference",
    );
    expect(write?.args.preference).toEqual({
      enabled: false,
      sort_order: 0,
      is_default: true,
      overrides: { temperature: 0.2 },
    });

    const preference = mounted.store.state.preferences.find(
      (entry) => entry.model_id === "vendor/alpha",
    );
    expect(preference?.enabled).toBe(false);
    expect(preference?.sort_order).toBe(0);
    expect(preference?.is_default).toBe(true);
    expect(preference?.overrides).toEqual({ temperature: 0.2 });

    await unmount(mounted.root);
  });

  test("a failed write restores the previous state and reports it", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    backend.failNextWrite = "disk is full";

    await act(async () => {
      await mounted.store.actions.setModelEnabled("vendor/beta", true);
    });

    // The model is not in "My models" any more …
    expect(
      mounted.store.state.preferences.some(
        (entry) => entry.model_id === "vendor/beta" && entry.enabled,
      ),
    ).toBe(false);
    // … and the user is told why.
    expect(mounted.store.state.toast?.kind).toBe("error");
    expect(mounted.store.state.toast?.text).toContain("disk is full");

    await unmount(mounted.root);
  });

  test("a failed write of a known model restores its enabled flag", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    backend.failNextWrite = "nope";
    await act(async () => {
      await mounted.store.actions.setModelEnabled("vendor/alpha", false);
    });

    const preference = mounted.store.state.preferences.find(
      (entry) => entry.model_id === "vendor/alpha",
    );
    expect(preference?.enabled).toBe(true);
    expect(mounted.store.state.toast?.kind).toBe("error");

    await unmount(mounted.root);
  });

  test("a late failure does not undo a newer click", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);
    backend.deferWrites = true;

    // Two clicks in a row: enable, then disable.
    await act(async () => {
      void mounted.store.actions.setModelEnabled("vendor/beta", true);
      void mounted.store.actions.setModelEnabled("vendor/beta", false);
    });

    // The first write is in flight; it fails late.
    await act(async () => {
      backend.pendingWrites[0]?.reject(new Error("late failure"));
      await Promise.resolve();
      await Promise.resolve();
    });

    // The second click owns the model: the state stays "disabled" …
    const preference = mounted.store.state.preferences.find(
      (entry) => entry.model_id === "vendor/beta",
    );
    expect(preference?.enabled).toBe(false);

    // … and the second write is sent after the first one.
    await act(async () => {
      backend.pendingWrites[1]?.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    const writes = backend.calls.filter(
      (call) => call.command === "set_model_preference",
    );
    expect(writes.length).toBe(2);
    expect(writes[0].args.preference.enabled).toBe(true);
    expect(writes[1].args.preference.enabled).toBe(false);

    await unmount(mounted.root);
  });

  test("an unknown model is refused with a message", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.setModelEnabled("does/not-exist", true);
    });

    expect(
      backend.calls.some((call) => call.command === "set_model_preference"),
    ).toBe(false);
    expect(mounted.store.state.toast?.kind).toBe("error");

    await unmount(mounted.root);
  });
});

describe("the catalogue rows are reachable", () => {
  test("every catalogue row has a working checkbox", async () => {
    const backend = new ModelsBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.openModal("models");
    });

    const rows = catalogRows();
    expect(rows.length).toBe(CATALOG.length);

    // The last catalogue row (the one at the bottom) is a real control.
    const last = rows[rows.length - 1];
    const checkbox = last.querySelector<HTMLInputElement>("input[type='checkbox']");
    expect(checkbox).not.toBeNull();
    expect(checkbox?.checked).toBe(false);

    await act(async () => {
      checkbox?.click();
    });

    const enabledId = mounted.store.state.preferences.find(
      (entry) => entry.enabled && entry.model_id !== "vendor/alpha",
    )?.model_id;
    expect(enabledId).toBe(
      last.querySelector(".model-row__name")?.getAttribute("title") ?? "",
    );
    expect(mounted.store.state.preferences.some((entry) => entry.enabled)).toBe(
      true,
    );

    await unmount(mounted.root);
  });
});
