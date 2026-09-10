// Tests for the custom-model status / picker-filter mapping helpers
import { describe, it, expect } from "vitest";
import {
  customModelStatus,
  pickFileFilters,
  customPathConfigKey,
  CUSTOM_MODEL_ID,
} from "./customModel";

describe("customModelStatus", () => {
  it("lists every unset path, in encoder/decoder/tokens order", () => {
    const status = customModelStatus({ encoder: "", decoder: "", tokens: "" }, false);
    expect(status).toEqual({ kind: "missing-paths", missing: ["encoder", "decoder", "tokens"] });
  });

  it("treats whitespace-only paths as unset", () => {
    const status = customModelStatus({ encoder: "  ", decoder: "/m/dec.onnx", tokens: "" }, false);
    expect(status).toEqual({ kind: "missing-paths", missing: ["encoder", "tokens"] });
  });

  it("reports files-missing when all paths set but the core says files don't exist", () => {
    const status = customModelStatus(
      { encoder: "/m/enc.onnx", decoder: "/m/dec.onnx", tokens: "/m/tokens.txt" },
      false,
    );
    expect(status).toEqual({ kind: "files-missing" });
  });

  it("reports ready when all paths set and the core verified the files", () => {
    const status = customModelStatus(
      { encoder: "/m/enc.onnx", decoder: "/m/dec.onnx", tokens: "/m/tokens.txt" },
      true,
    );
    expect(status).toEqual({ kind: "ready" });
  });

  it("missing paths win over the files-verified flag", () => {
    const status = customModelStatus({ encoder: "", decoder: "/m/dec.onnx", tokens: "/m/t.txt" }, true);
    expect(status).toEqual({ kind: "missing-paths", missing: ["encoder"] });
  });
});

describe("pickFileFilters", () => {
  it("uses .onnx for encoder/decoder and .txt for tokens", () => {
    expect(pickFileFilters("encoder")).toEqual([{ name: "ONNX model", extensions: ["onnx"] }]);
    expect(pickFileFilters("decoder")).toEqual([{ name: "ONNX model", extensions: ["onnx"] }]);
    expect(pickFileFilters("tokens")).toEqual([{ name: "Tokens file", extensions: ["txt"] }]);
  });
});

describe("customPathConfigKey", () => {
  it("maps to the AppConfig serde keys", () => {
    expect(customPathConfigKey("encoder")).toBe("custom_encoder_path");
    expect(customPathConfigKey("decoder")).toBe("custom_decoder_path");
    expect(customPathConfigKey("tokens")).toBe("custom_tokens_path");
  });
});

describe("CUSTOM_MODEL_ID", () => {
  it("matches the serde name of ModelVariant::Custom", () => {
    expect(CUSTOM_MODEL_ID).toBe("Custom");
  });
});
