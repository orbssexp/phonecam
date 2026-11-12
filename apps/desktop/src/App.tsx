import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Battery,
  BatteryCharging,
  Cpu,
  LoaderCircle,
  Pause,
  QrCode,
  RefreshCw,
  RotateCw,
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
import { cn } from "@/lib/utils";
import { UpdateBanner } from "@/UpdateBanner";

const PREVIEW_MAX_FPS = 15;

const CONN_LABEL: Record<ConnState, string> = {
  idle: "Waiting",
  connecting: "Connecting",
  live: "Live",
  paused: "Paused",
  reconnecting: "Reconnecting",
};

const CONN_STATUS_LINE: Record<ConnState, string> = {
  idle: "Scan the code with the phone to connect.",
  connecting: "Waiting for the phone to connect.",
  live: "Streaming from the phone.",
  paused: "Stream paused on the phone.",
  reconnecting: "Reconnecting. Scan again if the page was closed.",
};

const CONN_DOT: Record<ConnState, string> = {
  idle: "bg-white/35",
  connecting: "bg-white/35",
  live: "bg-success",
  paused: "bg-warning",
  reconnecting: "bg-warning",
};

const SOURCE_LABEL: Record<NonNullable<AppState["source"]>, string> = {
  native: "Native (LAN)",
  web: "Web (WebRTC)",
  usb: "USB",
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
  smooth: "Smooth 1080p 60 (app)",
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
      <div className="flex h-full items-center justify-center gap-2 font-mono text-[11px] tracking-[0.18em] text-muted-foreground uppercase">
        <LoaderCircle className="size-3.5 animate-spin" /> Loading
      </div>
    );
  }

  const showQr = state.conn === "idle" || state.conn === "connecting" || state.conn === "reconnecting";
  const showPreview = (state.conn === "live" || state.conn === "paused") && Boolean(frame);
  const showWaiting = state.conn === "live" && !frame;

  return (
    <div className="flex h-full min-w-[900px] flex-col gap-4 px-6 pt-5 pb-4">
      <UpdateBanner state={state} />

      {/* Header: app name, muted status line, status tag on the right, hairline below */}
      <header className="flex items-start justify-between gap-6 border-b border-border pb-4">
        <div className="min-w-0">
          <h1 className="text-[32px] leading-none font-light tracking-[-0.03em]">PhoneCam.</h1>
          <p className="mt-2 text-[13px] text-muted-foreground">{CONN_STATUS_LINE[state.conn]}</p>
        </div>
        <span className="mt-1.5 inline-flex shrink-0 items-center gap-2 font-mono text-[11px] tracking-[0.18em] text-muted-foreground uppercase">
          <span className={cn("size-1.5", CONN_DOT[state.conn])} aria-hidden />[ {CONN_LABEL[state.conn]} ]
        </span>
      </header>

      <main className="flex min-h-0 flex-1 gap-5">
        {/* Left: QR or live/paused preview */}
        <section className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden border border-border bg-black">
          <div className="relative flex min-h-0 flex-1 items-center justify-center">
            {showQr && (
              <div className="flex flex-col items-center gap-4 p-4 pb-16">
                <div
                  className="size-[280px] max-h-[50vh] max-w-[50vh] bg-white p-3 [&>svg]:size-full"
                  role="img"
                  aria-label="Pairing QR code"
                  dangerouslySetInnerHTML={{ __html: state.qrSvg }}
                />
                <p className="text-center text-[12px] text-muted-foreground">
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
              <div className="flex items-center gap-2 font-mono text-[11px] tracking-[0.14em] text-muted-foreground uppercase">
                <LoaderCircle className="size-3.5 animate-spin" /> Waiting for first frame
              </div>
            )}

            {state.conn === "reconnecting" && (
              <div className="absolute inset-x-0 top-3 flex justify-center px-4">
                <div className="flex h-8 max-w-full items-center gap-2 border border-border bg-black px-3 text-[12px] text-foreground">
                  <RefreshCw className="size-3.5 shrink-0 animate-spin" aria-hidden />
                  <span className="truncate">Reconnecting… scan again if the page was closed</span>
                </div>
              </div>
            )}

            {state.conn === "paused" && (
              <div className="absolute inset-x-0 top-3 flex justify-center px-4">
                <div className="flex h-8 max-w-full items-center gap-2 border border-border bg-black px-3 text-[12px] text-foreground">
                  <Pause className="size-3.5 shrink-0" aria-hidden />
                  <span className="truncate">{PAUSE_LABEL[state.pauseReason ?? "unknown"]}</span>
                </div>
              </div>
            )}
          </div>

          {/* Bottom bar: label chip left, icon button right */}
          <div className="absolute inset-x-3 bottom-3 flex items-center justify-between gap-3">
            <Badge className="h-6 bg-black px-2">
              {live ? <Video aria-hidden /> : <QrCode aria-hidden />}
              {CONN_LABEL[state.conn]}
            </Badge>
            <NewPairingButton />
          </div>
        </section>

        {/* Right: settings card with labeled groups */}
        <Card className="flex min-h-0 w-[300px] shrink-0 flex-col overflow-y-auto px-4 py-1">
          <ConnectionCard state={state} />
          <CameraCard state={state} />
          <DecoderCard state={state} />
        </Card>
      </main>

      <footer className="flex items-center justify-between gap-4 border-t border-border pt-3 text-[12px] text-muted-foreground">
        <span className="truncate">Same Wi-Fi network or USB cable</span>
        <span className="truncate">Keep the phone unlocked with the app open</span>
        <span className="shrink-0 font-mono text-[11px] tracking-[0.14em] uppercase">
          Preview max {PREVIEW_MAX_FPS} fps · v{__APP_VERSION__}
        </span>
      </footer>
    </div>
  );
}

/** Generating a new code unpairs the phone, so it takes two clicks within a few seconds. */
function NewPairingButton() {
  const [confirming, setConfirming] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const onClick = () => {
    window.clearTimeout(timer.current);
    if (!confirming) {
      setConfirming(true);
      timer.current = window.setTimeout(() => setConfirming(false), 4000);
      return;
    }
    setConfirming(false);
    void newPairing();
  };

  if (confirming) {
    return (
      <Button
        variant="outline"
        className="h-8 bg-black px-3 font-mono text-[11px] tracking-[0.14em] uppercase"
        onClick={onClick}
        aria-label="Confirm: new pairing code, the phone will be unpaired"
        title="Click again to unpair the phone and make a new code"
      >
        [ Unpair phone? ]
      </Button>
    );
  }
  return (
    <Button variant="outline" size="icon" className="bg-black" onClick={onClick} aria-label="New pairing code" title="New pairing code">
      <QrCode aria-hidden />
    </Button>
  );
}

/** One labeled group inside the settings card, separated by a hairline. */
function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2.5 border-t border-border py-3.5 first:border-t-0">
      <CardHeader>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardContent className="gap-2.5">{children}</CardContent>
    </section>
  );
}

function ConnectionCard({ state }: { state: AppState }) {
  const wifi = state.path === "lan";
  const usb = state.path === "usb";
  return (
    <Group title="[ CONNECTION ]">
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
      <Row label="Source">{state.source ? SOURCE_LABEL[state.source] : "—"}</Row>
      <Row label="Bitrate">{state.bitrateKbps ? `${(state.bitrateKbps / 1000).toFixed(1)} Mbps` : "—"}</Row>
      <Row label="Delay">{state.delayMs != null ? `${state.delayMs} ms` : "—"}</Row>
      <Row label="Phone battery">
        {state.battery != null ? (
          <span className="flex items-center gap-1.5">
            {state.charging ? <BatteryCharging className="size-3.5" aria-hidden /> : <Battery className="size-3.5" aria-hidden />}
            {(() => { const pct = Math.round(state.battery <= 1 ? state.battery * 100 : state.battery); return state.charging ? `${pct}% charging` : `${pct}%`; })()}
          </span>
        ) : (
          "—"
        )}
      </Row>
    </Group>
  );
}

const NEXT_ROTATION: Record<0 | 90 | 180 | 270, 0 | 90 | 180 | 270> = {
  0: 90,
  90: 180,
  180: 270,
  270: 0,
};

function CameraCard({ state }: { state: AppState }) {
  const { settings, lenses, zoomMin, zoomMax, torchAvailable } = state;
  const [zoomDraft, setZoomDraft] = useState<number | null>(null);
  const zoomValue = zoomDraft ?? settings.zoom;

  const patch = (p: SettingsPatch) => void setSettings(p);
  const rotation = (settings.rotation ?? 0) as 0 | 90 | 180 | 270;

  return (
    <Group title="[ CAMERA ]">
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
      <Button
        variant="default"
        size="sm"
        className="mt-0.5 h-8 w-full"
        onClick={() => patch({ rotation: NEXT_ROTATION[rotation] ?? 90 })}
      >
        <RotateCw aria-hidden />
        Rotate · {rotation}°
      </Button>
    </Group>
  );
}

function formatStreamStats(
  w: number | null | undefined,
  h: number | null | undefined,
  fps: number | null | undefined,
): string {
  if (!w || !h || !fps || w <= 0 || h <= 0 || fps <= 0) return "—";
  return `${w} × ${h} @ ${Math.round(fps)} fps`;
}

function formatResolution(w: number | null | undefined, h: number | null | undefined): string {
  if (!w || !h || w <= 0 || h <= 0) return "—";
  return `${w} × ${h}`;
}

function DecoderCard({ state }: { state: AppState }) {
  const { settings, decoderName, width, height, srcWidth, srcHeight, srcFps, phoneWidth, phoneHeight, phoneFps } = state;
  return (
    <Group title="[ STREAM ]">
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
          <Cpu className="size-3.5" aria-hidden />
          {decoderName ?? "—"}
        </span>
      </Row>
      <Row label="Phone sends">{formatStreamStats(phoneWidth, phoneHeight, phoneFps)}</Row>
      <Row label="Received">{formatStreamStats(srcWidth, srcHeight, srcFps)}</Row>
      <Row label="Camera output">{formatResolution(width, height)}</Row>
    </Group>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[12px] leading-none text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-h-[20px] items-center justify-between gap-3 text-[12px]">
      <span className="text-muted-foreground">{label}</span>
      <span className="text-right tabular-nums">{children}</span>
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
    <div className="flex min-h-[24px] items-center justify-between gap-3 text-[12px]">
      <div className="flex flex-col">
        <span>{label}</span>
        {hint && <span className="text-[11px] leading-tight text-muted-foreground">{hint}</span>}
      </div>
      <Switch checked={checked} disabled={disabled} onCheckedChange={onChange} aria-label={label} />
    </div>
  );
}
