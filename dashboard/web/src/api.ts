import { z } from "zod";
import { ApiErrorBody } from "../../shared/schemas";

export class ApiClientError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

export interface ApiResult<T> {
  data: T;
  status: number;
  /** True when the server answered from a stored idempotent result (HTTP 200 + header). */
  replayed: boolean;
}

/** Every response is validated against the shared schema, so contract drift surfaces as an error. */
export async function api<T>(path: string, schema: z.ZodType<T>, init: RequestInit = {}): Promise<ApiResult<T>> {
  const res = await fetch(`/api${path}`, {
    ...init,
    headers: { "content-type": "application/json", ...(init.headers ?? {}) },
  });
  const body: unknown = await res.json().catch(() => null);

  if (!res.ok) {
    const err = ApiErrorBody.safeParse(body);
    throw err.success
      ? new ApiClientError(res.status, err.data.error.code, err.data.error.message)
      : new ApiClientError(res.status, "unknown_error", `Request failed with status ${res.status}`);
  }
  const parsed = schema.safeParse(body);
  if (!parsed.success) {
    throw new ApiClientError(502, "contract_violation", "The server's response did not match the expected shape.");
  }
  return { data: parsed.data, status: res.status, replayed: res.headers.get("idempotent-replayed") === "true" };
}

export const post = (body: unknown, headers: Record<string, string> = {}): RequestInit => ({
  method: "POST",
  body: JSON.stringify(body),
  headers,
});
