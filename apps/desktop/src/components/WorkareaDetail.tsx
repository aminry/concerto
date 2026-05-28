// Placeholder workarea detail panel. V0.1 renders the selected
// workarea's JSON; the terminal arrives in Task 26.

import { useUiStore } from "../state/useUiStore";
import { useWorkarea } from "../hooks/useWorkareas";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";

export function WorkareaDetail(): JSX.Element {
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  const workareaQuery = useWorkarea(selectedWorkareaId);

  return (
    <main className="flex-1 p-6 overflow-auto">
      <Card>
        <CardHeader>
          <CardTitle>Workarea</CardTitle>
        </CardHeader>
        <CardContent>
          {workareaQuery.isLoading && <p>Loading…</p>}
          {workareaQuery.isError && (
            <p className="text-rose-400">
              Failed to load: {String(workareaQuery.error)}
            </p>
          )}
          {workareaQuery.data && (
            <pre className="text-xs whitespace-pre-wrap text-emerald-300">
              {JSON.stringify(workareaQuery.data, null, 2)}
            </pre>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
