import Foundation
import AVFoundation
import UIKit
import CoreMedia

/// Glue: camera -> encoder -> TCP server, and control handling.
final class AppModel: ObservableObject {
    @Published var status = "Not paired"
    @Published var paired = false
    @Published var showScanner = false
    @Published var showLog = false
    @Published var pairError: String?

    let camera = Camera()
    let encoder = Encoder()
    let server = NativeServer()

    // Touched on camera.queue only.
    private var streaming = false
    private var suspended = false          // scanner open: camera intentionally stopped
    private var capsPending = false
    private var sentFrames = 0
    private var sentBytes = 0
    private var statWindow = Date()
    private var measuredFps = 0
    private var measuredKbps = 0
    private var sentW = 0, sentH = 0
    private var lastRot: UInt8 = 1
    private var lastSampleAt = Date()
    private var lastRecover = Date.distantPast
    private var frameCounter = 0
    private var stride = 1                 // encode every Nth captured frame (adaptive when Wi-Fi is slow)
    private var lastDropAt = Date.distantPast
    private var watchdog: DispatchSourceTimer?

    // Net-queue stats, read on main.
    private var backlogMs = 0
    private var dropped = 0

    private var key: Data?
    private var connected = false
    private var started = false
    private var observers: [NSObjectProtocol] = []

    static var version: String {
        (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String) ?? "?"
    }

    init() {
        encoder.onAU = { [weak self] pts, keyframe, rot, data in
            guard let self = self else { return }
            self.server.sendVideo(ptsUs: pts, keyframe: keyframe, rot: rot, annexB: data)
            let n = data.count
            self.camera.queue.async { self.noteSent(bytes: n) }
        }
        camera.onSample = { [weak self] sb in self?.handleSample(sb) }

        server.onAuthed = { [weak self] in
            guard let self = self else { return }
            self.camera.queue.async {
                self.streaming = true
                self.lastSampleAt = Date()
                self.stride = 1
                // Encoding must be running whenever a desktop is authenticated.
                if !self.suspended { self.camera.startNow() }
                self.encoder.invalidate()
                self.encoder.requestKeyframe()
                self.server.setBitrate(self.currentBitrate())
                self.sendCaps()
            }
            DispatchQueue.main.async {
                self.connected = true
                UIApplication.shared.isIdleTimerDisabled = true
                self.status = "Connected"
            }
        }
        server.onClosed = { [weak self] in
            guard let self = self else { return }
            self.camera.queue.async { self.streaming = false }
            DispatchQueue.main.async {
                self.connected = false
                UIApplication.shared.isIdleTimerDisabled = false
                self.status = "Waiting for PC"
            }
        }
        server.onControl = { [weak self] ctl in self?.handleControl(ctl) }
        server.onNeedKeyframe = { [weak self] in
            guard let self = self else { return }
            self.camera.queue.async { self.encoder.requestKeyframe() }
        }
        server.onDropped = { [weak self] in
            guard let self = self else { return }
            // Wi-Fi cannot carry the target rate: encode fewer frames, stay live.
            self.camera.queue.async {
                self.lastDropAt = Date()
                if self.stride < 3 {
                    self.stride += 1
                    DebugLog.log("slow link: encoding every \(self.stride). frame")
                }
            }
        }
        server.onStats = { [weak self] ms, d in
            guard let self = self else { return }
            DispatchQueue.main.async { self.backlogMs = ms; self.dropped = d }
        }

        let t = DispatchSource.makeTimerSource(queue: camera.queue)
        t.schedule(deadline: .now() + 1, repeating: 1)
        t.setEventHandler { [weak self] in self?.tick() }
        t.resume()
        watchdog = t
    }

    func start() {
        guard !started else { return }     // onAppear can fire more than once
        started = true
        UIDevice.current.isBatteryMonitoringEnabled = true
        UIDevice.current.beginGeneratingDeviceOrientationNotifications()
        OrientationBox.shared.refresh()
        DebugLog.log("PhoneCam \(AppModel.version) start")
        let nc = NotificationCenter.default
        observers.append(nc.addObserver(forName: UIDevice.orientationDidChangeNotification, object: nil, queue: .main) { _ in
            OrientationBox.shared.refresh()
        })
        observers.append(nc.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
            DebugLog.log("background")
            self?.sendPaused("background")
        })
        observers.append(nc.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            self?.recover("became active")
        })
        observers.append(nc.addObserver(forName: .AVCaptureSessionWasInterrupted, object: nil, queue: .main) { [weak self] n in
            var reason = "unknown"
            if let r = n.userInfo?[AVCaptureSessionInterruptionReasonKey] as? Int {
                if r == AVCaptureSession.InterruptionReason.videoDeviceInUseByAnotherClient.rawValue ||
                    r == AVCaptureSession.InterruptionReason.audioDeviceInUseByAnotherClient.rawValue { reason = "call" }
                DebugLog.log("capture interrupted (reason \(r))")
            }
            self?.sendPaused(reason)
        })
        observers.append(nc.addObserver(forName: .AVCaptureSessionInterruptionEnded, object: nil, queue: .main) { [weak self] _ in
            self?.recover("interruption ended")
        })
        observers.append(nc.addObserver(forName: .AVCaptureSessionRuntimeError, object: nil, queue: .main) { [weak self] n in
            DebugLog.log("capture runtime error: \(String(describing: n.userInfo?[AVCaptureSessionErrorKey]))")
            self?.camera.queue.async { self?.camera.restartNow() }
            self?.recover("runtime error")
        })

        key = Keychain.load()
        paired = key != nil
        status = paired ? "Waiting for PC" : "Not paired"
        AVCaptureDevice.requestAccess(for: .video) { [weak self] ok in
            guard let self = self else { return }
            if ok { self.camera.startRunning() }
            else { DispatchQueue.main.async { self.status = "Camera access denied" } }
        }
        startServer()
    }

    private func startServer() {
        if let k = key { server.start(key: k, name: UIDevice.current.name) }
    }

    // MARK: pairing

    func resumeCamera() {
        camera.queue.async { self.suspended = false; self.camera.startNow() }
    }
    func suspendCamera() {
        camera.queue.async { self.suspended = true; if self.camera.session.isRunning { self.camera.session.stopRunning() } }
    }

    func handleScanned(_ text: String) -> Bool {
        guard let k = AppModel.parseKey(text) else { return false }
        Keychain.save(k)
        key = k
        paired = true
        pairError = nil
        status = "Waiting for PC"
        startServer()
        return true
    }

    /// Reads `k=<64 hex>` from the URL fragment (or the whole string as a fallback).
    static func parseKey(_ text: String) -> Data? {
        var frag = ""
        if let u = URLComponents(string: text), let f = u.fragment { frag = f }
        else if let i = text.firstIndex(of: "#") { frag = String(text[text.index(after: i)...]) }
        else { frag = text }
        for part in frag.split(separator: "&") {
            if part.hasPrefix("k=") {
                let hex = String(part.dropFirst(2))
                if hex.count == 64, let d = Wire.fromHex(hex), d.count == 32 { return d }
            }
        }
        return nil
    }

    // MARK: video

    private func handleSample(_ sb: CMSampleBuffer) {
        lastSampleAt = Date()
        guard streaming, let pb = CMSampleBufferGetImageBuffer(sb) else { return }
        // Adaptive stride: skip BEFORE the encoder so nothing ever queues behind a slow socket.
        frameCounter &+= 1
        if stride > 1 && frameCounter % stride != 0 { return }
        let pts = CMSampleBufferGetPresentationTimeStamp(sb)
        lastRot = OrientationBox.shared.value
        sentW = CVPixelBufferGetWidth(pb)
        sentH = CVPixelBufferGetHeight(pb)
        encoder.encode(pb, pts: pts, rot: lastRot)
    }

    private func noteSent(bytes: Int) {
        sentFrames += 1
        sentBytes += bytes
        let now = Date()
        let dt = now.timeIntervalSince(statWindow)
        if dt >= 1 {
            measuredFps = Int((Double(sentFrames) / dt).rounded())
            measuredKbps = Int(Double(sentBytes) * 8 / dt / 1000)
            sentFrames = 0
            sentBytes = 0
            statWindow = now
            let text = "Streaming \(sentW)x\(sentH) @ \(measuredFps) fps | \(measuredKbps) kbps"
            DispatchQueue.main.async {
                if self.connected {
                    self.status = text + " | lag \(self.backlogMs) ms | drop \(self.dropped)"
                }
            }
        }
    }

    private func currentBitrate() -> Int {
        if let s = camera.state() {
            return Encoder.bitrateFor(width: s.width, height: s.height, fps: Int(s.fps.rounded()))
        }
        return encoder.bitrate
    }

    /// 1 Hz on camera.queue: self-heal capture/encoder while a desktop is connected.
    private func tick() {
        guard streaming, !suspended else { return }
        let now = Date()
        if capsPending { sendCaps() }
        if stride > 1 && now.timeIntervalSince(lastDropAt) > 8 {
            stride -= 1
            lastDropAt = now
            DebugLog.log("link recovered: encoding every \(stride). frame")
        }
        guard now.timeIntervalSince(lastRecover) > 3 else { return }
        if !camera.session.isRunning {
            DebugLog.log("watchdog: capture not running")
            lastRecover = now
            camera.startNow()
            return
        }
        let sampleAge = now.timeIntervalSince(lastSampleAt)
        if sampleAge > 2.5 {
            DebugLog.log("watchdog: no frames for \(Int(sampleAge)) s, restarting capture")
            lastRecover = now
            camera.restartNow()
            encoder.invalidate()
            lastSampleAt = Date()
        } else if sampleAge < 1 && encoder.outputAge > 2.5 {
            DebugLog.log("watchdog: encoder stalled, rebuilding")
            lastRecover = now
            encoder.invalidate()
        }
    }

    /// Foreground / interruption ended: restart capture and encoder, then resumed + caps + keyframe.
    private func recover(_ why: String) {
        camera.queue.async {
            DebugLog.log("resume: \(why)")
            if !self.suspended { self.camera.startNow() }
            self.encoder.invalidate()
            self.encoder.requestKeyframe()
            self.lastSampleAt = Date()
            self.lastRecover = Date()
            if self.streaming {
                self.server.sendControl(Ctl(type: "resumed"))
                self.sendCaps()
            }
        }
    }

    // MARK: controls

    private func handleControl(_ ctl: Ctl) {
        switch ctl.type {
        case "ping":
            let b = Float(UIDevice.current.batteryLevel)
            let st = UIDevice.current.batteryState
            var p = Ctl(type: "pong")
            p.t = ctl.t ?? 0
            p.battery = b >= 0 ? b : nil
            p.charging = (st == .charging || st == .full)
            server.sendControl(p)
        case "keyframe":
            DebugLog.log("keyframe requested")
            camera.queue.async { self.encoder.requestKeyframe() }
        case "set":
            camera.queue.async { self.applySet(ctl) }
        default:
            break
        }
    }

    private static func mode(for profile: String) -> (Int, Int, Int)? {
        switch profile {
        case "economy": return (1280, 720, 30)
        case "balanced": return (1920, 1080, 30)
        case "quality": return (3840, 2160, 30)
        case "smooth", "fps60": return (1920, 1080, 60)
        default: return nil
        }
    }

    private func applySet(_ ctl: Ctl) {
        var w = 0, h = 0, f = 0
        if let s = camera.state() { w = s.width; h = s.height; f = Int(s.fps.rounded()) }
        var reconfigure = false
        if let p = ctl.profile, let m = AppModel.mode(for: p) { (w, h, f) = m; reconfigure = true }
        if ctl.lens != nil { reconfigure = true }
        if w == 0 { (w, h, f) = (1920, 1080, 30); reconfigure = true }
        DebugLog.log("set profile=\(ctl.profile ?? "-") lens=\(ctl.lens == nil ? "-" : "yes") zoom=\(ctl.zoom.map { String($0) } ?? "-") torch=\(ctl.torch.map { String($0) } ?? "-")")
        if reconfigure {
            DebugLog.log("reconfigure \(w)x\(h) @\(f)")
            camera.applyConfig(lensId: ctl.lens, width: w, height: h, fps: f)
            // Rebuild the compression session for the new size/fps (never mix sizes in one session).
            encoder.setFps(Int(camera.actualFps.rounded()))
            encoder.invalidate()
            stride = 1
            if !suspended { camera.startNow() }
            lastSampleAt = Date()
        }
        if let z = ctl.zoom { camera.setZoom(z) }
        if let t = ctl.torch { camera.setTorch(t) }
        encoder.requestKeyframe()
        server.setBitrate(currentBitrate())
        sendCaps()
    }

    /// Call on camera.queue.
    private func sendCaps() {
        guard let s = camera.state() else { capsPending = true; return }
        capsPending = false
        var c = Ctl(type: "caps")
        c.lenses = camera.lenses()
        c.current = s.lensId
        c.zoom_min = 1.0
        c.zoom_max = s.zoomMax
        c.torch = s.torch
        c.rotation = UInt16(lastRot) * 90
        c.width = UInt32(s.width)
        c.height = UInt32(s.height)
        c.fps = Float(s.fps)
        server.sendControl(c)
    }

    private func sendPaused(_ reason: String) {
        var c = Ctl(type: "paused")
        c.reason = reason
        server.sendControl(c)
    }
}
