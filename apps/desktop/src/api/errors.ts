// Normalize errors returned from `invoke` / React Query into a string
// the UI can show. Tauri serializes our `CoreClientError` as
// `{ kind: "rpc"|"transport"|"not_implemented", message: string }`
// — `String(e)` on that object yields `[object Object]`, which is the
// bug the renderer used to surface to users.

export function formatError(err: unknown): string {
  if (err == null) return "unknown error";
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (typeof err === "object") {
    const obj = err as { kind?: unknown; message?: unknown };
    if (typeof obj.message === "string") {
      return typeof obj.kind === "string"
        ? `${obj.kind}: ${obj.message}`
        : obj.message;
    }
    try {
      return JSON.stringify(err);
    } catch {
      return String(err);
    }
  }
  return String(err);
}
