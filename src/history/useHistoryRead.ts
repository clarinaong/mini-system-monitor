import { useEffect, useState, type RefObject } from "react";
import { historyError } from "./query";

type Outcome<T> = { data: T } | { error: string };

export function useHistoryRead<Request, Result>(
  request: Request,
  read: (request: Request) => Promise<Result>,
  pending: RefObject<Promise<unknown> | null>,
  attempt: number,
  subject: "history" | "incidents",
) {
  const [settled, setSettled] = useState<{
    request: Request;
    attempt: number;
    outcome: Outcome<Result>;
  } | null>(null);

  useEffect(() => {
    let active = true;

    async function load() {
      while (active && pending.current) await pending.current;
      if (!active) return;

      const task = read(request).then(
        data => ({ data }),
        error => ({ error: historyError(error, subject) }),
      );
      pending.current = task;
      const outcome = await task;
      pending.current = null;
      if (active) setSettled({ request, attempt, outcome });
    }

    void load();
    return () => { active = false; };
  }, [request, read, pending, attempt, subject]);

  return settled?.request === request && settled.attempt === attempt ? settled.outcome : null;
}
