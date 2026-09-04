import CoreGraphics
import Foundation
import ImageIO
import Vision

struct OcrBlock: Encodable {
    let text: String
    let x: Double
    let y: Double
    let w: Double
    let h: Double
    let confidence: Double
}

struct OcrImage: Encodable {
    let path: String
    let width: Int
    let height: Int
    let blocks: [OcrBlock]
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

let arguments = Array(CommandLine.arguments.dropFirst())
var level = "accurate"
var languages = ["ja", "en"]
var paths: [String] = []
var index = 0

while index < arguments.count {
    let argument = arguments[index]
    switch argument {
    case "--level":
        index += 1
        guard index < arguments.count else { fail("--level の値がありません") }
        level = arguments[index]
        guard level == "fast" || level == "accurate" else { fail("--level は fast または accurate です") }
    case "--languages":
        index += 1
        guard index < arguments.count else { fail("--languages の値がありません") }
        languages = arguments[index].split(separator: ",").map(String.init).filter { !$0.isEmpty }
        guard !languages.isEmpty else { fail("--languages が空です") }
    case "--help":
        print("使い方: coosenpai-ocr [--level fast|accurate] [--languages ja,en] image.png ...")
        exit(0)
    default:
        guard !argument.hasPrefix("--") else { fail("不明な引数です: \(argument)") }
        paths.append(argument)
    }
    index += 1
}

guard !paths.isEmpty else { fail("PNG パスを指定してください") }

let encoder = JSONEncoder()

for path in paths {
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil), let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        fail("PNG を読み込めません: \(path)")
    }
    guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB), let context = CGContext(
        data: nil,
        width: image.width,
        height: image.height,
        bitsPerComponent: 8,
        bytesPerRow: image.width * 4,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        fail("画像の色空間を準備できません: \(path)")
    }
    context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
    guard let normalizedImage = context.makeImage() else {
        fail("画像を正規化できません: \(path)")
    }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = level == "fast" ? .fast : .accurate
    request.recognitionLanguages = languages.map(recognitionLanguage)
    request.usesLanguageCorrection = true
    let handler = VNImageRequestHandler(cgImage: normalizedImage, orientation: .up, options: [:])
    do {
        try handler.perform([request])
    } catch {
        fail("OCR に失敗しました: \(path): \(error.localizedDescription)")
    }
    let blocks = (request.results ?? []).compactMap { observation -> OcrBlock? in
        guard let candidate = observation.topCandidates(1).first else { return nil }
        let box = observation.boundingBox
        return OcrBlock(
            text: candidate.string,
            x: box.origin.x,
            y: 1.0 - box.origin.y - box.size.height,
            w: box.size.width,
            h: box.size.height,
            confidence: Double(candidate.confidence)
        )
    }
    let output = OcrImage(path: path, width: image.width, height: image.height, blocks: blocks)
    do {
        FileHandle.standardOutput.write(try encoder.encode(output))
        FileHandle.standardOutput.write(Data("\n".utf8))
    } catch {
        fail("JSON の出力に失敗しました")
    }
}

func recognitionLanguage(_ language: String) -> String {
    switch language {
    case "ja": return "ja-JP"
    case "en": return "en-US"
    default: return language
    }
}
