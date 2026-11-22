import { useCallback, useState } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { Account } from "../../shared/schemas";
import { api } from "./api";

/**
 * Ledgerd has no "list accounts" endpoint yet, so the dashboard remembers account ids in this
 * browser. When `GET /v1/accounts` lands, replace this hook's storage with a single query.
 */
const KEY = "ledgerd.accounts";

function load(): string[] {
  try {
    const raw = localStorage.getItem(KEY);
    const ids: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(ids) ? ids.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function useAccounts() {
  const [ids, setIds] = useState<string[]>(load);
  const qc = useQueryClient();

  const persist = useCallback((next: string[]) => {
    setIds(next);
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {
      /* storage unavailable: keep in memory only */
    }
  }, []);

  const queries = useQueries({
    queries: ids.map((id) => ({
      queryKey: ["account", id],
      queryFn: async () => (await api(`/accounts/${id}`, Account)).data,
    })),
  });

  return {
    ids,
    queries,
    accounts: queries.flatMap((q) => (q.data ? [q.data] : [])),
    add: (id: string) => ids.includes(id) || persist([...ids, id]),
    remove: (id: string) => persist(ids.filter((x) => x !== id)),
    refresh: () => qc.invalidateQueries({ queryKey: ["account"] }),
  };
}
