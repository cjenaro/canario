// Helpers for the "Custom" model variant (local sherpa-onnx files, no
// download). Pure mapping logic so it can be unit-tested without the UI.

export const CUSTOM_MODEL_ID = "Custom";

export interface CustomModelPaths {
  encoder: string;
  decoder: string;
  tokens: string;
}

export type CustomModelStatus =
  | { kind: "missing-paths"; missing: string[] }
  | { kind: "files-missing" }
  | { kind: "ready" };

/**
 * Derive the custom-model status line from the configured paths and the
 * core's is_model_downloaded verdict (which checks all four resolved files
 * exist — joiner.int8.onnx must sit next to the encoder).
 */
export function customModelStatus(
  paths: CustomModelPaths,
  filesVerified: boolean,
): CustomModelStatus {
  const missing: string[] = [];
  if (!paths.encoder.trim()) missing.push("encoder");
  if (!paths.decoder.trim()) missing.push("decoder");
  if (!paths.tokens.trim()) missing.push("tokens");
  if (missing.length > 0) return { kind: "missing-paths", missing };
  return filesVerified ? { kind: "ready" } : { kind: "files-missing" };
}

/** File-picker filters per custom path field (tokens is .txt, rest .onnx). */
export function pickFileFilters(field: keyof CustomModelPaths): { name: string; extensions: string[] }[] {
  return field === "tokens"
    ? [{ name: "Tokens file", extensions: ["txt"] }]
    : [{ name: "ONNX model", extensions: ["onnx"] }];
}

/** Map a custom path field to its AppConfig key (serde snake_case). */
export function customPathConfigKey(
  field: keyof CustomModelPaths,
): "custom_encoder_path" | "custom_decoder_path" | "custom_tokens_path" {
  return `custom_${field}_path`;
}
