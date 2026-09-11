// i18n groundwork tests (canario-7ah.7): locale fallback resolution,
// catalog invariants, and completeness of the extraction — every `t("…")`
// call site must have a catalog entry, and every catalog entry must be
// referenced somewhere (no dead keys).
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { en, type MessageKey } from "./en";
import { resolveLocale, t } from "./index";

const rendererRoot = fileURLToPath(new URL("..", import.meta.url));

/** Every non-test renderer source file (the i18n dir itself excluded — it's the catalog side). */
function rendererSources(dir: string = rendererRoot): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      if (entry === "i18n") continue;
      out.push(...rendererSources(full));
    } else if (/\.(tsx?|jsx?)$/.test(entry) && !/\.test\.[jt]sx?$/.test(entry)) {
      out.push(full);
    }
  }
  return out;
}

/** Every `t("literal.key")` call found in renderer sources (the completeness surface). */
function usedKeys(): Set<string> {
  const keys = new Set<string>();
  for (const file of rendererSources()) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/\bt\(\s*(["'])([a-zA-Z0-9_.]+)\1/g)) {
      keys.add(match[2]);
    }
  }
  return keys;
}

describe("i18n catalog", () => {
  it("has no empty or whitespace-only values", () => {
    for (const [key, value] of Object.entries(en)) {
      expect(value, key).toMatch(/\S/);
    }
  });

  it("has no duplicate values collapsed by mistake (spot-check a few exact strings)", () => {
    // String-for-string parity spot checks against the pre-i18n UI text.
    expect(en["common.cancel"]).toBe("Cancel");
    expect(en["overlay.transcribing"]).toBe("Transcribing…");
    expect(en["record.transcribing"]).toBe("Transcribing...");
    expect(en["history.searchPlaceholder"]).toBe("🔍  Search transcriptions...");
  });

  it("resolves templates ({{ arg }} placeholders)", () => {
    expect(t("model.download", { name: "Parakeet TDT v3" })).toBe("Download Parakeet TDT v3");
    expect(t("history.noResults", { query: "hello" })).toBe("No results found for “hello”");
    expect(t("model.custom.missingPaths", { fields: "encoder, decoder", plural: "s" })).toBe(
      "⚠ Set the encoder, decoder paths — recording will fail until all three are configured.",
    );
    expect(t("model.custom.missingPaths", { fields: "encoder", plural: "" })).toBe(
      "⚠ Set the encoder path — recording will fail until all three are configured.",
    );
  });

  it("every key used via t(\"…\") in renderer sources exists in the catalog", () => {
    const catalog = new Set(Object.keys(en));
    const missing = [...usedKeys()].filter((key) => !catalog.has(key));
    expect(missing).toEqual([]);
  });

  it("every catalog key is referenced by some renderer source (no dead keys)", () => {
    const sources = rendererSources().map((file) => readFileSync(file, "utf8")).join("\n");
    const orphans = (Object.keys(en) as MessageKey[]).filter(
      (key) => !sources.includes(`"${key}"`) && !sources.includes(`'${key}'`),
    );
    expect(orphans).toEqual([]);
  });
});

describe("resolveLocale", () => {
  it("falls back to en with no preference list", () => {
    expect(resolveLocale(undefined)).toBe("en");
  });

  it("falls back to en with an empty preference list", () => {
    expect(resolveLocale([])).toBe("en");
  });

  it("accepts an exact match", () => {
    expect(resolveLocale(["en"])).toBe("en");
  });

  it("accepts a base-language match (en-GB → en)", () => {
    expect(resolveLocale(["en-GB", "en-US"])).toBe("en");
  });

  it("matches case-insensitively (EN-us → en)", () => {
    expect(resolveLocale(["EN-us"])).toBe("en");
  });

  it("walks the list: an unsupported first choice falls through to en", () => {
    expect(resolveLocale(["pt-BR", "fr-FR", "en-GB"])).toBe("en");
  });

  it("returns en when nothing in the list is supported", () => {
    expect(resolveLocale(["ja-JP", "de-DE"])).toBe("en");
  });
});

describe("t", () => {
  it("translates plain keys", () => {
    expect(t("common.notSet")).toBe("Not set");
    expect(t("hotkey.notice.step2")).toBe(
      "Log out and back in — group membership only applies to new sessions.",
    );
  });
});
