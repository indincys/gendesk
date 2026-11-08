// gendesk-live：静态图 → 手机随手拍感觉的实况照片（同名 JPG + MOV 配对）。
//
// GenDesk 随包携带，验收通过后逐张调用。运动回放自真实实况的轨迹库：快门前一段和快门后
// 一段各取自不同的实拍，再随机左右镜像、调幅度，所以每张都不一样。只模拟相机转动和前后
// 推拉，每帧是一个整体的透视变换，不做局部变形；帧时间照搬实拍（快门附近会掉帧卡一下）。
// 每张随机取一段实拍原声。输出 3:4 竖图（原图居中裁切），视频 1440×1920 HEVC；静态图至少
// 3024×4032，原图更大时保留原尺寸。两边写入同一个内容标识符，MOV 另有 still-image-time
// 元数据轨道，「照片」与 Poster 据此认出配对。
//
// 用法: gendesk-live --resources <目录> <输出目录> <原图> [--seed N] [--amp 0.75] [--no-audio]
//   <目录>/motion/*.json 轨迹库，<目录>/audio/*.m4a 实拍原声。成功时在 stdout 打印配对编号。
import Accelerate
import AVFoundation
import CoreImage
import Foundation
import ImageIO
import UniformTypeIdentifiers
import simd

let sub = 6                 // 每帧子采样数，用来合成运动模糊
let exposure = 1.0 / 60     // 每帧曝光时长
let maxZoom = 1.15          // 为不露黑边允许的最大放大；超了就把快门前那段幅度压小
let videoW = 1440, videoH = 1920
let stillW = 3024, stillH = 4032

struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ d: String) { description = d }
}

// MARK: - 随机数（可复现）

struct Rng {
    var state: UInt64
    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
    mutating func uniform() -> Double { Double(next() >> 11) / Double(1 << 53) }
    mutating func uniform(_ a: Double, _ b: Double) -> Double { a + (b - a) * uniform() }
    mutating func index(_ n: Int) -> Int { Int(next() % UInt64(max(n, 1))) }
    mutating func sign() -> Double { next() & 1 == 0 ? -1 : 1 }
    mutating func normal() -> Double {
        let u1 = max(uniform(), 1e-300)
        return (-2 * log(u1)).squareRoot() * cos(2 * Double.pi * uniform())
    }
    /// 批量标准正态（Box-Muller 成对产出），给噪点用。
    mutating func fillNormal(_ out: UnsafeMutablePointer<Float>, _ n: Int, scale: Float) {
        var i = 0
        while i < n {
            let u1 = Float(max(uniform(), 1e-12))
            let u2 = Float(uniform())
            let r = (-2 * logf(u1)).squareRoot() * scale
            out[i] = r * cosf(2 * .pi * u2)
            if i + 1 < n { out[i + 1] = r * sinf(2 * .pi * u2) }
            i += 2
        }
    }
}

// MARK: - 轨迹

struct Motion: Decodable {
    let still_time: Double
    let times, yaw, pitch, roll, scale: [Double]
}

struct Trajectory {
    var stillTime: Double
    var times: [Double]
    var yaw, pitch, roll, scale: [Double]
    var preMask: [Bool]

    mutating func shrink(_ factor: Double, all: Bool) {
        for i in times.indices where all || preMask[i] {
            yaw[i] *= factor
            pitch[i] *= factor
            roll[i] *= factor
            scale[i] = 1 + (scale[i] - 1) * factor
        }
    }
}

func nearest(_ values: [Double], _ v: Double) -> Int {
    var best = 0
    for i in values.indices where abs(values[i] - v) < abs(values[best] - v) { best = i }
    return best
}

/// 快门前取 pre、快门后取 post 拼成一条轨迹。
func compose(_ rng: inout Rng, pre: Motion, post: Motion) -> Trajectory {
    let ka = nearest(pre.times, pre.still_time)
    let kb = nearest(post.times, post.still_time)
    let tStill = pre.times[ka] - pre.times[0]
    let a = Array(0...ka)
    let b = Array((kb + 1)..<post.times.count)
    let times = a.map { pre.times[$0] - pre.times[ka] + tStill } + b.map { post.times[$0] - post.times[kb] + tStill }
    let mirrorYaw = rng.sign(), mirrorRoll = rng.sign()
    let ampPre = rng.uniform(0.75, 1.05), ampPost = rng.uniform(0.8, 1.2)
    func mix(_ p: [Double], _ q: [Double], base: Double, mirror: Double) -> [Double] {
        a.map { base + (p[$0] - base) * mirror * ampPre } + b.map { base + (q[$0] - base) * mirror * ampPost }
    }
    return Trajectory(
        stillTime: tStill, times: times,
        yaw: mix(pre.yaw, post.yaw, base: 0, mirror: mirrorYaw),
        pitch: mix(pre.pitch, post.pitch, base: 0, mirror: 1),
        roll: mix(pre.roll, post.roll, base: 0, mirror: mirrorRoll),
        scale: mix(pre.scale, post.scale, base: 1, mirror: 1),
        preMask: a.map { _ in true } + b.map { _ in false })
}

func interp(_ t: Double, _ xs: [Double], _ ys: [Double]) -> Double {
    if t <= xs[0] { return ys[0] }
    if t >= xs[xs.count - 1] { return ys[ys.count - 1] }
    var hi = 1
    while xs[hi] < t { hi += 1 }
    let lo = hi - 1
    let span = xs[hi] - xs[lo]
    return span <= 0 ? ys[hi] : ys[lo] + (ys[hi] - ys[lo]) * (t - xs[lo]) / span
}

func mat(_ r: [[Double]]) -> double3x3 {
    double3x3(rows: r.map { SIMD3($0[0], $0[1], $0[2]) })
}

func rotationHomography(_ w: Double, _ h: Double, _ yaw: Double, _ pitch: Double, _ roll: Double) -> double3x3 {
    let f = 0.75 * max(w, h)  // 约等于手机主摄 26mm 等效焦距
    let k = mat([[f, 0, w / 2], [0, f, h / 2], [0, 0, 1]])
    let (a, b, c) = (yaw * .pi / 180, pitch * .pi / 180, roll * .pi / 180)
    let ry = mat([[cos(a), 0, sin(a)], [0, 1, 0], [-sin(a), 0, cos(a)]])
    let rx = mat([[1, 0, 0], [0, cos(b), -sin(b)], [0, sin(b), cos(b)]])
    let rz = mat([[cos(c), -sin(c), 0], [sin(c), cos(c), 0], [0, 0, 1]])
    return k * rz * rx * ry * k.inverse
}

func zoomMatrix(_ w: Double, _ h: Double, _ s: Double) -> double3x3 {
    mat([[s, 0, w / 2 * (1 - s)], [0, s, h / 2 * (1 - s)], [0, 0, 1]])
}

func apply(_ m: double3x3, _ x: Double, _ y: Double) -> (Double, Double) {
    let p = m * SIMD3(x, y, 1)
    return (p.x / p.z, p.y / p.z)
}

/// 单应 H 之后再放大多少倍，输出画面才完全落在原图内（不露黑边）。
func coverZoom(_ w: Double, _ h: Double, _ hm: double3x3) -> Double {
    var lo = 1.0, hi = 2.0
    for _ in 0..<25 {
        let mid = (lo + hi) / 2
        let inv = (zoomMatrix(w, h, mid) * hm).inverse
        let ok = [(0.0, 0.0), (w, 0), (w, h), (0, h)].allSatisfy { corner in
            let (x, y) = apply(inv, corner.0, corner.1)
            return x >= 0 && y >= 0 && x <= w && y <= h
        }
        if ok { hi = mid } else { lo = mid }
    }
    return hi
}

func maximumFilter(_ v: [Double], size: Int) -> [Double] {
    let r = size / 2
    return v.indices.map { i in
        (max(0, i - r)...min(v.count - 1, i + r)).map { v[$0] }.max() ?? v[i]
    }
}

func gaussianFilter(_ v: [Double], sigma: Double) -> [Double] {
    let r = Int(4 * sigma + 0.5)
    let weights = (-r...r).map { exp(-0.5 * Double($0 * $0) / (sigma * sigma)) }
    let total = weights.reduce(0, +)
    return v.indices.map { i in
        var acc = 0.0
        for (j, w) in weights.enumerated() {
            acc += w * v[min(max(i + j - r, 0), v.count - 1)]
        }
        return acc / total
    }
}

/// 每帧 sub 个子采样的变换矩阵，以及每帧的防黑边放大倍数。
func frameMatrices(_ w: Double, _ h: Double, _ t: Trajectory) -> ([[double3x3]], [Double]) {
    let offsets = (0..<sub).map { (Double($0) / Double(sub - 1) - 0.5) * exposure }
    var homs: [[double3x3]] = []
    var need: [Double] = []
    for t0 in t.times {
        let row = offsets.map { off -> double3x3 in
            let at = t0 + off
            return zoomMatrix(w, h, interp(at, t.times, t.scale))
                * rotationHomography(w, h, interp(at, t.times, t.yaw), interp(at, t.times, t.pitch), interp(at, t.times, t.roll))
        }
        homs.append(row)
        need.append((row.map { coverZoom(w, h, $0) }.max() ?? 1) * 1.003)
    }
    // 放大倍数要慢慢变，不然看起来像在抽动变焦
    let smooth = gaussianFilter(maximumFilter(need, size: 11), sigma: 3)
    return (homs, zip(smooth, need).map { max($0, $1) })
}

/// 频带限制的随机曲线（0.2–1 Hz），峰值归一化到 ±1。
func bandNoise(_ rng: inout Rng, n: Int, rate: Double, lo: Double, hi: Double) -> [Double] {
    let x = (0..<n).map { _ in rng.normal() }
    var y = [Double](repeating: 0, count: n)
    for k in 1..<max(n / 2, 1) {
        let f = Double(k) * rate / Double(n)
        guard f >= lo && f <= hi else { continue }
        var re = 0.0, im = 0.0
        for (t, v) in x.enumerated() {
            let ph = -2 * Double.pi * Double(k * t) / Double(n)
            re += v * cos(ph)
            im += v * sin(ph)
        }
        for t in 0..<n {
            let ph = 2 * Double.pi * Double(k * t) / Double(n)
            y[t] += 2 / Double(n) * (re * cos(ph) - im * sin(ph))
        }
    }
    let peak = y.map(abs).max() ?? 0
    return y.map { $0 / (peak + 1e-12) }
}

// MARK: - 画面

let srgb = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
let context = CIContext(options: [.workingColorSpace: srgb, .outputColorSpace: srgb, .cacheIntermediates: false])

func lanczos(_ image: CIImage, toWidth w: Int, height h: Int) -> CIImage {
    let src = image.extent
    let sy = Double(h) / src.height
    let sx = Double(w) / src.width
    let scaled = image.clampedToExtent().cropped(to: src).applyingFilter("CILanczosScaleTransform", parameters: [
        kCIInputScaleKey: sy, kCIInputAspectRatioKey: sx / sy,
    ])
    return scaled.cropped(to: CGRect(x: 0, y: 0, width: w, height: h))
}

/// 居中裁成 3:4 竖图，原点归零。
func crop3x4(_ image: CIImage) -> CIImage {
    let img = image.transformed(by: CGAffineTransform(translationX: -image.extent.minX, y: -image.extent.minY))
    let w = Int(img.extent.width), h = Int(img.extent.height)
    let rect: CGRect
    if h * 3 > w * 4 {
        let ch = Int((Double(w) * 4 / 3).rounded())
        rect = CGRect(x: 0, y: (h - ch) / 2, width: w, height: ch)
    } else {
        let cw = Int((Double(h) * 3 / 4).rounded())
        rect = CGRect(x: (w - cw) / 2, y: 0, width: cw, height: h)
    }
    return img.cropped(to: rect).transformed(by: CGAffineTransform(translationX: -rect.minX, y: -rect.minY))
}

/// 把上左原点的单应 m 作用到整张 work 图，输出 videoW×videoH（CoreImage 是下左原点）。
func warp(_ work: CIImage, _ m: double3x3, w: Double, h: Double) -> CIImage {
    func point(_ x: Double, _ y: Double) -> CIVector {
        let (u, v) = apply(m, x, y)
        return CIVector(x: u, y: Double(videoH) - v)
    }
    return work.applyingFilter("CIPerspectiveTransform", parameters: [
        "inputTopLeft": point(0, 0), "inputTopRight": point(w, 0),
        "inputBottomRight": point(w, h), "inputBottomLeft": point(0, h),
    ]).cropped(to: CGRect(x: 0, y: 0, width: videoW, height: videoH))
}

/// 每帧不同的噪点：暗部更明显，带一点低分辨率的色噪。直接改写 BGRA 像素。
struct SensorNoise {
    var rng: Rng
    let w = videoW, h = videoH
    let cw = videoW / 4, ch = videoH / 4
    var grain: [Float]
    var blurred: [Float]
    var chroma: [[Float]]
    var chromaFull: [[Float]]
    let kernel: [Float]

    init(seed: UInt64) {
        rng = Rng(state: seed)
        grain = [Float](repeating: 0, count: videoW * videoH)
        blurred = grain
        chroma = (0..<3).map { _ in [Float](repeating: 0, count: (videoW / 4) * (videoH / 4)) }
        chromaFull = (0..<3).map { _ in [Float](repeating: 0, count: videoW * videoH) }
        // 与 OpenCV GaussianBlur(sigma=0.6) 同核宽：7 抽头。
        let raw = (-3...3).map { Float(exp(-0.5 * Double($0 * $0) / 0.36)) }
        let total = raw.reduce(0, +)
        kernel = raw.map { $0 / total }
    }

    mutating func apply(to buffer: CVPixelBuffer) {
        let n = w * h
        grain.withUnsafeMutableBufferPointer { rng.fillNormal($0.baseAddress!, n, scale: 1) }
        var tmp = [Float](repeating: 0, count: n)
        grain.withUnsafeMutableBufferPointer { g in
            tmp.withUnsafeMutableBufferPointer { t in
                blurred.withUnsafeMutableBufferPointer { b in
                    var src = vImage_Buffer(data: g.baseAddress, height: vImagePixelCount(h), width: vImagePixelCount(w), rowBytes: w * 4)
                    var mid = vImage_Buffer(data: t.baseAddress, height: vImagePixelCount(h), width: vImagePixelCount(w), rowBytes: w * 4)
                    var dst = vImage_Buffer(data: b.baseAddress, height: vImagePixelCount(h), width: vImagePixelCount(w), rowBytes: w * 4)
                    kernel.withUnsafeBufferPointer { k in
                        vImageConvolve_PlanarF(&src, &mid, nil, 0, 0, k.baseAddress!, 1, 7, 0, vImage_Flags(kvImageEdgeExtend))
                        vImageConvolve_PlanarF(&mid, &dst, nil, 0, 0, k.baseAddress!, 7, 1, 0, vImage_Flags(kvImageEdgeExtend))
                    }
                }
            }
        }
        for c in 0..<3 {
            chroma[c].withUnsafeMutableBufferPointer { rng.fillNormal($0.baseAddress!, cw * ch, scale: 0.9) }
            chroma[c].withUnsafeMutableBufferPointer { small in
                chromaFull[c].withUnsafeMutableBufferPointer { full in
                    var s = vImage_Buffer(data: small.baseAddress, height: vImagePixelCount(ch), width: vImagePixelCount(cw), rowBytes: cw * 4)
                    var d = vImage_Buffer(data: full.baseAddress, height: vImagePixelCount(h), width: vImagePixelCount(w), rowBytes: w * 4)
                    _ = vImageScale_PlanarF(&s, &d, nil, vImage_Flags(kvImageHighQualityResampling))
                }
            }
        }
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let base = CVPixelBufferGetBaseAddress(buffer)?.assumingMemoryBound(to: UInt8.self) else { return }
        let stride = CVPixelBufferGetBytesPerRow(buffer)
        blurred.withUnsafeBufferPointer { g in
            chromaFull[0].withUnsafeBufferPointer { c0 in
                chromaFull[1].withUnsafeBufferPointer { c1 in
                    chromaFull[2].withUnsafeBufferPointer { c2 in
                        for y in 0..<h {
                            let row = base + y * stride
                            for x in 0..<w {
                                let i = y * w + x
                                let p = row + x * 4
                                let bv = Float(p[0]), gv = Float(p[1]), rv = Float(p[2])
                                let sigma = 1 + 2.2 * (1 - (bv + gv + rv) / 765)
                                let gr = g[i] * sigma * 1.4
                                p[0] = UInt8(min(max(bv + gr + c0[i], 0), 255))
                                p[1] = UInt8(min(max(gv + gr + c1[i], 0), 255))
                                p[2] = UInt8(min(max(rv + gr + c2[i], 0), 255))
                            }
                        }
                    }
                }
            }
        }
    }
}

// MARK: - 写文件

let captureDate = Date()

func dateString(_ format: String) -> String {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.dateFormat = format
    return f.string(from: captureDate)
}

func writeStill(_ image: CGImage, to dst: URL, assetID: String) throws {
    guard let dest = CGImageDestinationCreateWithURL(dst as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else {
        throw Failure("无法写入 \(dst.path)")
    }
    let exifDate = dateString("yyyy:MM:dd HH:mm:ss")
    let offset = dateString("xxx")
    let props: [CFString: Any] = [
        kCGImagePropertyMakerAppleDictionary: ["17": assetID],
        kCGImagePropertyExifDictionary: [
            kCGImagePropertyExifDateTimeOriginal: exifDate,
            kCGImagePropertyExifDateTimeDigitized: exifDate,
            kCGImagePropertyExifOffsetTimeOriginal: offset,
            kCGImagePropertyExifOffsetTimeDigitized: offset,
        ],
        kCGImagePropertyTIFFDictionary: [kCGImagePropertyTIFFDateTime: exifDate],
        kCGImageDestinationLossyCompressionQuality: 0.92,
    ]
    CGImageDestinationAddImage(dest, image, props as CFDictionary)
    guard CGImageDestinationFinalize(dest) else { throw Failure("无法写入 \(dst.path)") }
}

func metadataItem(_ key: String, _ value: NSCopying & NSObjectProtocol, _ type: String) -> AVMutableMetadataItem {
    let item = AVMutableMetadataItem()
    item.key = key as NSString
    item.keySpace = .quickTimeMetadata
    item.value = value
    item.dataType = type
    return item
}

/// 原声：解码成 PCM 再编 AAC；从后台队列按需喂，避免与视频交织时互相等待。
func feedAudio(_ url: URL, into input: AVAssetWriterInput, until end: CMTime, group: DispatchGroup) throws {
    let asset = AVURLAsset(url: url)
    guard let track = asset.tracks(withMediaType: .audio).first else { throw Failure("原声文件没有音轨 \(url.lastPathComponent)") }
    let reader = try AVAssetReader(asset: asset)
    let output = AVAssetReaderTrackOutput(track: track, outputSettings: [
        AVFormatIDKey: kAudioFormatLinearPCM, AVLinearPCMBitDepthKey: 16,
        AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false, AVLinearPCMIsNonInterleaved: false,
    ])
    reader.add(output)
    guard reader.startReading() else { throw Failure("原声读取失败：\(String(describing: reader.error))") }
    group.enter()
    var done = false
    input.requestMediaDataWhenReady(on: DispatchQueue(label: "gendesk-live.audio")) {
        while !done && input.isReadyForMoreMediaData {
            if let buffer = output.copyNextSampleBuffer(),
               CMTimeCompare(CMSampleBufferGetPresentationTimeStamp(buffer), end) < 0,
               input.append(buffer) {
                continue
            }
            done = true
            reader.cancelReading()
            input.markAsFinished()
            group.leave()
        }
    }
}

// MARK: - 主流程

struct Options {
    var resources = URL(fileURLWithPath: "")
    var outDir = URL(fileURLWithPath: "")
    var image = URL(fileURLWithPath: "")
    var seed = UInt64.random(in: 0..<1_000_000)
    var amp = 0.75
    var audio = true
}

func parse() throws -> Options {
    var opts = Options()
    var positional: [String] = []
    var args = CommandLine.arguments.dropFirst().makeIterator()
    var resources: String?
    while let a = args.next() {
        switch a {
        case "--resources": resources = args.next()
        case "--seed": opts.seed = UInt64(args.next() ?? "") ?? opts.seed
        case "--amp": opts.amp = Double(args.next() ?? "") ?? opts.amp
        case "--no-audio": opts.audio = false
        default: positional.append(a)
        }
    }
    guard let resources, positional.count == 2 else {
        throw Failure("用法: gendesk-live --resources <目录> <输出目录> <原图> [--seed N] [--amp 0.75] [--no-audio]")
    }
    opts.resources = URL(fileURLWithPath: resources)
    opts.outDir = URL(fileURLWithPath: positional[0])
    opts.image = URL(fileURLWithPath: positional[1])
    return opts
}

func files(_ dir: URL, _ ext: String) -> [URL] {
    let items = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
    return items.filter { $0.pathExtension.lowercased() == ext }.sorted { $0.lastPathComponent < $1.lastPathComponent }
}

func run() throws -> String {
    let opts = try parse()
    let library = try files(opts.resources.appendingPathComponent("motion"), "json").map {
        try JSONDecoder().decode(Motion.self, from: Data(contentsOf: $0))
    }
    guard !library.isEmpty else { throw Failure("轨迹库为空：\(opts.resources.path)/motion") }
    let audios = opts.audio ? files(opts.resources.appendingPathComponent("audio"), "m4a") : []
    if opts.audio && audios.isEmpty { throw Failure("实拍原声库为空：\(opts.resources.path)/audio") }

    guard let source = CIImage(contentsOf: opts.image, options: [.applyOrientationProperty: true]) else {
        throw Failure("无法读取原图 \(opts.image.path)")
    }
    let stem = opts.image.deletingPathExtension().lastPathComponent
    let full = crop3x4(source)
    let fullW = Int(full.extent.width), fullH = Int(full.extent.height)
    // 原图比 3024 宽就保留原尺寸，否则放大到 12MP
    let (sw, sh) = fullW >= stillW ? (fullW, fullH) : (stillW, stillH)
    // 视频先缩到略大于输出，避免大图直接缩小时出现锯齿
    let workW = Int(Double(videoW) * 1.25)
    let work = fullW > workW
        ? lanczos(full, toWidth: workW, height: Int((Double(workW) * Double(fullH) / Double(fullW)).rounded()))
        : full
    let w = Double(work.extent.width), h = Double(work.extent.height)

    var rng = Rng(state: opts.seed)
    let pre = rng.index(library.count)
    let others = library.indices.filter { $0 != pre }
    let post = others.isEmpty ? pre : others[rng.index(others.count)]
    var traj = compose(&rng, pre: library[pre], post: library[post])

    // 先压快门前那段；压了 10 轮还超，说明超的是快门后那段，整条一起压
    var (homs, zoom) = frameMatrices(w, h, traj)
    for round in 0..<200 where (zoom.max() ?? 1) > maxZoom {
        traj.shrink(0.9, all: round >= 10)
        (homs, zoom) = frameMatrices(w, h, traj)
    }
    // 整体幅度系数放在防黑边压缩之后，保证每张都按同一比例变小
    if opts.amp != 1 {
        traj.shrink(opts.amp, all: true)
        (homs, zoom) = frameMatrices(w, h, traj)
    }
    let times = traj.times
    guard times.count >= 2 else { throw Failure("轨迹帧数不足") }
    let k = nearest(times, traj.stillTime)
    var gain = bandNoise(&rng, n: times.count, rate: 30, lo: 0.2, hi: 1.0).map { 1 + 0.008 * $0 }
    let g0 = gain[k]
    gain = gain.map { $0 / g0 }
    let audioURL = audios.isEmpty ? nil : audios[rng.index(audios.count)]

    try FileManager.default.createDirectory(at: opts.outDir, withIntermediateDirectories: true)
    let assetID = UUID().uuidString
    let jpg = opts.outDir.appendingPathComponent("\(stem).JPG")
    let mov = opts.outDir.appendingPathComponent("\(stem).MOV")

    // 静态图：快门时刻那一帧，不加噪点；放大与缩放合成一次重采样
    let z = zoom[k]
    let scaled = lanczos(full, toWidth: Int((Double(sw) * z).rounded()), height: Int((Double(sh) * z).rounded()))
    let still = scaled
        .transformed(by: CGAffineTransform(translationX: -(scaled.extent.width - Double(sw)) / 2,
                                           y: -(scaled.extent.height - Double(sh)) / 2))
        .cropped(to: CGRect(x: 0, y: 0, width: sw, height: sh))
    guard let stillCG = context.createCGImage(still, from: still.extent, format: .RGBA8, colorSpace: srgb) else {
        throw Failure("静态图渲染失败")
    }
    try writeStill(stillCG, to: jpg, assetID: assetID)

    // 视频
    try? FileManager.default.removeItem(at: mov)
    let writer = try AVAssetWriter(outputURL: mov, fileType: .mov)
    writer.metadata = [
        metadataItem("com.apple.quicktime.content.identifier", assetID as NSString, "com.apple.metadata.datatype.UTF-8"),
        metadataItem("com.apple.quicktime.creationdate", dateString("yyyy-MM-dd'T'HH:mm:ssxx") as NSString, "com.apple.metadata.datatype.UTF-8"),
    ]
    let videoIn = AVAssetWriterInput(mediaType: .video, outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.hevc,
        AVVideoWidthKey: videoW, AVVideoHeightKey: videoH,
        AVVideoCompressionPropertiesKey: [
            AVVideoAverageBitRateKey: 12_000_000,
            AVVideoAllowFrameReorderingKey: false,
            AVVideoExpectedSourceFrameRateKey: 30,
        ],
        AVVideoColorPropertiesKey: [
            AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
            AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
            AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
        ],
    ])
    videoIn.expectsMediaDataInRealTime = false
    let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: videoIn, sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: videoW, kCVPixelBufferHeightKey as String: videoH,
    ])
    writer.add(videoIn)

    var audioIn: AVAssetWriterInput?
    if audioURL != nil {
        let input = AVAssetWriterInput(mediaType: .audio, outputSettings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48_000,
            AVNumberOfChannelsKey: 2, AVEncoderBitRateKey: 128_000,
        ])
        input.expectsMediaDataInRealTime = false
        writer.add(input)
        audioIn = input
    }

    let spec: [NSString: Any] = [
        kCMMetadataFormatDescriptionMetadataSpecificationKey_Identifier: "mdta/com.apple.quicktime.still-image-time",
        kCMMetadataFormatDescriptionMetadataSpecificationKey_DataType: "com.apple.metadata.datatype.int8",
    ]
    var metaFormat: CMFormatDescription?
    CMMetadataFormatDescriptionCreateWithMetadataSpecifications(
        allocator: kCFAllocatorDefault, metadataType: kCMMetadataFormatType_Boxed,
        metadataSpecifications: [spec] as CFArray, formatDescriptionOut: &metaFormat)
    let metaIn = AVAssetWriterInput(mediaType: .metadata, outputSettings: nil, sourceFormatHint: metaFormat)
    let metaAdaptor = AVAssetWriterInputMetadataAdaptor(assetWriterInput: metaIn)
    writer.add(metaIn)

    guard writer.startWriting() else { throw Failure("视频写入失败：\(String(describing: writer.error))") }
    writer.startSession(atSourceTime: .zero)

    // 实拍的 still-image-time 值是 -1
    let stillTime = CMTime(seconds: traj.stillTime, preferredTimescale: 600)
    metaAdaptor.append(AVTimedMetadataGroup(
        items: [metadataItem("com.apple.quicktime.still-image-time", -1 as NSNumber, "com.apple.metadata.datatype.int8")],
        timeRange: CMTimeRange(start: stillTime, duration: CMTime(value: 1, timescale: 600))))
    metaIn.markAsFinished()

    let duration = times[times.count - 1] + (times[times.count - 1] - times[times.count - 2])
    let end = CMTime(seconds: duration, preferredTimescale: 600)
    let group = DispatchGroup()
    if let audioURL, let audioIn { try feedAudio(audioURL, into: audioIn, until: end, group: group) }

    guard let pool = adaptor.pixelBufferPool else { throw Failure("视频缓冲区创建失败") }
    var noise = SensorNoise(seed: opts.seed &+ 1000)
    let s = mat([[Double(videoW) / w, 0, 0], [0, Double(videoH) / h, 0], [0, 0, 1]])
    for i in times.indices {
        let zm = s * zoomMatrix(w, h, zoom[i])
        var frame: CIImage?
        let g = gain[i]
        for hm in homs[i] {
            // 未预乘的 RGB 乘增益、alpha 置 1/sub：六张相加后正好是平均值且不透明。
            let layer = warp(work, zm * hm, w: w, h: h).applyingFilter("CIColorMatrix", parameters: [
                "inputRVector": CIVector(x: g, y: 0, z: 0, w: 0),
                "inputGVector": CIVector(x: 0, y: g, z: 0, w: 0),
                "inputBVector": CIVector(x: 0, y: 0, z: g, w: 0),
                "inputAVector": CIVector(x: 0, y: 0, z: 0, w: 0),
                "inputBiasVector": CIVector(x: 0, y: 0, z: 0, w: 1.0 / Double(sub)),
            ])
            frame = frame.map { layer.applyingFilter("CIAdditionCompositing", parameters: [kCIInputBackgroundImageKey: $0]) } ?? layer
        }
        guard let frame else { throw Failure("视频帧为空") }
        var out: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(nil, pool, &out)
        guard let buffer = out else { throw Failure("视频缓冲区不足") }
        context.render(frame, to: buffer, bounds: CGRect(x: 0, y: 0, width: videoW, height: videoH), colorSpace: srgb)
        noise.apply(to: buffer)
        while !videoIn.isReadyForMoreMediaData {
            if writer.status == .failed { throw Failure("视频编码失败：\(String(describing: writer.error))") }
            Thread.sleep(forTimeInterval: 0.002)
        }
        guard adaptor.append(buffer, withPresentationTime: CMTime(seconds: times[i], preferredTimescale: 600)) else {
            throw Failure("视频编码失败：\(String(describing: writer.error))")
        }
    }
    videoIn.markAsFinished()
    group.wait()
    writer.endSession(atSourceTime: end)
    let closed = DispatchSemaphore(value: 0)
    writer.finishWriting { closed.signal() }
    closed.wait()
    guard writer.status == .completed else { throw Failure("视频写入失败：\(String(describing: writer.error))") }
    return assetID
}

do {
    print(try run())
} catch {
    FileHandle.standardError.write(Data("gendesk-live: \(error)\n".utf8))
    exit(1)
}
