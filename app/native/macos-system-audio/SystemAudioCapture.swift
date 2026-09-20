import CoreMedia
import Foundation
import ScreenCaptureKit

private let outputRate = 16_000.0
private let queueCapacity = 24

private func stderrLine(_ value: String) {
    let bytes = Array((value + "\n").utf8)
    bytes.withUnsafeBytes { raw in
        guard let base = raw.baseAddress else { return }
        _ = Darwin.write(STDERR_FILENO, base, raw.count)
    }
}

private func cleanError(_ error: Error) -> String {
    String(describing: error).replacingOccurrences(of: "\n", with: " ")
}

private final class LinearResampler {
    private var sourceRate = 0.0
    private var sourceBase: Int64 = 0
    private var nextPosition = 0.0
    private var previous: Float?

    func process(_ input: [Float], sourceRate newRate: Double) -> [Int16] {
        guard !input.isEmpty, newRate > 0 else { return [] }
        if abs(newRate - sourceRate) > 0.01 {
            sourceRate = newRate
            sourceBase = 0
            nextPosition = 0
            previous = nil
        }

        let first = sourceBase
        let last = first + Int64(input.count) - 1
        let step = sourceRate / outputRate
        var output: [Int16] = []
        output.reserveCapacity(Int(ceil(Double(input.count) / step)) + 1)

        while Int64(ceil(nextPosition)) <= last {
            let low = Int64(floor(nextPosition))
            let high = Int64(ceil(nextPosition))
            guard low >= first - 1 else {
                nextPosition += step
                continue
            }

            let lowValue: Float
            if low == first - 1, let previous {
                lowValue = previous
            } else {
                lowValue = input[Int(low - first)]
            }
            let highValue = input[Int(high - first)]
            let fraction = Float(nextPosition - Double(low))
            let sample = lowValue + (highValue - lowValue) * fraction
            let scaled = Int32((max(-1, min(1, sample)) * 32_767).rounded())
            output.append(Int16(clamping: scaled))
            nextPosition += step
        }

        previous = input.last
        sourceBase += Int64(input.count)
        return output
    }
}

private enum PCMError: Error {
    case noFormat
    case unsupportedFormat(String)
    case malformedBuffer
}

private struct AudioItem {
    let buffer: CMSampleBuffer
    let sourceStart: Int64
    let sourceFrames: Int
}

private final class AudioPipeline {
    private let condition = NSCondition()
    private var queue: [AudioItem] = []
    private var closed = false
    private var receivedSourceFrames: Int64 = 0
    private var completion: (() -> Void)?
    private let resampler = LinearResampler()

    init() {
        let thread = Thread { [weak self] in self?.run() }
        thread.name = "LectureEdit system audio writer"
        thread.qualityOfService = .userInitiated
        thread.start()
    }

    func enqueue(_ sampleBuffer: CMSampleBuffer) {
        condition.lock()
        defer { condition.unlock() }
        guard !closed else { return }
        let frameCount = CMSampleBufferGetNumSamples(sampleBuffer)
        let item = AudioItem(
            buffer: sampleBuffer,
            sourceStart: receivedSourceFrames,
            sourceFrames: frameCount
        )
        receivedSourceFrames += Int64(frameCount)
        if queue.count >= queueCapacity {
            return
        }
        queue.append(item)
        condition.signal()
    }

    func finish(_ completion: @escaping () -> Void) {
        condition.lock()
        if closed {
            condition.unlock()
            completion()
            return
        }
        closed = true
        self.completion = completion
        condition.broadcast()
        condition.unlock()
    }

    private func take() -> (AudioItem?, Int64, Bool) {
        condition.lock()
        while queue.isEmpty && !closed { condition.wait() }
        let total = receivedSourceFrames
        let item = queue.isEmpty ? nil : queue.removeFirst()
        let done = closed && queue.isEmpty && item == nil
        condition.unlock()
        return (item, total, done)
    }

    private func run() {
        var sourceCursor: Int64 = 0
        var outputCursor: Int64 = 0
        var lastRate = outputRate
        while true {
            let (item, total, done) = take()
            if done {
                let trailing = total - sourceCursor
                if trailing > 0 {
                    insertGap(sourceFrames: trailing, rate: lastRate, outputCursor: &outputCursor)
                }
                break
            }
            guard let item else { continue }
            let rate = sourceRate(of: item.buffer) ?? lastRate
            lastRate = rate
            let gap = item.sourceStart - sourceCursor
            if gap > 0 {
                insertGap(sourceFrames: gap, rate: rate, outputCursor: &outputCursor)
                sourceCursor += gap
            }
            do {
                let (mono, rate) = try decode(item.buffer)
                let converted = resampler.process(mono, sourceRate: rate)
                if !converted.isEmpty, !writeSamples(converted) {
                    stderrLine("ERROR:STDOUT:PCM consumer closed")
                    Foundation.exit(EX_IOERR)
                }
                outputCursor += Int64(converted.count)
            } catch {
                stderrLine("ERROR:AUDIO_FORMAT:\(cleanError(error))")
                insertGap(
                    sourceFrames: Int64(item.sourceFrames),
                    rate: rate,
                    outputCursor: &outputCursor
                )
            }
            sourceCursor = item.sourceStart + Int64(item.sourceFrames)
        }
        let callback = completion
        DispatchQueue.main.async { callback?() }
    }

    private func sourceRate(of sampleBuffer: CMSampleBuffer) -> Double? {
        guard let description = CMSampleBufferGetFormatDescription(sampleBuffer) else { return nil }
        return CMAudioFormatDescriptionGetStreamBasicDescription(description)?.pointee.mSampleRate
    }

    private func insertGap(sourceFrames: Int64, rate: Double, outputCursor: inout Int64) {
        guard sourceFrames > 0 else { return }
        let start = outputCursor
        var remaining = sourceFrames
        while remaining > 0 {
            let count = Int(min(remaining, 4_096))
            let zeros = [Float](repeating: 0, count: count)
            let converted = resampler.process(zeros, sourceRate: rate)
            if !converted.isEmpty, !writeSamples(converted) {
                stderrLine("ERROR:STDOUT:PCM consumer closed")
                Foundation.exit(EX_IOERR)
            }
            outputCursor += Int64(converted.count)
            remaining -= Int64(count)
        }
        if outputCursor > start { stderrLine("GAP:\(start):\(outputCursor)") }
    }

    private func decode(_ sampleBuffer: CMSampleBuffer) throws -> ([Float], Double) {
        guard let description = CMSampleBufferGetFormatDescription(sampleBuffer),
              let format = CMAudioFormatDescriptionGetStreamBasicDescription(description)?.pointee
        else { throw PCMError.noFormat }

        guard format.mFormatID == kAudioFormatLinearPCM else {
            throw PCMError.unsupportedFormat("format=\(format.mFormatID)")
        }
        let channels = Int(format.mChannelsPerFrame)
        guard channels > 0 else { throw PCMError.malformedBuffer }

        let frames = CMSampleBufferGetNumSamples(sampleBuffer)
        let bytesPerSample = Int(format.mBitsPerChannel / 8)
        let bytesPerFrame = Int(format.mBytesPerFrame)
        guard frames >= 0, bytesPerSample > 0, bytesPerFrame > 0 else {
            throw PCMError.malformedBuffer
        }

        let isFloat = (format.mFormatFlags & kAudioFormatFlagIsFloat) != 0
        let isSigned = (format.mFormatFlags & kAudioFormatFlagIsSignedInteger) != 0
        let isBigEndian = (format.mFormatFlags & kAudioFormatFlagIsBigEndian) != 0
        let isNonInterleaved = (format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
        var mono = [Float](repeating: 0, count: frames)
        try sampleBuffer.withAudioBufferList { buffers, _ in
            guard buffers.count >= (isNonInterleaved ? channels : 1) else {
                throw PCMError.malformedBuffer
            }
            for frame in 0..<frames {
                var sum: Float = 0
                for channel in 0..<channels {
                    let buffer = buffers[isNonInterleaved ? channel : 0]
                    let offset = isNonInterleaved
                        ? frame * bytesPerFrame
                        : frame * bytesPerFrame + channel * bytesPerSample
                    guard let base = buffer.mData,
                          offset + bytesPerSample <= Int(buffer.mDataByteSize) else {
                        throw PCMError.malformedBuffer
                    }
                    let pointer = base.advanced(by: offset)
                    let value: Float
                    switch (isFloat, isSigned, bytesPerSample) {
                    case (true, _, 4):
                        let bits = pointer.loadUnaligned(as: UInt32.self)
                        value = Float(bitPattern: isBigEndian ? bits.bigEndian : bits.littleEndian)
                    case (true, _, 8):
                        let bits = pointer.loadUnaligned(as: UInt64.self)
                        value = Float(Double(bitPattern: isBigEndian ? bits.bigEndian : bits.littleEndian))
                    case (false, true, 2):
                        let bits = pointer.loadUnaligned(as: UInt16.self)
                        value = Float(Int16(bitPattern: isBigEndian ? bits.bigEndian : bits.littleEndian)) / 32_768
                    case (false, true, 4):
                        let bits = pointer.loadUnaligned(as: UInt32.self)
                        value = Float(Int32(bitPattern: isBigEndian ? bits.bigEndian : bits.littleEndian)) / 2_147_483_648
                    default:
                        throw PCMError.unsupportedFormat("flags=\(format.mFormatFlags) bits=\(format.mBitsPerChannel)")
                    }
                    sum += value
                }
                mono[frame] = sum / Float(channels)
            }
        }
        return (mono, format.mSampleRate)
    }

    private func writeSamples(_ samples: [Int16]) -> Bool {
        var data = Data(count: samples.count * 2)
        data.withUnsafeMutableBytes { raw in
            let bytes = raw.bindMemory(to: UInt8.self)
            for (index, sample) in samples.enumerated() {
                let value = UInt16(bitPattern: sample).littleEndian
                bytes[index * 2] = UInt8(truncatingIfNeeded: value)
                bytes[index * 2 + 1] = UInt8(truncatingIfNeeded: value >> 8)
            }
        }
        return data.withUnsafeBytes { raw in
            guard let start = raw.baseAddress else { return true }
            var offset = 0
            while offset < raw.count {
                let count = Darwin.write(STDOUT_FILENO, start.advanced(by: offset), raw.count - offset)
                if count > 0 { offset += count; continue }
                if count < 0 && errno == EINTR { continue }
                return false
            }
            return true
        }
    }
}

@available(macOS 13.0, *)
private final class CaptureController: NSObject, SCStreamOutput, SCStreamDelegate {
    private let pipeline = AudioPipeline()
    private let callbackQueue = DispatchQueue(label: "com.lectureedit.system-audio.samples", qos: .userInteractive)
    private var stream: SCStream?
    private var stopping = false

    func start() {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: false) { [weak self] content, error in
            guard let self else { return }
            if let error { self.fail("SHAREABLE_CONTENT", error); return }
            guard let content, !content.displays.isEmpty else {
                self.failMessage("NO_DISPLAY", "No capturable display is available")
                return
            }

            let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays[0]
            let parentPID = pid_t(getppid())
            let excluded = content.applications.filter { $0.processID == parentPID }
            let filter = SCContentFilter(display: display, excludingApplications: excluded, exceptingWindows: [])
            let configuration = SCStreamConfiguration()
            configuration.width = 2
            configuration.height = 2
            configuration.minimumFrameInterval = CMTime(seconds: 1, preferredTimescale: 1)
            configuration.queueDepth = 3
            configuration.showsCursor = false
            configuration.capturesAudio = true
            configuration.sampleRate = Int(outputRate)
            configuration.channelCount = 1
            configuration.excludesCurrentProcessAudio = true

            let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
            do {
                try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: self.callbackQueue)
            } catch {
                self.fail("ADD_OUTPUT", error)
                return
            }
            self.stream = stream
            stream.startCapture { error in
                if let error { self.fail("START", error) }
                else { stderrLine("READY") }
            }
        }
    }

    func stop() {
        guard !stopping else { return }
        stopping = true
        guard let stream else { finish(); return }
        stream.stopCapture { [weak self] error in
            if let error { stderrLine("ERROR:STOP:\(cleanError(error))") }
            self?.finish()
        }
    }

    private func finish() {
        stream = nil
        pipeline.finish {
            stderrLine("STOPPED")
            Foundation.exit(EXIT_SUCCESS)
        }
    }

    private func fail(_ code: String, _ error: Error) {
        failMessage(code, cleanError(error))
    }

    private func failMessage(_ code: String, _ message: String) {
        stderrLine("ERROR:\(code):\(message)")
        stopping = true
        stream?.stopCapture { [weak self] _ in self?.finish() }
        if stream == nil { finish() }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of outputType: SCStreamOutputType) {
        guard outputType == .audio, sampleBuffer.isValid else { return }
        pipeline.enqueue(sampleBuffer)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        guard !stopping else { return }
        stderrLine("ERROR:STREAM:\(cleanError(error))")
        stopping = true
        finish()
    }
}

if CommandLine.arguments.count != 1 {
    stderrLine("ERROR:USAGE:This helper accepts no arguments")
    Foundation.exit(EX_USAGE)
}

guard #available(macOS 13.0, *) else {
    stderrLine("ERROR:UNSUPPORTED_OS:macOS 13 or newer is required")
    Foundation.exit(EX_OSERR)
}

private let controller = CaptureController()
signal(SIGPIPE, SIG_IGN)
signal(SIGTERM, SIG_IGN)
let termination = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
termination.setEventHandler { controller.stop() }
termination.resume()

DispatchQueue.global(qos: .utility).async {
    var byte: UInt8 = 0
    while true {
        let result = Darwin.read(STDIN_FILENO, &byte, 1)
        if result == 1 && byte != 10 { continue }
        if result < 0 && errno == EINTR { continue }
        DispatchQueue.main.async { controller.stop() }
        break
    }
}

controller.start()
dispatchMain()
