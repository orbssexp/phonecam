// PhoneCam signaling relay: forwards SDP/ICE text between exactly two peers
// (roles "desktop" and "phone") in a room. No message content is stored.
export interface Env {
  ROOM: DurableObjectNamespace;
}

const ID_RE = /^[A-Za-z0-9_-]{22,128}$/;
const MAX_MSG = 64 * 1024;

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const m = url.pathname.match(/^\/room\/([^/]+)$/);
    if (!m) return new Response("phonecam relay", { status: 404 });
    if (!ID_RE.test(m[1])) return new Response("bad room id", { status: 400 });
    const role = url.searchParams.get("role");
    if (role !== "desktop" && role !== "phone") return new Response("bad role", { status: 400 });
    if (req.headers.get("Upgrade") !== "websocket") return new Response("expected websocket", { status: 426 });
    const stub = env.ROOM.get(env.ROOM.idFromName(m[1]));
    return stub.fetch(req);
  },
};

export class Room implements DurableObject {
  constructor(private ctx: DurableObjectState, _env: Env) {}

  async fetch(req: Request): Promise<Response> {
    const role = new URL(req.url).searchParams.get("role")!;
    const other = role === "desktop" ? "phone" : "desktop";
    if (this.ctx.getWebSockets().length >= 2 || this.ctx.getWebSockets(role).length >= 1) {
      return new Response("room full", { status: 409 });
    }
    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    this.ctx.acceptWebSocket(server, [role]);
    const otherUp = this.ctx.getWebSockets(other).length > 0;
    server.send(JSON.stringify({ t: "peer", up: otherUp }));
    for (const ws of this.ctx.getWebSockets(other)) ws.send(JSON.stringify({ t: "peer", up: true }));
    return new Response(null, { status: 101, webSocket: client });
  }

  private otherOf(ws: WebSocket): WebSocket[] {
    const role = this.ctx.getTags(ws)[0];
    return this.ctx.getWebSockets(role === "desktop" ? "phone" : "desktop");
  }

  webSocketMessage(ws: WebSocket, msg: string | ArrayBuffer): void {
    if (typeof msg !== "string" || msg.length > MAX_MSG) {
      ws.close(1009, "text only, max 64KB");
      return;
    }
    for (const o of this.otherOf(ws)) o.send(msg);
  }

  private gone(ws: WebSocket): void {
    for (const o of this.otherOf(ws)) {
      try { o.send(JSON.stringify({ t: "peer", up: false })); } catch {}
    }
  }
  webSocketClose(ws: WebSocket, code: number): void {
    this.gone(ws);
    try { ws.close(code === 1005 || code === 1006 ? 1000 : code); } catch {}
  }
  webSocketError(ws: WebSocket): void { this.gone(ws); }
}
