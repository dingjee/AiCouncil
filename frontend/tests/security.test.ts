import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

/**
 * Guards for the credential and rendering rules:
 *   - no secret is ever persisted in the browser;
 *   - the settings panel has no key input (the runtime reads the local
 *     `.env`, the UI only shows booleans and a Reload control);
 *   - the model output is never inserted as raw HTML.
 */

const SRC = new URL("../src", import.meta.url).pathname;

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) out.push(...sourceFiles(path));
    else if (/\.(ts|tsx)$/.test(path)) out.push(path);
  }
  return out;
}

describe("no browser persistence", () => {
  test("the source never touches localStorage or sessionStorage", () => {
    const offenders: string[] = [];
    for (const file of sourceFiles(SRC)) {
      const content = readFileSync(file, "utf8");
      if (/localStorage|sessionStorage/.test(content)) offenders.push(file);
    }
    expect(offenders).toEqual([]);
  });

  test("the source never handles an api key value", () => {
    const offenders: string[] = [];
    for (const file of sourceFiles(SRC)) {
      const content = readFileSync(file, "utf8");
      // A key *value* would have to be read, sent or stored somewhere.
      if (/api_key\s*[:=]|apiKey\s*[:=]|save_credential|delete_credential/.test(content)) {
        offenders.push(file);
      }
    }
    expect(offenders).toEqual([]);
  });
});

describe("credentials surface", () => {
  test("the client only asks for the credential status", () => {
    const content = readFileSync(join(SRC, "ipc/client.ts"), "utf8");
    expect(content).toContain("credentialStatus");
    expect(content).toContain("reloadCredentials");
    // No command that would transmit a key from the UI.
    expect(content).not.toContain("saveCredential");
    expect(content).not.toContain("deleteCredential");
  });

  test("the credential payload is a boolean map", () => {
    const content = readFileSync(join(SRC, "ipc/types.ts"), "utf8");
    expect(content).toContain("Record<string, boolean>");
    expect(content).not.toContain("api_key");
  });

  test("the settings panel has no key input field", () => {
    const content = readFileSync(join(SRC, "components/SettingsPanel.tsx"), "utf8");
    expect(content).not.toContain('type="password"');
    expect(content).not.toContain("api_key");
    expect(content).toContain("reloadCredentials");
    expect(content).toContain("credentials_env_hint");
  });
});

describe("markdown safety", () => {
  test("raw HTML from the model is never injected", () => {
    const content = readFileSync(join(SRC, "markdown/render.ts"), "utf8");
    // markdown-it runs with `html: false`.
    expect(content).toContain("html: false");
  });

  test("the message renderer goes through the safe renderer", () => {
    const content = readFileSync(join(SRC, "components/Markdown.tsx"), "utf8");
    expect(content).toContain("renderMarkdown");
    expect(content).not.toContain("dangerouslySetInnerHTML={{ __html: message.content");
  });
});
