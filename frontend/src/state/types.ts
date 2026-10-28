/**
 * Application state shape.
 *
 * The reducer in `reducer.ts` is pure so the behaviour can be unit tested
 * without a DOM or a backend.
 */

import type {
  CatalogEntry,
  ConversationSummary,
  Draft,
  EgressState,
  Generation,
  Group,
  Id,
  KeyStatus,
  Message,
  ModelPreferenceEntry,
  Settings,
  UsageRecord,
} from "../ipc/types";

/**
 * The popup dialogs the shell can show.
 *
 * Models and Settings are **modals**, not layout columns: the chat keeps
 * its width and scroll position while a dialog is open.
 */
export type ModalKind = "models" | "settings";
export type ToastKind = "info" | "error" | "success";

export interface Toast {
  id: number;
  text: string;
  kind: ToastKind;
}

export interface ModelSelectionState {
  /** Same-question multi-model mode. */
  parallel: boolean;
  /** Checked model ids (parallel mode). */
  checked: string[];
  /** Primary model; owns the main branch. */
  primary: string | null;
}

export interface EgressModalState {
  /** True when the region could not be verified (vs. explicitly blocked). */
  unknown: boolean;
  country: string | null;
  detail: string;
  /** Some (but not all) targets are Anthropic models. */
  mixed: boolean;
  checking: boolean;
}

export interface AppState {
  status: "booting" | "ready" | "error";
  bootError: string | null;
  appVersion: string;

  conversations: ConversationSummary[];
  groups: Group[];
  currentConversationId: Id | null;

  /** Per conversation snapshots. */
  messages: Record<Id, Message[]>;
  generations: Record<Id, Generation[]>;
  usage: Record<Id, UsageRecord[]>;
  /** Draft CAS token of the open conversation. */
  draftRevision: number;

  catalog: CatalogEntry[];
  catalogUpdatedAt: number | null;
  catalogRefreshing: boolean;
  preferences: ModelPreferenceEntry[];

  modelSelection: ModelSelectionState;
  /** Assistant leaves chosen for a multi-branch follow-up. */
  branchSelection: Id[];

  composerText: string;
  composerAttachments: Draft["attachments"];
  editingLatest: Id | null;
  editText: string;
  editAttachments: Draft["edit_attachments"];
  /**
   * The latest user question that **may** be edited (the backend's
   * `edit_latest` eligibility). It does not mean an edit is in progress —
   * that state lives in the draft's `edit_target_id`.
   */
  editLatestEligible: Id | null;

  /** The open popup dialog (`null` = no dialog). */
  modal: ModalKind | null;
  sidebarCollapsed: boolean;
  conversationSearch: string;
  modelSearch: string;
  pickerOpen: boolean;
  showAttempts: Record<Id, boolean>;
  splitView: Record<Id, boolean>;
  activeAnswer: Record<string, string>;

  /** Boolean map: `true` means a key exists for that provider. */
  credentials: Record<string, boolean>;
  credentialsReloading: boolean;
  keyStatus: KeyStatus | null;
  keyStatusLoading: boolean;

  settings: Settings;
  egress: EgressState | null;
  egressModal: EgressModalState | null;

  toast: Toast | null;
  toastSeq: number;
}

export const DEFAULT_SETTINGS: Settings = {
  max_concurrent_requests: 4,
  assumed_output_tokens: 1000,
  connect_timeout_secs: 15,
  read_idle_timeout_secs: 120,
  body_text_size: 15,
  locale: "en",
  theme_id: "soft_sage",
  sidebar_width: 300,
  sidebar_collapsed: false,
};

export const initialState: AppState = {
  status: "booting",
  bootError: null,
  appVersion: "0.2.1",

  conversations: [],
  groups: [],
  currentConversationId: null,

  messages: {},
  generations: {},
  usage: {},
  draftRevision: 0,

  catalog: [],
  catalogUpdatedAt: null,
  catalogRefreshing: false,
  preferences: [],

  modelSelection: { parallel: false, checked: [], primary: null },
  branchSelection: [],

  composerText: "",
  composerAttachments: [],
  editingLatest: null,
  editText: "",
  editAttachments: [],
  editLatestEligible: null,

  modal: null,
  sidebarCollapsed: false,
  conversationSearch: "",
  modelSearch: "",
  pickerOpen: false,
  showAttempts: {},
  splitView: {},
  activeAnswer: {},

  credentials: {},
  credentialsReloading: false,
  keyStatus: null,
  keyStatusLoading: false,

  settings: DEFAULT_SETTINGS,
  egress: null,
  egressModal: null,

  toast: null,
  toastSeq: 0,
};
