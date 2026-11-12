import { useEffect, useRef, useState } from "react";
import { historyError, queryHistory, type HistoryRequest, type HistoryResult } from "./query";

type Outcome = { data: HistoryResult } | { error: string };

export function useHistory(request: HistoryRequest) {
  const pending = useRef<Promise<Outcome> | null>(null);
  const [settled, setSettled] = useState<{ request: HistoryRequest; outcome: Outcome } | null>(null);

  useEffect(() => {
    let active = true;

    async function read() {
      if (pending.current) await pending.current;
      if (!active) return;

      const task = queryHistory(request).then(
        data => ({ data }),
        error => ({ error: historyError(error) }),
      );
      pending.current = task;
      const outcome = await task;
      pending.current = null;
      if (active) setSettled({ request, outcome });
    }

    void read();
    return () => { active = false; };
  }, [request]);

  return settled?.request === request ? settled.outcome : null;
}
