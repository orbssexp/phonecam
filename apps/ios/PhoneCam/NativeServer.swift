import Foundation
import Network

/// TCP listener + Bonjour advertisement + handshake + framing.
/// One ACTIVE desktop at a time: a newly authenticated connection replaces the current one.
final class NativeServer {
    let queue = DispatchQueue(label: "phonecam.net")

    /// Called (on the net queue) once a desktop is authenticated.
    var onAuthed: (() -> Void)?
    var onClosed: (() -> Void)?
    var onControl: ((Ctl) -> Void)?
    /// The encoder must produce a keyframe (backpressure drained / waiting for one).
    var onNeedKeyframe: (() -> Void)?
    var onListening: ((UInt16) -> Void)?
    /// Video was dropped because the socket is backed up (net queue).
    var onDropped: (() -> Void)?
    /// Once per second while a desktop is active: (backlog ms, dropped frames total).
    var onStats: ((Int, Int) -> Void)?

    private final class Peer {
        let conn: NWConnection
        let id: Int
        var buffer = Data()
        var authed = false
        var pcNonce: String?
        var phoneNonce: String?
        var timeout: DispatchWorkItem?
        var lastRx = Date()
        /// Bytes handed to NWConnection and not yet completed.
        var outstanding = 0
        var dropping = false
        var waitingKey = true
        var lastKeyAsk = Date.distantPast
        var closed = false
        var droppedFrames = 0
        init(conn: NWConnection, id: Int) { self.conn = conn; self.id = id }
    }

    private var key = Data()
    private var name = "iPhone"
    private var listener: NWListener?
    private var failCount = 0
    private var peers: [Peer] = []
    private var active: Peer?
    private var nextId = 1
    private var bitrate = 10_000_000
    private var watchdog: DispatchSourceTimer?

    init() {
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + 1, repeating: 1)
        t.setEventHandler { [weak self] in self?.tick() }
        t.resume()
        watchdog = t
    }

    /// Idempotent: keeps the listener (and its port) when nothing relevant changed.
    func start(key: Data, name: String) {
        queue.async {
            let changed = key != self.key || name != self.name
            if self.listener != nil && !changed { return }
            if changed {
                self.closeAll("re-paired")
                self.listener?.cancel(); self.listener = nil
            }
            self.key = key
            self.name = name
            if self.listener == nil { self.makeListener(port: 47810) }
        }
    }

    func stop() {
        queue.async {
            self.closeAll("stop")
            self.listener?.cancel(); self.listener = nil
        }
    }

    func setBitrate(_ b: Int) { queue.async { self.bitrate = max(1_000_000, b) } }

    // MARK: listener

    private func makeListener(port: UInt16?) {
        let params = NWParameters.tcp
        params.allowLocalEndpointReuse = true
        if let tcp = params.defaultProtocolStack.transportProtocol as? NWProtocolTCP.Options {
            tcp.noDelay = true
            tcp.enableKeepalive = true
            tcp.keepaliveIdle = 5
            tcp.keepaliveInterval = 2
            tcp.keepaliveCount = 3
        }
        let l: NWListener
        do {
            if let p = port, let np = NWEndpoint.Port(rawValue: p) {
                l = try NWListener(using: params, on: np)
            } else {
                l = try NWListener(using: params)
            }
        } catch {
            DebugLog.log("listen create failed: \(error)")
            if port != nil { makeListener(port: nil) }
            return
        }
        var txt = NWTXTRecord()
        txt["v"] = "1"
        txt["id"] = Wire.serviceId(key: key)
        txt["name"] = name
        l.service = NWListener.Service(name: name, type: "_phonecam._tcp", txtRecord: txt)
        l.stateUpdateHandler = { [weak self, weak l] state in
            guard let self = self, let l = l, l === self.listener else { return }
            switch state {
            case .ready:
                self.failCount = 0
                let p = l.port?.rawValue ?? 0
                DebugLog.log("listening on \(p)")
                self.onListening?(p)
            case .waiting(let e):
                DebugLog.log("listener waiting: \(e)")
            case .failed(let e):
                DebugLog.log("listener failed: \(e)")
                l.cancel()
                self.listener = nil
                self.failCount += 1
                // Retry the preferred port twice, then any port.
                let p: UInt16? = self.failCount <= 2 ? 47810 : nil
                self.queue.asyncAfter(deadline: .now() + 1) {
                    if self.listener == nil && !self.key.isEmpty { self.makeListener(port: p) }
                }
            default: break
            }
        }
        l.newConnectionHandler = { [weak self] c in self?.accept(c) }
        listener = l
        l.start(queue: queue)
    }

    // MARK: peers

    private func accept(_ c: NWConnection) {
        let pending = peers.filter { !$0.authed }
        if pending.count >= 4, let oldest = pending.first { close(oldest, "too many pending") }
        let p = Peer(conn: c, id: nextId)
        nextId += 1
        peers.append(p)
        DebugLog.log("conn #\(p.id) accepted")
        c.stateUpdateHandler = { [weak self, weak p] st in
            guard let self = self, let p = p else { return }
            switch st {
            case .failed(let e): self.close(p, "conn failed: \(e)")
            case .cancelled: self.close(p, "cancelled")
            default: break
            }
        }
        c.start(queue: queue)
        let w = DispatchWorkItem { [weak self, weak p] in
            guard let self = self, let p = p, !p.authed else { return }
            self.close(p, "handshake timeout")
        }
        p.timeout = w
        queue.asyncAfter(deadline: .now() + 5, execute: w)
        receiveLoop(p)
    }

    private func close(_ p: Peer, _ reason: String) {
        if p.closed { return }
        p.closed = true
        p.timeout?.cancel(); p.timeout = nil
        peers.removeAll { $0 === p }
        p.conn.cancel()
        p.buffer = Data()
        DebugLog.log("close #\(p.id): \(reason)")
        if active === p {
            active = nil
            onClosed?()
        }
    }

    private func closeAll(_ reason: String) {
        for p in peers { close(p, reason) }
    }

    private func tick() {
        guard let p = active else { return }
        if Date().timeIntervalSince(p.lastRx) > 6 { close(p, "no inbound data for 6 s"); return }
        let ms = p.outstanding * 8 * 1000 / bitrate
        onStats?(ms, p.droppedFrames)
    }

    private func receiveLoop(_ p: Peer) {
        p.conn.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, isComplete, error in
            guard let self = self, !p.closed else { return }
            if let d = data, !d.isEmpty {
                p.lastRx = Date()
                p.buffer.append(d)
                if !self.parse(p) { self.close(p, "protocol violation"); return }
            }
            if let e = error { self.close(p, "recv error: \(e)"); return }
            if isComplete { self.close(p, "peer closed"); return }
            self.receiveLoop(p)
        }
    }

    /// Returns false on a protocol violation.
    private func parse(_ p: Peer) -> Bool {
        while p.buffer.count >= 4 {
            let h = Array(p.buffer.prefix(4))
            let len = Int(UInt32(h[0]) << 24 | UInt32(h[1]) << 16 | UInt32(h[2]) << 8 | UInt32(h[3]))
            if len < 1 || len > Wire.maxFrame { return false }
            if p.buffer.count < 4 + len { return true }
            let body = Array(p.buffer.prefix(4 + len).dropFirst(4))
            p.buffer = Data(p.buffer.dropFirst(4 + len))
            let type = body[0]
            let payload = Data(body.dropFirst())
            if type == Wire.typeControl {
                if !handleControl(p, payload) { return false }
                if p.closed { return true }
            }
            // Other frame types from the desktop are ignored.
        }
        return true
    }

    private func handleControl(_ p: Peer, _ payload: Data) -> Bool {
        let dec = JSONDecoder()
        if !p.authed {
            guard let m = try? dec.decode(HandshakeMsg.self, from: payload) else { return false }
            switch m.t {
            case "hello":
                guard p.pcNonce == nil, m.v == 1, let n = m.nonce, !n.isEmpty, n.count <= 128 else { return false }
                p.pcNonce = n
                let ph = Wire.randomNonceHex()
                p.phoneNonce = ph
                let reply = HandshakeMsg(t: "auth", v: nil, nonce: ph, proof: Wire.phoneProof(key: key, pcNonce: n, phoneNonce: ph))
                guard let data = try? JSONEncoder().encode(reply) else { return false }
                sendRaw(p, Wire.frame(type: Wire.typeControl, payload: data), counted: false)
                return true
            case "auth_ok":
                guard let pc = p.pcNonce, let ph = p.phoneNonce, let proof = m.proof else { return false }
                guard Wire.safeEqual(proof, Wire.desktopProof(key: key, pcNonce: pc, phoneNonce: ph)) else {
                    DebugLog.log("conn #\(p.id): bad desktop proof")
                    return false
                }
                p.authed = true
                p.timeout?.cancel(); p.timeout = nil
                if let old = active, old !== p { close(old, "replaced by new desktop connection") }
                active = p
                p.waitingKey = true
                p.dropping = false
                p.outstanding = 0
                DebugLog.log("auth ok #\(p.id)")
                onAuthed?()
                return true
            default:
                return false
            }
        }
        guard p === active else { return true }
        guard let ctl = try? dec.decode(Ctl.self, from: payload) else { return true } // unknown: ignore
        onControl?(ctl)
        return true
    }

    // MARK: sending

    func sendControl(_ ctl: Ctl) {
        queue.async {
            guard let p = self.active, let data = try? JSONEncoder().encode(ctl) else { return }
            self.sendRaw(p, Wire.frame(type: Wire.typeControl, payload: data), counted: false)
        }
    }

    /// Never blocks the caller: hops to the net queue and drops when the socket is backed up.
    /// The unsent backlog (bytes handed to NWConnection, not yet completed) is bounded to
    /// ~200 ms of video; beyond that we drop until it drains and resync on a keyframe.
    func sendVideo(ptsUs: UInt64, keyframe: Bool, rot: UInt8, annexB: Data) {
        queue.async {
            guard let p = self.active, annexB.count + 11 < Wire.maxFrame else { return }
            let now = Date()
            // 200 ms of video, but never less than a typical keyframe so one IDR cannot self-trigger a drop.
            let limit = max(self.bitrate / 8 / 5, 600_000)
            if p.dropping {
                p.droppedFrames += 1
                if p.outstanding < limit / 4 {
                    p.dropping = false
                    p.waitingKey = true
                    p.lastKeyAsk = now
                    DebugLog.log("backlog drained, asking keyframe (dropped \(p.droppedFrames) total)")
                    self.onNeedKeyframe?()
                }
                return
            }
            if p.outstanding > limit && !keyframe {
                p.dropping = true
                p.droppedFrames += 1
                DebugLog.log("backlog \(p.outstanding / 1000) kB > limit \(limit / 1000) kB: dropping to next keyframe")
                self.onDropped?()
                return
            }
            if p.waitingKey {
                if !keyframe {
                    p.droppedFrames += 1
                    if now.timeIntervalSince(p.lastKeyAsk) > 1 {
                        p.lastKeyAsk = now
                        self.onNeedKeyframe?()
                    }
                    return
                }
                p.waitingKey = false
            }
            let payload = Wire.videoPayload(ptsUs: ptsUs, keyframe: keyframe, rot: rot, annexB: annexB)
            self.sendRaw(p, Wire.frame(type: Wire.typeVideo, payload: payload), counted: true)
        }
    }

    private func sendRaw(_ p: Peer, _ data: Data, counted: Bool) {
        if p.closed { return }
        let n = data.count
        if counted { p.outstanding += n }
        p.conn.send(content: data, completion: .contentProcessed { [weak self] err in
            guard let self = self else { return }
            if counted { p.outstanding = max(0, p.outstanding - n) }
            if let e = err { self.close(p, "send error: \(e)") }
        })
    }
}
