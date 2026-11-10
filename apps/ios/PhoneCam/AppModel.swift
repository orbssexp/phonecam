import Foundation
import AVFoundation
import UIKit
import CoreMedia

/// Glue: camera -> encoder -> TCP server, and control handling.
final class AppModel: ObservableObject {
    @Published var status = "Not paired"
    @Published var paired = false
    @Published var showScanner = false
    @Published var pairError: String?

    let camera = Camera()
    let encoder = Encoder()
    let server = NativeServer()

    // Touched on camera.queue only.
    private var streaming = false
    private var sentFrames = 0
    private var statWindow = Date()
    private var measuredFps = 0
    private var sentW = 0, sentH = 0
    private var lastRot: UInt8 = 1

    private var key: Data?
    private var connected = false
    private var observers: [NSObjectProtocol] = []

    init() {
        encoder.onAU = { [weak self] pts, keyframe, rot, data in
            guard let self = self else { return }
            self.server.sendVideo(ptsUs: pts, keyframe: keyframe, rot: rot, annexB: data)
            self.camera.queue.async { self.noteSent() }
        }
        camera.onSample = { [weak self] sb in self?.handleSample(sb) }

        server.onAuthed = { [weak self] in
            guard let self = self else { return }
            self.camera.queue.async {
                self.streaming = true
                self.encoder.requestKeyframe()
                self.server.bitrate = self.encoder.bitrate
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
    }

    func start() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        UIDevice.current.beginGeneratingDeviceOrientationNotifications()
        OrientationBox.shared.refresh()
        let nc = NotificationCenter.default
        observers.append(nc.addObserver(forName: UIDevice.orientationDidChangeNotification, object: nil, queue: .main) { _ in
            OrientationBox.shared.refresh()
        })
        observers.append(nc.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.sendPaused("background")
        })
        observers.append(nc.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.sendResumed()
        })
        observers.append(nc.addObserver(forName: .AVCaptureSessionWasInterrupted, object: nil, queue: .main) { [weak self] n in
            var reason = "unknown"
            if let r = n.userInfo?[AVCaptureSessionInterruptionReasonKey] as? Int {
                if r == AVCaptureSession.InterruptionReason.videoDeviceInUseByAnotherClient.rawValue ||
                    r == AVCaptureSession.InterruptionReason.audioDeviceInUseByAnotherClient.rawValue { reason = "call" }
            }
            self?.sendPaused(reason)
        })
        observers.append(nc.addObserver(forName: .AVCaptureSessionInterruptionEnded, object: nil, queue: .main) { [weak self] _ in
            self?.sendResumed()
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

    func resumeCamera() { camera.startRunning() }
    func suspendCamera() { camera.stopRunning() }

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
        guard streaming, let pb = CMSampleBufferGetImageBuffer(sb) else { return }
        let pts = CMSampleBufferGetPresentationTimeStamp(sb)
        lastRot = OrientationBox.shared.value
        sentW = CVPixelBufferGetWidth(pb)
        sentH = CVPixelBufferGetHeight(pb)
        encoder.encode(pb, pts: pts, rot: lastRot)
    }

    private func noteSent() {
        sentFrames += 1
        let now = Date()
        if now.timeIntervalSince(statWindow) >= 1 {
            measuredFps = Int((Double(sentFrames) / now.timeIntervalSince(statWindow)).rounded())
            sentFrames = 0
            statWindow = now
            let text = "Streaming \(sentW)x\(sentH) @ \(measuredFps) fps"
            DispatchQueue.main.async { if self.connected { self.status = text } }
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
        if reconfigure {
            camera.applyConfig(lensId: ctl.lens, width: w, height: h, fps: f)
            encoder.setFps(Int(camera.actualFps.rounded()))
        }
        if let z = ctl.zoom { camera.setZoom(z) }
        if let t = ctl.torch { camera.setTorch(t) }
        encoder.requestKeyframe()
        server.bitrate = Encoder.bitrateFor(width: w, height: h, fps: Int(camera.actualFps.rounded()))
        sendCaps()
    }

    /// Call on camera.queue.
    private func sendCaps() {
        guard let s = camera.state() else { return }
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

    private func sendResumed() {
        server.sendControl(Ctl(type: "resumed"))
        camera.queue.async { self.encoder.requestKeyframe() }
    }
}
