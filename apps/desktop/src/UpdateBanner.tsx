import { useState } from "react";
import { installUpdate, type AppState } from "@/ipc";
import { Button } from "@/components/ui/button";

/** Hairline strip shown only while a newer release is available. */
export function UpdateBanner({ state }: { state: AppState }) {
  const [failed, setFailed] = useState(false);
  const version = state.updateVersion;
  if (!version) return null;
  const progress = state.updateProgress;
  const busy = progress != null;

  const install = () => {
    setFailed(false);
    installUpdate().catch(() => setFailed(true));
  };

  return (
    <div className="-mb-1 flex items-center justify-between gap-4 border border-border px-3 py-2">
      <span className="font-mono text-[11px] tracking-[0.18em] text-muted-foreground uppercase">
        [ Update ] v{version} available
        {busy && ` · ${progress}%`}
        {failed && !busy && " · failed, try again"}
      </span>
      <Button size="sm" variant="outline" disabled={busy} onClick={install}>
        {busy ? "Installing" : "Install"}
      </Button>
    </div>
  );
}
