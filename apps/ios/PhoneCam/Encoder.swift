import Foundation
import AVFoundation
import VideoToolbox

/// VideoToolbox H.264 encoder producing Annex-B access units. Not thread-safe:
/// call everything from the camera queue.
final class Encoder {
    /// (pts_us, keyframe, rot, annexB)
    var onAU: ((UInt64, Bool, UInt8, Data) -> Void)?

    private var session: VTCompressionSession?
    private var width = 0
    private var height = 0
    private var fps = 30
    private(set) var bitrate = 10_000_000
    private var forceKey = true
    private var lastKey = Date.distantPast

    static func bitrateFor(width: Int, height: Int, fps: Int) -> Int {
        let longSide = max(width, height)
        if longSide >= 3000 { return 25_000_000 }
        if longSide >= 1900 { return fps >= 50 ? 16_000_000 : 10_000_000 }
        return 5_000_000
    }

    func requestKeyframe() { forceKey = true }

    func setFps(_ f: Int) {
        if f != fps { fps = max(1, f); invalidate() }
    }

    func invalidate() {
        if let s = session {
            VTCompressionSessionCompleteFrames(s, untilPresentationTimeStamp: .invalid)
            VTCompressionSessionInvalidate(s)
        }
        session = nil
    }

    private func create(width w: Int, height h: Int) -> Bool {
        invalidate()
        var s: VTCompressionSession?
        let st = VTCompressionSessionCreate(
            allocator: nil, width: Int32(w), height: Int32(h), codecType: kCMVideoCodecType_H264,
            encoderSpecification: nil, imageBufferAttributes: nil, compressedDataAllocator: nil,
            outputCallback: nil, refcon: nil, compressionSessionOut: &s)
        guard st == noErr, let sess = s else { return false }
        bitrate = Encoder.bitrateFor(width: w, height: h, fps: fps)
        func set(_ k: CFString, _ v: CFTypeRef) { VTSessionSetProperty(sess, key: k, value: v) }
        set(kVTCompressionPropertyKey_RealTime, kCFBooleanTrue)
        set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse)
        set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel)
        set(kVTCompressionPropertyKey_AverageBitRate, NSNumber(value: bitrate))
        set(kVTCompressionPropertyKey_DataRateLimits, [NSNumber(value: bitrate * 3 / 2 / 8), NSNumber(value: 1)] as CFArray)
        set(kVTCompressionPropertyKey_ExpectedFrameRate, NSNumber(value: fps))
        set(kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, NSNumber(value: 2))
        set(kVTCompressionPropertyKey_MaxKeyFrameInterval, NSNumber(value: fps * 2))
        VTCompressionSessionPrepareToEncodeFrames(sess)
        session = sess
        width = w
        height = h
        forceKey = true
        return true
    }

    func encode(_ pb: CVPixelBuffer, pts: CMTime, rot: UInt8) {
        let w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb)
        if session == nil || w != width || h != height {
            guard create(width: w, height: h) else { return }
        }
        guard let sess = session else { return }
        var props: CFDictionary?
        if forceKey {
            props = [kVTEncodeFrameOptionKey_ForceKeyFrame as String: true] as CFDictionary
            forceKey = false
        }
        let cb = onAU
        VTCompressionSessionEncodeFrame(
            sess, imageBuffer: pb, presentationTimeStamp: pts, duration: .invalid,
            frameProperties: props, infoFlagsOut: nil
        ) { status, _, sbuf in
            guard status == noErr, let sb = sbuf, let (data, key) = Encoder.annexB(sb) else { return }
            let us = pts.seconds.isFinite ? UInt64(max(0, pts.seconds * 1_000_000)) : 0
            cb?(us, key, rot, data)
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
