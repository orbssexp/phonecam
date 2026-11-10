import Foundation
import AVFoundation
import UIKit

/// Thread-safe orientation holder (rot: x90 deg clockwise needed for an upright picture).
final class OrientationBox {
    static let shared = OrientationBox()
    private let lock = NSLock()
    private var rot: UInt8 = 1

    var value: UInt8 { lock.lock(); defer { lock.unlock() }; return rot }

    /// Call on the main thread.
    func refresh() {
        let r: UInt8?
        switch UIDevice.current.orientation {
        case .portrait: r = 1
        case .landscapeLeft: r = 0          // home button on the right = sensor native
        case .portraitUpsideDown: r = 3
        case .landscapeRight: r = 2
        default: r = nil                    // flat / unknown: keep the last one
        }
        if let r = r { lock.lock(); rot = r; lock.unlock() }
    }
}

struct CameraState {
    var lensId: String
    var width: Int
    var height: Int
    var fps: Double
    var zoomMax: Float
    var torch: Bool
}

/// AVFoundation capture. All mutating calls must run on `queue`.
final class Camera: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    let session = AVCaptureSession()
    let queue = DispatchQueue(label: "phonecam.camera")
    var onSample: ((CMSampleBuffer) -> Void)?

    private let output = AVCaptureVideoDataOutput()
    private(set) var device: AVCaptureDevice?
    private var wantW = 1920, wantH = 1080, wantFps = 30
    private(set) var actualFps: Double = 30
    private var configured = false

    // MARK: lenses

    static func discoverDevices() -> [AVCaptureDevice] {
        let types: [AVCaptureDevice.DeviceType] = [
            .builtInWideAngleCamera, .builtInUltraWideCamera, .builtInTelephotoCamera
        ]
        return AVCaptureDevice.DiscoverySession(deviceTypes: types, mediaType: .video, position: .unspecified).devices
    }

    static func label(_ d: AVCaptureDevice) -> String {
        let side = d.position == .front ? "Front" : "Back"
        switch d.deviceType {
        case .builtInUltraWideCamera: return side + " ultra wide"
        case .builtInTelephotoCamera: return side + " tele"
        default: return side + " wide"
        }
    }

    func lenses() -> [LensInfo] {
        let ds = Camera.discoverDevices()
        let sorted = ds.sorted { a, b in
            (a.position == .back ? 0 : 1, a.deviceType.rawValue) < (b.position == .back ? 0 : 1, b.deviceType.rawValue)
        }
        return sorted.map { LensInfo(id: $0.uniqueID, label: Camera.label($0)) }
    }

    // MARK: configuration

    func startRunning() { queue.async { self.startNow() } }

    /// Call on `queue`. No-op without camera permission.
    func startNow() {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else { return }
        if !configured { applyConfig(lensId: nil, width: 1920, height: 1080, fps: 30) }
        if !session.isRunning {
            DebugLog.log("capture start")
            session.startRunning()
        }
    }

    /// Call on `queue`: full stop/start (after interruption, runtime error, or no frames).
    func restartNow() {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else { return }
        DebugLog.log("capture restart")
        if session.isRunning { session.stopRunning() }
        startNow()
    }

    func stopRunning() {
        queue.async { if self.session.isRunning { self.session.stopRunning() } }
    }

    /// Switch lens and/or format. Keeps the session running (no TCP impact).
    func applyConfig(lensId: String?, width: Int, height: Int, fps: Int) {
        wantW = width; wantH = height; wantFps = fps
        let all = Camera.discoverDevices()
        var target: AVCaptureDevice? = nil
        if let id = lensId { target = all.first { $0.uniqueID == id } }
        if target == nil { target = device }
        if target == nil {
            target = all.first { $0.position == .back && $0.deviceType == .builtInWideAngleCamera } ?? all.first
        }
        guard let dev = target else { return }

        session.beginConfiguration()
        if !configured {
            if session.canSetSessionPreset(.inputPriority) { session.sessionPreset = .inputPriority }
            output.alwaysDiscardsLateVideoFrames = true
            output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
            output.setSampleBufferDelegate(self, queue: queue)
            if session.canAddOutput(output) { session.addOutput(output) }
            configured = true
        }
        if dev !== device {
            for i in session.inputs { session.removeInput(i) }
            if let input = try? AVCaptureDeviceInput(device: dev), session.canAddInput(input) {
                session.addInput(input)
                device = dev
            }
        }
        session.commitConfiguration()
        guard let cur = device else { return }

        if let (fmt, maxFps) = pickFormat(cur, w: width, h: height, fps: fps) {
            do {
                try cur.lockForConfiguration()
                cur.activeFormat = fmt
                let f = min(Double(fps), maxFps)
                let dur = CMTime(value: 1000, timescale: CMTimeScale(f * 1000))
                cur.activeVideoMinFrameDuration = dur
                cur.activeVideoMaxFrameDuration = dur
                actualFps = f
                cur.unlockForConfiguration()
            } catch {}
        }
        if let conn = output.connection(with: .video) {
            if conn.isVideoMirrored { conn.isVideoMirrored = false }
        }
    }

    private func pickFormat(_ d: AVCaptureDevice, w: Int, h: Int, fps: Int) -> (AVCaptureDevice.Format, Double)? {
        var best: (AVCaptureDevice.Format, Double)? = nil
        var bestScore = Double.greatestFiniteMagnitude
        let wantArea = Double(w * h)
        for f in d.formats {
            let sub = CMFormatDescriptionGetMediaSubType(f.formatDescription)
            guard sub == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange || sub == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange else { continue }
            let dims = CMVideoFormatDescriptionGetDimensions(f.formatDescription)
            let maxFps = f.videoSupportedFrameRateRanges.map { $0.maxFrameRate }.max() ?? 30
            let area = Double(Int(dims.width) * Int(dims.height))
            var score = abs(area - wantArea) / wantArea
            if maxFps < Double(fps) { score += 0.5 + (Double(fps) - maxFps) / Double(fps) }
            if sub == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange { score += 0.001 }
            if score < bestScore { bestScore = score; best = (f, maxFps) }
        }
        return best
    }

    func setZoom(_ z: Float) {
        guard let d = device else { return }
        let lo = Float(d.minAvailableVideoZoomFactor)
        let hi = min(Float(d.maxAvailableVideoZoomFactor), 16)
        let v = min(max(z, lo), hi)
        do { try d.lockForConfiguration(); d.videoZoomFactor = CGFloat(v); d.unlockForConfiguration() } catch {}
    }

    func setTorch(_ on: Bool) {
        guard let d = device, d.hasTorch, d.isTorchAvailable else { return }
        do { try d.lockForConfiguration(); d.torchMode = on ? .on : .off; d.unlockForConfiguration() } catch {}
    }

    func state() -> CameraState? {
        guard let d = device else { return nil }
        let dims = CMVideoFormatDescriptionGetDimensions(d.activeFormat.formatDescription)
        return CameraState(
            lensId: d.uniqueID, width: Int(dims.width), height: Int(dims.height), fps: actualFps,
            zoomMax: min(Float(d.activeFormat.videoMaxZoomFactor), 16), torch: d.hasTorch)
    }

    // MARK: delegate

    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        onSample?(sampleBuffer)
    }
}
