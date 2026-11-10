import { useEffect, useState, type ReactNode } from "react";
import {
  Battery,
  BatteryCharging,
  Cpu,
  LoaderCircle,
  Pause,
  QrCode,
  RefreshCw,
  Usb,
  Video,
  Wifi,
  WifiOff,
} from "lucide-react";
import {
  getPreview,
  getState,
  newPairing,
  onState,
  setSettings,
  type AppState,
  type ConnState,
  type DecoderKind,
  type Profile,
  type SettingsPatch,
} from "@/ipc";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";

const PREVIEW_MAX_FPS = 15;

const CONN_LABEL: Record<ConnState, string> = {
  idle: "Waiting",
  connecting: "Connecting",
  live: "Live",
  paused: "Paused",
  reconnecting: "Reconnecting",
};

const PAUSE_LABEL: Record<NonNullable<AppState["pauseReason"]>, string> = {
  call: "Paused: phone is in a call",
  background: "Paused: phone app in background",
  locked: "Paused: phone is locked",
  unknown: "Paused",
};

const PROFILE_LABEL: Record<Profile, string> = {
  economy: "Economy 720p",
  balanced: "Balanced 1080p",
  quality: "Quality 4K",
};

const DECODER_LABEL: Record<DecoderKind, string> = {
  auto: "Auto (NVIDIA first)",
  cuda: "NVIDIA CUDA",
  cpu: "CPU",
};

function useAppState(): AppState | null {
  const [state, setState] = useState<AppState | null>(null);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    getState().then((s) => alive && setState(s));
    onState((s) => alive && setState(s)).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);
  return state;
}

/** Polls the preview at <= 15 fps, only while the document is visible and the connection is live. */
function usePreview(active: boolean, reset: boolean): string | null {
  const [src, setSrc] = useState<string | null>(null);
  const [visible, setVisible] = useState(() => document.visibilityState === "visible");

  useEffect(() => {
    const onVis = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", onVis);
    return () => document.removeEventListener("visibilitychange", onVis);
  }, []);

  useEffect(() => {
    if (reset) {
      setSrc(null);
    }
  }, [reset]);

  useEffect(() => {
    if (!active || !visible) {
      return;
    }
    let alive = true;
    let inflight = false;
    const tick = async () => {
      if (inflight) return;
      inflight = true;
      try {
        const b64 = await getPreview();
        if (alive && b64) setSrc(b64.startsWith("data:") ? b64 : `data:image/jpeg;base64,${b64}`);
      } catch {
        /* keep last frame */
      } finally {
        inflight = false;
      }
    };
    tick();
    const id = window.setInterval(tick, 1000 / PREVIEW_MAX_FPS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [active, visible]);

  return src;
}

export default function App() {
  const state = useAppState();
  const live = state?.conn === "live";
  const shouldResetPreview = state?.conn === "idle" || state?.conn === "connecting";
  const frame = usePreview(live, shouldResetPreview);

  if (!state) {
    return (
      <div className="flex h-full items-center justify-center gap-2 text-muted-foreground">
        <LoaderCircle className="size-4 animate-spin" /> Loading
      </div>
    );
  }

  const showQr = state.conn === "idle" || state.conn === "connecting" || state.conn === "reconnecting";
  const showPreview = (state.conn === "live" || state.conn === "paused") && Boolean(frame);
  const showWaiting = state.conn === "live" && !frame;

  return (
    <div className="flex h-full min-w-[900px] gap-3 px-3 py-2.5">
      {/* Left: QR or live/paused preview */}
      <section className="flex min-w-0 flex-1 flex-col gap-2.5">
        <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden rounded-lg border border-border bg-black">
          {showQr && (
            <div className="flex flex-col items-center gap-3 p-4">
              <div
                className="size-[280px] max-h-[50vh] max-w-[50vh] rounded-md bg-white p-3 [&>svg]:size-full"
                role="img"
                aria-label="Pairing QR code"
                dangerouslySetInnerHTML={{ __html: state.qrSvg }}
              />
              <p className="text-center text-[13px] text-muted-foreground">
                {state.conn === "connecting"
                  ? "Waiting for the phone. Scan the code with the phone camera."
                  : state.conn === "reconnecting"
                  ? "Scan again if the page was closed."
                  : "Scan this code with the phone to connect."}
              </p>
            </div>
          )}

          {showPreview && <img src={frame!} alt="Camera preview" className="size-full object-contain" />}
          {showWaiting && (
            <div className="flex items-center gap-2 text-muted-foreground">
              <LoaderCircle className="size-4 animate-spin" /> Waiting for first frame
            </div>
          )}

          {state.conn === "reconnecting" && (
            <div className="absolute inset-x-0 top-3 flex justify-center px-4">
              <div className="flex h-8 max-w-full items-center gap-2 rounded-full bg-black/80 px-3 text-[13px] text-white shadow-md">
                <RefreshCw className="size-4 shrink-0 animate-spin" aria-hidden />
                <span className="truncate">Reconnecting… scan again if the page was closed</span>
              </div>
            </div>
          )}

          {state.conn === "paused" && (
            <div className="absolute inset-x-0 top-3 flex justify-center px-4">
              <div className="flex h-8 max-w-full items-center gap-2 rounded-full bg-black/80 px-3 text-[13px] text-white shadow-md">
                <Pause className="size-4 shrink-0" aria-hidden />
                <span className="truncate">{PAUSE_LABEL[state.pauseReason ?? "unknown"]}</span>
              </div>
            </div>
          )}
        </div>

        <div className="flex items-center justify-between gap-3">
          <Badge variant={live ? "success" : state.conn === "paused" || state.conn === "reconnecting" ? "warning" : "neutral"}>
            {live ? <Video aria-hidden /> : <QrCode aria-hidden />}
            {CONN_LABEL[state.conn]}
          </Badge>
          <Button variant="outline" onClick={() => void newPairing()}>
            <QrCode aria-hidden />
            New pairing code
          </Button>
        </div>
      </section>

      {/* Right: cards */}
      <aside className="flex w-[290px] shrink-0 flex-col gap-2 overflow-y-auto">
        <ConnectionCard state={state} />
        <CameraCard state={state} />
        <DecoderCard state={state} />
      </aside>
    </div>
  );
}

function ConnectionCard({ state }: { state: AppState }) {
  const wifi = state.path === "lan";
  const usb = state.path === "usb";
  return (
    <Card>
      <CardHeader>
        <CardTitle>Connection</CardTitle>
      </CardHeader>
      <CardContent>
        <Row label="Path">
          {wifi ? (
            <Badge variant="success">
              <Wifi aria-hidden />
              Wi-Fi
            </Badge>
          ) : usb ? (
            <Badge variant="success">
              <Usb aria-hidden />
              USB
            </Badge>
          ) : (
            <Badge>
              <WifiOff aria-hidden />
              No path
            </Badge>
          )}
        </Row>
        <Row label="Latency">{state.rttMs != null ? `${Math.round(state.rttMs)} ms` : "—"}</Row>
        <Row label="Phone battery">
          {state.battery != null ? (
            <span className="flex items-center gap-1.5">
              {state.charging ? <BatteryCharging className="size-4" aria-hidden /> : <Battery className="size-4" aria-hidden />}
              {(() => { const pct = Math.round(state.battery <= 1 ? state.battery * 100 : state.battery); return state.charging ? `${pct}% charging` : `${pct}%`; })()}
            </span>
          ) : (
            "—"
          )}
        </Row>
      </CardContent>
    </Card>
  );
}

function CameraCard({ state }: { state: AppState }) {
  const { settings, lenses, zoomMin, zoomMax, torchAvailable } = state;
  const [zoomDraft, setZoomDraft] = useState<number | null>(null);
  const zoomValue = zoomDraft ?? settings.zoom;

  const patch = (p: SettingsPatch) => void setSettings(p);

  return (
    <Card>
      <CardHeader>
        <CardTitle>Camera</CardTitle>
      </CardHeader>
      <CardContent>
        <Field label="Lens">
          <Select
            value={settings.lens ?? ""}
            onValueChange={(lens) => patch({ lens })}
            disabled={lenses.length === 0}
          >
            <SelectTrigger aria-label="Lens">
              <SelectValue placeholder="No lens" />
            </SelectTrigger>
            <SelectContent>
              {lenses.map((l) => (
                <SelectItem key={l.id} value={l.id}>
                  {l.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>

        <Field label="Quality">
          <Select value={settings.profile} onValueChange={(profile) => patch({ profile: profile as Profile })}>
            <SelectTrigger aria-label="Quality profile">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(Object.keys(PROFILE_LABEL) as Profile[]).map((p) => (
                <SelectItem key={p} value={p}>
                  {PROFILE_LABEL[p]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>

        <Field label={`Zoom ${zoomValue.toFixed(1)}×`}>
          <Slider
            aria-label="Zoom"
            min={zoomMin}
            max={zoomMax}
            step={0.1}
            value={[zoomValue]}
            onValueChange={([v]) => setZoomDraft(v)}
            onValueCommit={([v]) => {
              setZoomDraft(null);
              patch({ zoom: v });
            }}
          />
        </Field>

        <SwitchRow
          label="Torch"
          checked={settings.torch}
          disabled={!torchAvailable}
          hint={torchAvailable ? undefined : "Not available on this lens"}
          onChange={(torch) => patch({ torch })}
        />
        <SwitchRow
          label="Mirror output"
          checked={settings.mirror}
          onChange={(mirror) => patch({ mirror })}
        />
      </CardContent>
    </Card>
  );
}

function DecoderCard({ state }: { state: AppState }) {
  const { settings, decoderName, fps, width, height } = state;
  return (
    <Card>
      <CardHeader>
        <CardTitle>Decoder</CardTitle>
      </CardHeader>
      <CardContent>
        <Field label="Decoder">
          <Select value={settings.decoder} onValueChange={(decoder) => void setSettings({ decoder: decoder as DecoderKind })}>
            <SelectTrigger aria-label="Decoder">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(Object.keys(DECODER_LABEL) as DecoderKind[]).map((d) => (
                <SelectItem key={d} value={d}>
                  {DECODER_LABEL[d]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Row label="In use">
          <span className="flex items-center gap-1.5">
            <Cpu className="size-4" aria-hidden />
            {decoderName ?? "—"}
          </span>
        </Row>
        <Row label="Frame rate">{fps > 0 ? `${fps} fps` : "—"}</Row>
        <Row label="Resolution">{width > 0 && height > 0 ? `${width} × ${height}` : "—"}</Row>
      </CardContent>
    </Card>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-[12px] font-medium text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-h-[28px] items-center justify-between gap-3 text-[13px]">
      <span className="text-muted-foreground">{label}</span>
      <span className="text-right">{children}</span>
    </div>
  );
}

function SwitchRow({
  label,
  checked,
  disabled,
  hint,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  hint?: string;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex min-h-[28px] items-center justify-between gap-3 text-[13px]">
      <div className="flex flex-col">
        <span>{label}</span>
        {hint && <span className="text-[11px] leading-tight text-muted-foreground">{hint}</span>}
      </div>
      <Switch checked={checked} disabled={disabled} onCheckedChange={onChange} aria-label={label} />
    </div>
  );
}
