import { describe, expect, test } from "bun:test";

import {
  developerOf,
  disambiguate,
  isAnthropic,
  presentation,
} from "../src/model/presentation";

/**
 * The presentation rules mirror `src/domain/model_identity.rs`; the UI must
 * group and label models exactly like the native app.
 */
describe("developerOf", () => {
  test("maps known namespaces", () => {
    expect(developerOf("anthropic/claude-opus-5.5")).toBe("Anthropic");
    expect(developerOf("openai/gpt-4.1")).toBe("OpenAi");
    expect(developerOf("google/gemini-2.5-flash")).toBe("Google");
    expect(developerOf("deepseek/deepseek-chat")).toBe("DeepSeek");
    expect(developerOf("moonshotai/kimi-k2")).toBe("MoonshotAi");
    expect(developerOf("qwen/qwen3-max")).toBe("Alibaba");
    expect(developerOf("x-ai/grok-4")).toBe("XAI");
  });

  test("does not guess for unknown namespaces", () => {
    expect(developerOf("acme/secret-model")).toBe("Unknown");
    expect(developerOf("plain-model")).toBe("Unknown");
  });

  test("covers the controlled Anthropic aliases", () => {
    expect(developerOf("claude-3.5-sonnet")).toBe("Anthropic");
    expect(isAnthropic("claude-3.5-sonnet")).toBe(true);
    expect(isAnthropic("anthropic/claude-fable-5.1")).toBe(true);
    expect(isAnthropic("openai/gpt-4.1")).toBe(false);
  });
});

describe("presentation", () => {
  test("groups by family and shortens the item label", () => {
    const result = presentation("anthropic/claude-opus-4.1", "Anthropic: Claude Opus 4.1");
    expect(result.family_key).toBe("claude");
    expect(result.group_label).toBe("Claude");
    expect(result.item_label).toBe("Opus 4.1");
    expect(result.full_label).toBe("Claude Opus 4.1");
  });

  test("keeps the o series as one label", () => {
    const result = presentation("openai/o3", "OpenAI: o3");
    expect(result.family_key).toBe("o");
    expect(result.group_label).toBe("o series");
    expect(result.item_label).toBe("o3");
    expect(result.full_label).toBe("o3");
  });

  test("keeps version numbers verbatim", () => {
    const gpt = presentation("openai/gpt-4o", "OpenAI: GPT-4o");
    expect(gpt.group_label).toBe("GPT");
    expect(gpt.item_label).toBe("4o");
    expect(gpt.full_label).toBe("GPT 4o");

    const gemini = presentation("google/gemini-2.5-pro", "Google: Gemini 2.5 Pro");
    expect(gemini.group_label).toBe("Gemini");
    expect(gemini.item_label).toBe("2.5 Pro");
    const sonnet = presentation(
      "anthropic/claude-sonnet-4-20250514",
      "Anthropic: Claude Sonnet 4 (2025-05-14)",
    );
    expect(sonnet.item_label).toBe("Sonnet 4 20250514");
  });

  test("keeps a variant suffix", () => {
    const result = presentation("deepseek/deepseek-chat:free", "DeepSeek: V3");
    expect(result.item_label).toContain("(free)");
  });

  test("treats an unknown model as its own group", () => {
    const result = presentation("weirdvendor/special-model-x", "Special Model X");
    expect(result.group_label).toBe("Special Model X");
    expect(result.item_label).toBe("Special Model X");
    expect(result.full_label).toBe("Special Model X");
  });

  test("falls back to a humanised id without a catalog name", () => {
    const result = presentation("another/thing", "");
    expect(result.group_label.length).toBeGreaterThan(0);
    expect(result.item_label.length).toBeGreaterThan(0);
    expect(result.group_label).toBe("Thing");
  });

  test("keeps a variant suffix", () => {
    const result = presentation("deepseek/deepseek-r1:free", "DeepSeek: R1 (free)");
    expect(result.group_label).toBe("DeepSeek");
    expect(result.item_label).toContain("(free)");
  });

  test("disambiguates identical short names inside a family", () => {
    const items = [
      presentation("anthropic/claude-sonnet-4.5", "Anthropic: Claude Sonnet 4.5"),
      presentation("anthropic/claude-sonnet-4.5:free", "Anthropic: Claude Sonnet 4.5"),
    ];
    const result = disambiguate(items);
    expect(result[0].item_label).not.toBe(result[1].item_label);
    expect(result[1].item_label).toContain("(free)");
  });

  test("leaves unique labels untouched", () => {
    const items = [
      presentation("anthropic/claude-opus-4.1", "Anthropic: Claude Opus 4.1"),
      presentation("anthropic/claude-sonnet-4.5", "Anthropic: Claude Sonnet 4.5"),
    ];
    expect(disambiguate(items)).toEqual(items);
  });
});
