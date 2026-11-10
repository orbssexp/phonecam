import Foundation
import Network

/// TCP listener + Bonjour advertisement + handshake + framing. One desktop at a time.
final class NativeServer {
    let queue = DispatchQueue(label: "phonecam.net")

    /// Called (on the net queue) once the desktop is authenticated.
    var onAuthed: (() -> Void)?
    var onClosed: (() -> Void)?
    var onControl: ((Ctl) -> Void)?
    /// Backpressure drained: the encoder must produce a keyframe.
    var onNeedKeyframe: (() -> Void)?
    var onListening: ((UInt16) -> Void)?

    private var key = Data()
    private var name = "iPhone"
    private var listener: NWListener?
    private var conn: NWConnection?
    private var buffer = Data()
    private var authed = false
    private var pcNonce: String?
    private var phoneNonce: String?
    private var timeout: DispatchWorkItem?

    private var outstanding = 0
    private var dropping = false
    private var waitingKey = true
    var bitrate: Int = 10_000_000

    func start(key: Data, name: String) {
        queue.async {
            self.stopLocked()
            self.key = key
            self.name = name
            self.makeListener(port: 47810)
        }
    }

    func stop() { queue.async { self.stopLocked() } }

    private func stopLocked() {
        listener?.cancel(); listener = nil
        closeConn()
    }

    private func makeListener(port: UInt16?) {
        let params = NWParameters.tcp
        if let tcp = params.defaultProtocolStack.transportProtocol as? NWProtocolTCP.Options {
            tcp.noDelay = true
        }
        let l: NWListener
        do {
            if let p = port, let np = NWEndpoint.Port(rawValue: p) {
                l = try NWListener(using: params, on: np)
            } else {
                l = try NWListener(using: params)
            }
        } catch {
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
                self.onListening?(l.port?.rawValue ?? 0)
            case .failed:
                l.cancel()
                self.listener = nil
                // Preferred port busy (or other failure): fall back to any port.
                if port != nil {
                    self.makeListener(port: nil)
                } else {
                    self.queue.asyncAfter(deadline: .now() + 2) {
                        if self.listener == nil && !self.key.isEmpty { self.makeListener(port: nil) }
                    }
                }
            default: break
            }
        }
        l.newConnectionHandler = { [weak self] c in
            self?.accept(c)
        }
        listener = l
        l.start(queue: queue)
    }

    private func accept(_ c: NWConnection) {
        if conn != nil { c.cancel(); return }   // one desktop at a time
        conn = c
        buffer = Data()
        authed = false
        pcNonce = nil
        phoneNonce = nil
        outstanding = 0
        dropping = false
        waitingKey = true
        c.stateUpdateHandler = { [weak self, weak c] st in
            guard let self = self, let c = c, c === self.conn else { return }
            switch st {
            case .failed, .cancelled: self.closeConn()
            default: break
            }
        }
        c.start(queue: queue)
        let w = DispatchWorkItem { [weak self] in
            guard let self = self, !self.authed else { return }
            self.closeConn()
        }
        timeout = w
        queue.asyncAfter(deadline: .now() + 10, execute: w)
        receiveLoop(c)
    }

    private func closeConn() {
        timeout?.cancel(); timeout = nil
        guard let c = conn else { return }
        conn = nil
        let was = authed
        authed = false
        c.cancel()
        buffer = Data()
        if was { onClosed?() }
    }

    private func receiveLoop(_ c: NWConnection) {
        c.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, isComplete, error in
            guard let self = self, c === self.conn else { return }
            if let d = data, !d.isEmpty {
                self.buffer.append(d)
                if !self.parse() { self.closeConn(); return }
            }
            if isComplete || error != nil { self.closeConn(); return }
            self.receiveLoop(c)
        }
    }

    /// Returns false on a protocol violation.
    private func parse() -> Bool {
        while buffer.count >= 4 {
            let h = Array(buffer.prefix(4))
            let len = Int(UInt32(h[0]) << 24 | UInt32(h[1]) << 16 | UInt32(h[2]) << 8 | UInt32(h[3]))
            if len < 1 || len > Wire.maxFrame { return false }
            if buffer.count < 4 + len { return true }
            let body = Array(buffer.prefix(4 + len).dropFirst(4))
            buffer = Data(buffer.dropFirst(4 + len))
            let type = body[0]
            let payload = Data(body.dropFirst())
            if type == Wire.typeControl {
                if !handleControl(payload) { return false }
            }
            // Other frame types from the desktop are ignored.
        }
        return true
    }

    private func handleControl(_ payload: Data) -> Bool {
        let dec = JSONDecoder()
        if !authed {
            guard let m = try? dec.decode(HandshakeMsg.self, from: payload) else { return false }
            switch m.t {
            case "hello":
                guard pcNonce == nil, m.v == 1, let n = m.nonce, !n.isEmpty, n.count <= 128 else { return false }
                pcNonce = n
                let ph = Wire.randomNonceHex()
                phoneNonce = ph
                let reply = HandshakeMsg(t: "auth", v: nil, nonce: ph, proof: Wire.phoneProof(key: key, pcNonce: n, phoneNonce: ph))
                guard let data = try? JSONEncoder().encode(reply) else { return false }
                sendRaw(Wire.frame(type: Wire.typeControl, payload: data), counted: false)
                return true
            case "auth_ok":
                guard let pc = pcNonce, let ph = phoneNonce, let proof = m.proof else { return false }
                guard Wire.safeEqual(proof, Wire.desktopProof(key: key, pcNonce: pc, phoneNonce: ph)) else { return false }
                authed = true
                timeout?.cancel(); timeout = nil
                onAuthed?()
                return true
            default:
                return false
            }
        }
        guard let ctl = try? dec.decode(Ctl.self, from: payload) else { return true } // unknown: ignore
        onControl?(ctl)
        return true
    }

    // MARK: sending

    func sendControl(_ ctl: Ctl) {
        queue.async {
            guard self.authed, let data = try? JSONEncoder().encode(ctl) else { return }
            self.sendRaw(Wire.frame(type: Wire.typeControl, payload: data), counted: false)
        }
    }

    func sendVideo(ptsUs: UInt64, keyframe: Bool, rot: UInt8, annexB: Data) {
        queue.async {
            guard self.authed, annexB.count + 11 < Wire.maxFrame else { return }
            // Backpressure: more than ~1 s of video queued -> drop until drained.
            let queuedBits = self.outstanding * 8
            if self.dropping {
                if queuedBits < self.bitrate / 5 {
                    self.dropping = false
                    self.waitingKey = true
                    self.onNeedKeyframe?()
                }
                return
            }
            if queuedBits > self.bitrate {
                self.dropping = true
                return
            }
            if self.waitingKey {
                if !keyframe { return }
                self.waitingKey = false
            }
            let p = Wire.videoPayload(ptsUs: ptsUs, keyframe: keyframe, rot: rot, annexB: annexB)
            self.sendRaw(Wire.frame(type: Wire.typeVideo, payload: p), counted: true)
        }
    }

    private func sendRaw(_ data: Data, counted: Bool) {
        guard let c = conn else { return }
        let n = data.count
        if counted { outstanding += n }
        c.send(content: data, completion: .contentProcessed { [weak self] err in
            guard let self = self else { return }
            if counted { self.outstanding = max(0, self.outstanding - n) }
            if err != nil, c === self.conn { self.closeConn() }
        })
    }
}
