import Foundation
import AVFoundation
import VideoToolbox

/// VideoToolbox H.264 encoder producing Annex-B access units.
/// encode()/invalidate()/setFps() must be called from the camera queue; the output
/// handler runs on VideoToolbox threads and only touches lock-protected state.
final class Encoder {
    /// (pts_us, keyframe, rot, annexB)
    var onAU: ((UInt64, Bool, UInt8, Data) -> Void)?

    private var session: VTCompressionSession?
    private var width = 0
    private var height = 0
    private var fps = 30
    private(set) var bitrate = 10_000_000
    private var forceKey = true

    // Shared with VT threads.
    private let lock = NSLock()
    private var gen = 0
    private var inflight = 0
    private var failed = false
    private var lastOut = Date()

    /// Seconds since the last encoded frame came out.
    var outputAge: TimeInterval {
        lock.lock(); defer { lock.unlock() }
        return Date().timeIntervalSince(lastOut)
    }

    static func bitrateFor(width: Int, height: Int, fps: Int) -> Int {
        let longSide = max(width, height)
        if longSide >= 3000 { return 30_000_000 }
        if longSide >= 1900 { return fps >= 50 ? 16_000_000 : 10_000_000 }
        return 5_000_000
    }

    func requestKeyframe() { forceKey = true }

    func setFps(_ f: Int) {
        if f != fps { fps = max(1, f); invalidate() }
    }

    /// Tear the session down; the next encode() builds a fresh one (keyframe guaranteed).
    func invalidate() {
        let s = session
        session = nil
        if let s = s {
            VTCompressionSessionCompleteFrames(s, untilPresentationTimeStamp: .invalid)
            VTCompressionSessionInvalidate(s)
        }
        lock.lock()
        gen += 1
        inflight = 0
        failed = false
        lock.unlock()
        forceKey = true
    }

    private func create(width w: Int, height h: Int) -> Bool {
        invalidate()
        var s: VTCompressionSession?
        let st = VTCompressionSessionCreate(
            allocator: nil, width: Int32(w), height: Int32(h), codecType: kCMVideoCodecType_H264,
            encoderSpecification: nil, imageBufferAttributes: nil, compressedDataAllocator: nil,
            outputCallback: nil, refcon: nil, compressionSessionOut: &s)
        guard st == noErr, let sess = s else {
            DebugLog.log("encoder create failed \(st)")
            return false
        }
        bitrate = Encoder.bitrateFor(width: w, height: h, fps: fps)
        func set(_ k: CFString, _ v: CFTypeRef, _ label: String) {
            let r = VTSessionSetProperty(sess, key: k, value: v)
            if r != noErr { DebugLog.log("encoder prop \(label) failed \(r)") }
        }
        set(kVTCompressionPropertyKey_RealTime, kCFBooleanTrue, "RealTime")
        set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse, "Reorder")
        set(kVTCompressionPropertyKey_MaxFrameDelayCount, NSNumber(value: 0), "MaxFrameDelay")
        set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel, "Profile")
        set(kVTCompressionPropertyKey_H264EntropyMode, kVTH264EntropyMode_CABAC, "CABAC")
        set(kVTCompressionPropertyKey_AverageBitRate, NSNumber(value: bitrate), "AverageBitRate")
        set(kVTCompressionPropertyKey_DataRateLimits,
            [NSNumber(value: bitrate * 3 / 2 / 8), NSNumber(value: 1)] as CFArray, "DataRateLimits")
        set(kVTCompressionPropertyKey_ExpectedFrameRate, NSNumber(value: fps), "ExpectedFrameRate")
        set(kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, NSNumber(value: 2), "KeyIntervalDuration")
        set(kVTCompressionPropertyKey_MaxKeyFrameInterval, NSNumber(value: fps * 2), "KeyInterval")
        set(kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, kCFBooleanFalse, "SpeedOverQuality")
        set(kVTCompressionPropertyKey_MaximizePowerEfficiency, kCFBooleanFalse, "PowerEfficiency")
        VTCompressionSessionPrepareToEncodeFrames(sess)
        session = sess
        width = w
        height = h
        forceKey = true
        DebugLog.log("encoder \(w)x\(h) @\(fps) \(bitrate / 1000) kbps")
        return true
    }

    func encode(_ pb: CVPixelBuffer, pts: CMTime, rot: UInt8) {
        let w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb)
        lock.lock(); let bad = failed; lock.unlock()
        if bad {
            DebugLog.log("encoder session failed, rebuilding")
            invalidate()
        }
        if session == nil || w != width || h != height {
            guard create(width: w, height: h) else { return }
        }
        guard let sess = session else { return }
        // Never buffer: if the hardware encoder is behind, drop this input frame.
        lock.lock(); let busy = inflight >= 3; let myGen = gen; if !busy { inflight += 1 }; lock.unlock()
        if busy { return }
        var props: CFDictionary?
        if forceKey {
            props = [kVTEncodeFrameOptionKey_ForceKeyFrame as String: true] as CFDictionary
            forceKey = false
        }
        let cb = onAU
        let st = VTCompressionSessionEncodeFrame(
            sess, imageBuffer: pb, presentationTimeStamp: pts, duration: .invalid,
            frameProperties: props, infoFlagsOut: nil
        ) { [weak self] status, _, sbuf in
            guard let self = self else { return }
            self.lock.lock()
            let current = self.gen == myGen
            if current { self.inflight = max(0, self.inflight - 1) }
            if status != noErr && current { self.failed = true }
            self.lock.unlock()
            guard current, status == noErr, let sb = sbuf, let (data, key) = Encoder.annexB(sb) else { return }
            self.lock.lock(); self.lastOut = Date(); self.lock.unlock()
            let us = pts.seconds.isFinite ? UInt64(max(0, pts.seconds * 1_000_000)) : 0
            cb?(us, key, rot, data)
        }
        if st != noErr {
            DebugLog.log("encode error \(st) (invalid session?), rebuilding")
            invalidate()   // e.g. kVTInvalidSessionErr after background/lock
        }
    }

    /// AVCC -> Annex-B; SPS/PPS prepended on keyframes.
    static func annexB(_ sb: CMSampleBuffer) -> (Data, Bool)? {
        guard let fmt = CMSampleBufferGetFormatDescription(sb),
              let block = CMSampleBufferGetDataBuffer(sb) else { return nil }
        var isKey = true
        if let arr = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[CFString: Any]],
           let first = arr.first, let notSync = first[kCMSampleAttachmentKey_NotSync] as? Bool, notSync {
            isKey = false
        }
        let start: [UInt8] = [0, 0, 0, 1]
        var out = Data()
        var nalLen: Int32 = 4
        if isKey {
            var count = 0
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: &nalLen)
            for i in 0..<count {
                var ptr: UnsafePointer<UInt8>?
                var size = 0
                let r = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, parameterSetIndex: i, parameterSetPointerOut: &ptr, parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil)
                if r == noErr, let p = ptr {
                    out.append(contentsOf: start)
                    out.append(p, count: size)
                }
            }
        } else {
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil, parameterSetCountOut: nil, nalUnitHeaderLengthOut: &nalLen)
        }
        let hdr = Int(nalLen)
        var total = 0
        var base: UnsafeMutablePointer<Int8>?
        guard CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &total, dataPointerOut: &base) == kCMBlockBufferNoErr,
              let raw = base else { return nil }
        let p = UnsafeRawPointer(raw).assumingMemoryBound(to: UInt8.self)
        var off = 0
        while off + hdr <= total {
            var n = 0
            for i in 0..<hdr { n = n << 8 | Int(p[off + i]) }
            off += hdr
            if n <= 0 || off + n > total { break }
            out.append(contentsOf: start)
            out.append(p + off, count: n)
            off += n
        }
        return out.isEmpty ? nil : (out, isKey)
    }
}
