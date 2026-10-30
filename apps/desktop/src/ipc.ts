// Thin IPC layer. Rust owns all state; the UI only reads AppState and sends patches.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ConnState = "idle" | "connecting" | "live" | "paused" | "reconnecting";
export type PauseReason = "call" | "background" | "locked" | "unknown";
export type PathKind = "lan" | "usb";
export type Profile = "economy" | "balanced" | "quality";
export type DecoderKind = "auto" | "cuda" | "cpu";

export interface Lens {
  id: string;
  label: string;
}

export interface Settings {
  lens: string | null;
  profile: Profile;
  zoom: number;
  torch: boolean;
  mirror: boolean;
  decoder: DecoderKind;
}

export type SettingsPatch = Partial<Settings>;

export interface AppState {
  phoneUrl: string;
  qrSvg: string; // qrcode crate, SVG string
  conn: ConnState;
  pauseReason: PauseReason | null;
  path: PathKind | null;
  rttMs: number | null;
  battery: number | null;
  charging: boolean | null;
  decoderName: string | null;
  fps: number;
  width: number;
  height: number;
  lenses: Lens[];
  zoomMin: number;
  zoomMax: number;
  torchAvailable: boolean;
  settings: Settings;
}

const hasTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ---- Mock backend for `npm run dev` outside Tauri. ----
const QR_PLACEHOLDER =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 21 21" shape-rendering="crispEdges">' +
  '<rect width="21" height="21" fill="#ffffff"/>' +
  '<rect x="2" y="2" width="5" height="5" fill="#000"/><rect x="14" y="2" width="5" height="5" fill="#000"/>' +
  '<rect x="2" y="14" width="5" height="5" fill="#000"/><rect x="9" y="9" width="3" height="3" fill="#000"/>' +
  '<rect x="4" y="4" width="1" height="1" fill="#fff"/><rect x="15" y="4" width="1" height="1" fill="#fff"/>' +
  '<rect x="4" y="16" width="1" height="1" fill="#fff"/>' +
  "</svg>";

let mockState: AppState = {
  phoneUrl: "https://phonecam-eta.vercel.app/?room=mock",
  qrSvg: QR_PLACEHOLDER,
  conn: "idle",
  pauseReason: null,
  path: null,
  rttMs: null,
  battery: null,
  charging: null,
  decoderName: null,
  fps: 0,
  width: 0,
  height: 0,
  lenses: [
    { id: "back-wide", label: "Back Wide" },
    { id: "back-main", label: "Back Main" },
    { id: "front", label: "Front" },
  ],
  zoomMin: 1,
  zoomMax: 10,
  torchAvailable: true,
  settings: {
    lens: "back-main",
    profile: "balanced",
    zoom: 1,
    torch: false,
    mirror: false,
    decoder: "auto",
  },
};

const mockListeners = new Set<(s: AppState) => void>();
const mockEmit = () => mockListeners.forEach((cb) => cb(mockState));

// ---- Public wrappers ----
export function getState(): Promise<AppState> {
  if (!hasTauri()) return Promise.resolve(mockState);
  return invoke<AppState>("get_state");
}

export function setSettings(patch: SettingsPatch): Promise<AppState> {
  if (!hasTauri()) {
    mockState = { ...mockState, settings: { ...mockState.settings, ...patch } };
    mockEmit();
    return Promise.resolve(mockState);
  }
  return invoke<AppState>("set_settings", { patch });
}

export function newPairing(): Promise<AppState> {
  if (!hasTauri()) {
    mockState = { ...mockState, conn: "connecting", phoneUrl: `https://phonecam-eta.vercel.app/?room=${Math.random().toString(36).slice(2, 8)}` };
    mockEmit();
    return Promise.resolve(mockState);
  }
  return invoke<AppState>("new_pairing");
}

/** Base64 JPEG (max width 640), or null when no frame is available. */
export function getPreview(): Promise<string | null> {
  if (!hasTauri()) return Promise.resolve(null);
  return invoke<string | null>("get_preview");
}

export function onState(cb: (s: AppState) => void): Promise<UnlistenFn> {
  if (!hasTauri()) {
    mockListeners.add(cb);
    return Promise.resolve(() => {
      mockListeners.delete(cb);
    });
  }
  return listen<AppState>("state", (e) => cb(e.payload));
}
